//! Executable release-policy kernel mirrored by `formal/release_gate.qnt`.
//!
//! This module makes the machine-release invariants usable from Rust without
//! claiming that a release preview or this service certifies shop-floor safety.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::{self, Display, Formatter},
};

/// Release-blocking evidence families modeled by the formal specification.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReleaseGate {
    SourceProvenance,
    MachineEnvelope,
    ProcessReadiness,
    SimulationAndQuality,
    HumanOrAutomationHandoff,
}

pub const RELEASE_GATES: [ReleaseGate; 5] = [
    ReleaseGate::SourceProvenance,
    ReleaseGate::MachineEnvelope,
    ReleaseGate::ProcessReadiness,
    ReleaseGate::SimulationAndQuality,
    ReleaseGate::HumanOrAutomationHandoff,
];

/// A review snapshot. `gates_clear` is evidence eligibility, not authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePreview {
    pub job_id: String,
    pub revision: u64,
    pub gates_clear: bool,
    pub blocked_gates: Vec<ReleaseGate>,
}

/// The idempotent logical authorization for one immutable job revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseAuthorization {
    pub job_id: String,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleasePolicyError {
    InvalidInitialRevision,
    JobMismatch { expected: String, actual: String },
    RevisionMismatch { expected: u64, actual: u64 },
    RevisionMustIncrease { current: u64, proposed: u64 },
    ValidationRequired,
    SafePreviewRequired,
    AuthorizedRevisionImmutable,
}

impl Display for ReleasePolicyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInitialRevision => formatter.write_str("revision must be positive"),
            Self::JobMismatch { expected, actual } => {
                write!(formatter, "expected job {expected}, received {actual}")
            }
            Self::RevisionMismatch { expected, actual } => {
                write!(formatter, "expected revision {expected}, received {actual}")
            }
            Self::RevisionMustIncrease { current, proposed } => write!(
                formatter,
                "revision must increase from {current}; received {proposed}"
            ),
            Self::ValidationRequired => {
                formatter.write_str("the current revision must be validated first")
            }
            Self::SafePreviewRequired => {
                formatter.write_str("a current preview with every gate clear is required")
            }
            Self::AuthorizedRevisionImmutable => formatter
                .write_str("an authorized revision is immutable; create a new revision first"),
        }
    }
}

impl Error for ReleasePolicyError {}

/// Mutable evidence state for one fabrication job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePolicy {
    job_id: String,
    revision: u64,
    validated_revision: Option<u64>,
    evidence_revision: BTreeMap<ReleaseGate, u64>,
    blockers: BTreeSet<ReleaseGate>,
    preview: Option<ReleasePreview>,
    authorization: Option<ReleaseAuthorization>,
}

impl ReleasePolicy {
    pub fn new(job_id: impl Into<String>, revision: u64) -> Result<Self, ReleasePolicyError> {
        if revision == 0 {
            return Err(ReleasePolicyError::InvalidInitialRevision);
        }
        Ok(Self {
            job_id: job_id.into(),
            revision,
            validated_revision: None,
            evidence_revision: BTreeMap::new(),
            blockers: RELEASE_GATES.into_iter().collect(),
            preview: None,
            authorization: None,
        })
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn blocked_gates(&self) -> Vec<ReleaseGate> {
        self.blockers.iter().copied().collect()
    }

    pub fn machine_ready(&self) -> bool {
        self.authorization.as_ref().is_some_and(|authorization| {
            authorization.job_id == self.job_id
                && authorization.revision == self.revision
                && self.release_preconditions_hold()
        })
    }

    pub fn validate(&mut self, job_id: &str, revision: u64) -> Result<(), ReleasePolicyError> {
        self.require_current(job_id, revision)?;
        if self.validated_revision == Some(revision) {
            return Ok(());
        }
        self.validated_revision = Some(revision);
        self.preview = None;
        self.authorization = None;
        Ok(())
    }

    pub fn record_evidence(
        &mut self,
        job_id: &str,
        revision: u64,
        gate: ReleaseGate,
    ) -> Result<(), ReleasePolicyError> {
        self.require_current(job_id, revision)?;
        if self.validated_revision != Some(revision) {
            return Err(ReleasePolicyError::ValidationRequired);
        }
        if self.evidence_revision.get(&gate) == Some(&revision) && !self.blockers.contains(&gate) {
            return Ok(());
        }
        self.evidence_revision.insert(gate, revision);
        self.blockers.remove(&gate);
        self.preview = None;
        self.authorization = None;
        Ok(())
    }

    /// Reopen a gate before authorization.
    ///
    /// Once authorized, the revision is immutable. New evidence or a new
    /// blocker must be represented by `revise`, which was the counterexample
    /// discovered by the first exhaustive model.
    pub fn reopen_gate(
        &mut self,
        job_id: &str,
        revision: u64,
        gate: ReleaseGate,
    ) -> Result<(), ReleasePolicyError> {
        self.require_current(job_id, revision)?;
        if self.machine_ready() {
            return Err(ReleasePolicyError::AuthorizedRevisionImmutable);
        }
        self.evidence_revision.remove(&gate);
        self.blockers.insert(gate);
        self.preview = None;
        Ok(())
    }

    pub fn preview(
        &mut self,
        job_id: &str,
        revision: u64,
    ) -> Result<ReleasePreview, ReleasePolicyError> {
        self.require_current(job_id, revision)?;
        let preview = ReleasePreview {
            job_id: self.job_id.clone(),
            revision: self.revision,
            gates_clear: self.release_preconditions_hold(),
            blocked_gates: self.blocked_gates(),
        };
        self.preview = Some(preview.clone());
        Ok(preview)
    }

    pub fn authorize(
        &mut self,
        job_id: &str,
        revision: u64,
    ) -> Result<ReleaseAuthorization, ReleasePolicyError> {
        self.require_current(job_id, revision)?;
        if let Some(authorization) = &self.authorization {
            return Ok(authorization.clone());
        }
        let preview_is_safe = self.preview.as_ref().is_some_and(|preview| {
            preview.job_id == self.job_id
                && preview.revision == self.revision
                && preview.gates_clear
        });
        if !preview_is_safe || !self.release_preconditions_hold() {
            return Err(ReleasePolicyError::SafePreviewRequired);
        }
        let authorization = ReleaseAuthorization {
            job_id: self.job_id.clone(),
            revision: self.revision,
        };
        self.authorization = Some(authorization.clone());
        Ok(authorization)
    }

    pub fn revise(
        &mut self,
        job_id: &str,
        current_revision: u64,
        new_revision: u64,
    ) -> Result<(), ReleasePolicyError> {
        self.require_current(job_id, current_revision)?;
        if new_revision <= self.revision {
            return Err(ReleasePolicyError::RevisionMustIncrease {
                current: self.revision,
                proposed: new_revision,
            });
        }
        self.revision = new_revision;
        self.validated_revision = None;
        self.evidence_revision.clear();
        self.blockers = RELEASE_GATES.into_iter().collect();
        self.preview = None;
        self.authorization = None;
        Ok(())
    }

    fn require_current(&self, job_id: &str, revision: u64) -> Result<(), ReleasePolicyError> {
        if job_id != self.job_id {
            return Err(ReleasePolicyError::JobMismatch {
                expected: self.job_id.clone(),
                actual: job_id.to_owned(),
            });
        }
        if revision != self.revision {
            return Err(ReleasePolicyError::RevisionMismatch {
                expected: self.revision,
                actual: revision,
            });
        }
        Ok(())
    }

    fn release_preconditions_hold(&self) -> bool {
        self.validated_revision == Some(self.revision)
            && self.blockers.is_empty()
            && RELEASE_GATES
                .iter()
                .all(|gate| self.evidence_revision.get(gate) == Some(&self.revision))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validated_policy() -> ReleasePolicy {
        let mut policy = ReleasePolicy::new("job-1", 1).unwrap();
        policy.validate("job-1", 1).unwrap();
        policy
    }

    fn clear_every_gate(policy: &mut ReleasePolicy) {
        for gate in RELEASE_GATES {
            policy.record_evidence("job-1", 1, gate).unwrap();
        }
    }

    #[test]
    fn preview_never_authorizes_machine_execution() {
        let mut policy = validated_policy();
        let blocked = policy.preview("job-1", 1).unwrap();
        assert!(!blocked.gates_clear);
        assert!(!policy.machine_ready());
        assert_eq!(
            policy.authorize("job-1", 1),
            Err(ReleasePolicyError::SafePreviewRequired)
        );

        clear_every_gate(&mut policy);
        let eligible = policy.preview("job-1", 1).unwrap();
        assert!(eligible.gates_clear);
        assert!(!policy.machine_ready());
    }

    #[test]
    fn formal_trace_revision_invalidates_release_and_rejects_stale_evidence() {
        let mut policy = validated_policy();
        clear_every_gate(&mut policy);
        policy.preview("job-1", 1).unwrap();
        policy.authorize("job-1", 1).unwrap();
        assert!(policy.machine_ready());

        policy.revise("job-1", 1, 2).unwrap();
        assert!(!policy.machine_ready());
        assert_eq!(policy.blocked_gates(), RELEASE_GATES);
        assert_eq!(
            policy.record_evidence("job-1", 1, ReleaseGate::SourceProvenance),
            Err(ReleasePolicyError::RevisionMismatch {
                expected: 2,
                actual: 1,
            })
        );
        assert_eq!(policy.blocked_gates(), RELEASE_GATES);
    }

    #[test]
    fn authorization_is_idempotent_and_authorized_revision_is_immutable() {
        let mut policy = validated_policy();
        clear_every_gate(&mut policy);
        policy.preview("job-1", 1).unwrap();
        let first = policy.authorize("job-1", 1).unwrap();
        let retry = policy.authorize("job-1", 1).unwrap();
        assert_eq!(first, retry);
        assert_eq!(
            policy.reopen_gate("job-1", 1, ReleaseGate::SimulationAndQuality),
            Err(ReleasePolicyError::AuthorizedRevisionImmutable)
        );
        assert!(policy.machine_ready());
    }

    #[test]
    fn evidence_from_another_job_cannot_clear_a_gate() {
        let mut policy = validated_policy();
        assert_eq!(
            policy.record_evidence("job-2", 1, ReleaseGate::HumanOrAutomationHandoff,),
            Err(ReleasePolicyError::JobMismatch {
                expected: "job-1".to_owned(),
                actual: "job-2".to_owned(),
            })
        );
        assert_eq!(policy.blocked_gates(), RELEASE_GATES);
    }
}
