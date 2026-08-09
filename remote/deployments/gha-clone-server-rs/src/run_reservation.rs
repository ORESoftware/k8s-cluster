use std::collections::{BTreeMap, BTreeSet};

use uuid::Uuid;

use super::{RunRecord, RunStatus};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ReserveRunsError {
    Capacity {
        max_runs: usize,
        active_runs: usize,
        requested: usize,
    },
    DuplicateId(Uuid),
}

pub(crate) fn reserve_run_records(
    runs: &mut BTreeMap<Uuid, RunRecord>,
    records: Vec<RunRecord>,
    max_runs: usize,
) -> Result<(), ReserveRunsError> {
    let requested = records.len();
    let active_runs = runs
        .values()
        .filter(|run| !is_terminal(&run.status))
        .count();
    if max_runs == 0 || active_runs.saturating_add(requested) > max_runs {
        return Err(ReserveRunsError::Capacity {
            max_runs,
            active_runs,
            requested,
        });
    }

    let mut incoming_ids = BTreeSet::new();
    for record in &records {
        if runs.contains_key(&record.id) || !incoming_ids.insert(record.id) {
            return Err(ReserveRunsError::DuplicateId(record.id));
        }
    }

    let remove = runs
        .len()
        .saturating_add(requested)
        .saturating_sub(max_runs);
    let mut terminal = runs
        .values()
        .filter(|run| is_terminal(&run.status))
        .map(|run| (run.updated_at_ms, run.id))
        .collect::<Vec<_>>();
    terminal.sort_unstable();
    debug_assert!(terminal.len() >= remove);

    for (_, id) in terminal.into_iter().take(remove) {
        runs.remove(&id);
    }
    for record in records {
        runs.insert(record.id, record);
    }
    Ok(())
}

fn is_terminal(status: &RunStatus) -> bool {
    matches!(status, RunStatus::Succeeded | RunStatus::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u128, status: RunStatus, updated_at_ms: u128) -> RunRecord {
        RunRecord {
            id: Uuid::from_u128(id),
            plan_id: format!("plan-{id}"),
            repository: "owner/repo".into(),
            revision: "0123456789abcdef0123456789abcdef01234567".into(),
            workflow_path: ".github/workflows/ci.yml".into(),
            status,
            current_job: None,
            submissions: Vec::new(),
            error: None,
            created_at_ms: updated_at_ms,
            updated_at_ms,
        }
    }

    fn ids(runs: &BTreeMap<Uuid, RunRecord>) -> BTreeSet<Uuid> {
        runs.keys().copied().collect()
    }

    #[test]
    fn reserves_a_batch_atomically_after_evicting_oldest_terminal_runs() {
        let active = record(10, RunStatus::Running, 30);
        let old_terminal = record(11, RunStatus::Succeeded, 10);
        let newer_terminal = record(12, RunStatus::Failed, 20);
        let first = record(20, RunStatus::Queued, 40);
        let second = record(21, RunStatus::Queued, 40);
        let mut runs = BTreeMap::from([
            (active.id, active.clone()),
            (old_terminal.id, old_terminal.clone()),
            (newer_terminal.id, newer_terminal.clone()),
        ]);

        reserve_run_records(&mut runs, vec![first.clone(), second.clone()], 3)
            .expect("terminal runs make enough capacity");

        assert_eq!(ids(&runs), BTreeSet::from([active.id, first.id, second.id]));
    }

    #[test]
    fn capacity_failure_does_not_evict_or_partially_insert() {
        let first_active = record(1, RunStatus::Running, 1);
        let second_active = record(2, RunStatus::Queued, 2);
        let incoming = record(3, RunStatus::Queued, 3);
        let mut runs = BTreeMap::from([
            (first_active.id, first_active.clone()),
            (second_active.id, second_active.clone()),
        ]);
        let before = ids(&runs);

        assert_eq!(
            reserve_run_records(&mut runs, vec![incoming.clone()], 2),
            Err(ReserveRunsError::Capacity {
                max_runs: 2,
                active_runs: 2,
                requested: 1,
            })
        );
        assert_eq!(ids(&runs), before);
        assert!(!runs.contains_key(&incoming.id));
    }

    #[test]
    fn oversized_batch_and_zero_capacity_fail_without_mutation() {
        let terminal = record(1, RunStatus::Succeeded, 1);
        let mut runs = BTreeMap::from([(terminal.id, terminal.clone())]);
        let before = ids(&runs);

        assert!(matches!(
            reserve_run_records(
                &mut runs,
                vec![
                    record(2, RunStatus::Queued, 2),
                    record(3, RunStatus::Queued, 3),
                ],
                1,
            ),
            Err(ReserveRunsError::Capacity { .. })
        ));
        assert_eq!(ids(&runs), before);
        assert!(matches!(
            reserve_run_records(&mut runs, Vec::new(), 0),
            Err(ReserveRunsError::Capacity { .. })
        ));
        assert_eq!(ids(&runs), before);
    }

    #[test]
    fn duplicate_existing_or_incoming_ids_fail_without_mutation() {
        let existing = record(1, RunStatus::Succeeded, 1);
        let mut runs = BTreeMap::from([(existing.id, existing.clone())]);
        let before = ids(&runs);

        assert_eq!(
            reserve_run_records(&mut runs, vec![existing.clone()], 2),
            Err(ReserveRunsError::DuplicateId(existing.id))
        );
        assert_eq!(ids(&runs), before);

        let duplicate = record(2, RunStatus::Queued, 2);
        assert_eq!(
            reserve_run_records(&mut runs, vec![duplicate.clone(), duplicate.clone()], 3),
            Err(ReserveRunsError::DuplicateId(duplicate.id))
        );
        assert_eq!(ids(&runs), before);
    }

    #[test]
    fn equal_age_terminal_eviction_is_deterministic_by_id() {
        let lower_id = record(1, RunStatus::Succeeded, 5);
        let higher_id = record(2, RunStatus::Succeeded, 5);
        let incoming = record(3, RunStatus::Queued, 6);
        let mut runs = BTreeMap::from([
            (lower_id.id, lower_id.clone()),
            (higher_id.id, higher_id.clone()),
        ]);

        reserve_run_records(&mut runs, vec![incoming.clone()], 2).unwrap();
        assert!(!runs.contains_key(&lower_id.id));
        assert!(runs.contains_key(&higher_id.id));
        assert!(runs.contains_key(&incoming.id));
    }
}
