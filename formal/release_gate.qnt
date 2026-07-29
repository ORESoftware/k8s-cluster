// Executable finite model of the machine-release policy.
//
// Five gate IDs represent source provenance, machine envelope, process
// readiness, simulation/quality evidence, and human-or-automation handoff.
// Two revisions are enough to explore stale evidence and release invalidation.
module release_gate {
  pure val NO_REVISION = 0
  pure val REVISIONS = Set(1, 2)
  pure val GATES = Set(1, 2, 3, 4, 5)

  type State = {
    revision: int,
    validated_revision: int,
    evidence_revision: int -> int,
    blockers: Set[int],
    preview_revision: int,
    preview_safe: bool,
    authorized_revision: int,
    authorization_count: int,
    saw_stale_evidence_rejection: bool,
    saw_foreign_job_rejection: bool,
    saw_revision_invalidation: bool,
    saw_preview_without_authorization: bool,
    saw_blocked_authorization: bool,
    saw_duplicate_authorization: bool,
  }

  var s: State

  def all_current_evidence =
    GATES.forall(gate => s.evidence_revision.get(gate) == s.revision)

  def release_preconditions = and {
    s.validated_revision == s.revision,
    s.blockers == Set(),
    all_current_evidence,
  }

  def machine_ready = s.authorized_revision == s.revision

  action init = {
    s' = {
      revision: 1,
      validated_revision: NO_REVISION,
      evidence_revision: GATES.mapBy(_ => NO_REVISION),
      blockers: GATES,
      preview_revision: NO_REVISION,
      preview_safe: false,
      authorized_revision: NO_REVISION,
      authorization_count: 0,
      saw_stale_evidence_rejection: false,
      saw_foreign_job_rejection: false,
      saw_revision_invalidation: false,
      saw_preview_without_authorization: false,
      saw_blocked_authorization: false,
      saw_duplicate_authorization: false,
    }
  }

  // A second initializer supports a focused, deterministic replay of the
  // authorize-then-revise counterexample boundary. The main exhaustive model
  // still starts from `init` and reaches this state through ordinary actions.
  action init_authorized = {
    s' = {
      revision: 1,
      validated_revision: 1,
      evidence_revision: GATES.mapBy(_ => 1),
      blockers: Set(),
      preview_revision: 1,
      preview_safe: true,
      authorized_revision: 1,
      authorization_count: 1,
      saw_stale_evidence_rejection: false,
      saw_foreign_job_rejection: false,
      saw_revision_invalidation: false,
      saw_preview_without_authorization: true,
      saw_blocked_authorization: false,
      saw_duplicate_authorization: false,
    }
  }

  action validate_current_revision = all {
    s.validated_revision != s.revision,
    s' = {
      ...s,
      validated_revision: s.revision,
      preview_revision: NO_REVISION,
      preview_safe: false,
      authorized_revision: NO_REVISION,
    },
  }

  // Valid evidence is tied to the current job and revision and is accepted only
  // after validation. Recording evidence invalidates any older preview.
  action record_current_evidence(gate) = all {
    GATES.contains(gate),
    s.validated_revision == s.revision,
    s.blockers.contains(gate),
    s' = {
      ...s,
      evidence_revision: s.evidence_revision.set(gate, s.revision),
      blockers: s.blockers.filter(blocker => blocker != gate),
      preview_revision: NO_REVISION,
      preview_safe: false,
      authorized_revision: NO_REVISION,
    },
  }

  // Duplicate evidence is idempotent and cannot create another authorization.
  action record_duplicate_evidence(gate) = all {
    GATES.contains(gate),
    s.evidence_revision.get(gate) == s.revision,
    not(s.blockers.contains(gate)),
    s' = s,
  }

  // Evidence naming a different revision or job is rejected without touching
  // any gate, preview, or authorization state.
  action reject_stale_evidence(evidence_revision) = all {
    REVISIONS.contains(evidence_revision),
    evidence_revision != s.revision,
    s' = { ...s, saw_stale_evidence_rejection: true },
  }

  action reject_foreign_job_evidence = {
    s' = { ...s, saw_foreign_job_rejection: true }
  }

  // An authorized revision is immutable. Before authorization, a failed gate
  // invalidates its evidence and preview; after authorization, the caller must
  // create a new revision before recording changed evidence.
  action reopen_gate(gate) = all {
    GATES.contains(gate),
    not(s.blockers.contains(gate)),
    not(machine_ready),
    s' = {
      ...s,
      evidence_revision:
        s.evidence_revision.set(gate, NO_REVISION),
      blockers: s.blockers.union(Set(gate)),
      preview_revision: NO_REVISION,
      preview_safe: false,
      authorized_revision: NO_REVISION,
    },
  }

  // A preview is a snapshot of readiness evidence. It deliberately does not
  // authorize machine execution, whether the snapshot is safe or blocked.
  action create_preview = {
    s' = {
      ...s,
      preview_revision: s.revision,
      preview_safe: release_preconditions,
      saw_preview_without_authorization:
        s.saw_preview_without_authorization or not(machine_ready),
    }
  }

  action reject_blocked_authorization = all {
    or {
      s.preview_revision != s.revision,
      not(s.preview_safe),
      not(release_preconditions),
    },
    s' = { ...s, saw_blocked_authorization: true },
  }

  action authorize_current_revision = all {
    s.preview_revision == s.revision,
    s.preview_safe,
    release_preconditions,
    s.authorized_revision != s.revision,
    s' = {
      ...s,
      authorized_revision: s.revision,
      authorization_count: s.authorization_count + 1,
    },
  }

  // Retrying the exact authorization is one logical authorization.
  action retry_authorization = all {
    machine_ready,
    s' = { ...s, saw_duplicate_authorization: true },
  }

  // A plan revision invalidates all validation, evidence, previews, and current
  // authorization. Old evidence cannot become valid again merely because it
  // arrives after the revision change.
  action revise_plan = all {
    s.revision == 1,
    s' = {
      ...s,
      revision: 2,
      validated_revision: NO_REVISION,
      evidence_revision: GATES.mapBy(_ => NO_REVISION),
      blockers: GATES,
      preview_revision: NO_REVISION,
      preview_safe: false,
      authorized_revision: NO_REVISION,
      saw_revision_invalidation:
        s.saw_revision_invalidation or machine_ready,
    },
  }

  action revision_step = revise_plan

  action idle = s' = s

  action step = {
    nondet gate = GATES.oneOf()
    nondet evidence_revision = REVISIONS.oneOf()
    any {
      validate_current_revision,
      record_current_evidence(gate),
      record_duplicate_evidence(gate),
      reject_stale_evidence(evidence_revision),
      reject_foreign_job_evidence,
      reopen_gate(gate),
      create_preview,
      reject_blocked_authorization,
      authorize_current_revision,
      retry_authorization,
      revise_plan,
      idle,
    }
  }

  val revisions_are_bounded = and {
    REVISIONS.contains(s.revision),
    or {
      s.validated_revision == NO_REVISION,
      s.validated_revision == s.revision,
    },
    or {
      s.preview_revision == NO_REVISION,
      s.preview_revision == s.revision,
    },
    or {
      s.authorized_revision == NO_REVISION,
      s.authorized_revision == s.revision,
    },
  }

  val evidence_is_current_or_absent =
    GATES.forall(gate => or {
      s.evidence_revision.get(gate) == NO_REVISION,
      s.evidence_revision.get(gate) == s.revision,
    })

  val blockers_match_evidence =
    GATES.forall(gate => or {
      s.blockers.contains(gate) and
        s.evidence_revision.get(gate) == NO_REVISION,
      not(s.blockers.contains(gate)) and
        s.evidence_revision.get(gate) == s.revision,
    })

  val safe_preview_matches_current_evidence =
    not(s.preview_safe) or and {
      s.preview_revision == s.revision,
      release_preconditions,
    }

  val machine_ready_requires_every_gate = not(machine_ready) or and {
    release_preconditions,
    s.preview_revision == s.revision,
    s.preview_safe,
  }

  val authorization_is_idempotent = and {
    s.authorization_count >= 0,
    s.authorization_count <= s.revision,
  }

  val release_safety = and {
    revisions_are_bounded,
    evidence_is_current_or_absent,
    blockers_match_evidence,
    safe_preview_matches_current_evidence,
    machine_ready_requires_every_gate,
    authorization_is_idempotent,
  }

  // CI checks these witnesses so the model must reach both rejection paths and
  // a complete release rather than proving safety by disabling progress.
  val stale_evidence_rejection_reached =
    s.saw_stale_evidence_rejection and s.saw_foreign_job_rejection

  val preview_without_authorization_reached =
    s.saw_preview_without_authorization

  val blocked_authorization_reached = s.saw_blocked_authorization

  val safe_release_reached = machine_ready

  val revision_invalidation_reached =
    s.saw_revision_invalidation and not(machine_ready) and s.revision == 2

  val duplicate_authorization_reached =
    s.saw_duplicate_authorization and machine_ready
}
