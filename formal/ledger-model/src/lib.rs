#![cfg(test)]

//! Bounded explicit-state model for ledger posting.
//!
//! The production service performs validation before opening a database
//! transaction, serializes same-key calls with a Postgres advisory lock, stages
//! the transaction and postings in one database transaction, commits, releases
//! the lock, and only then publishes a best-effort event. This model makes those
//! ordering assumptions executable.

use stateright::{Model, Property};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Caller {
    A,
    B,
}

impl Caller {
    const ALL: [Self; 2] = [Self::A, Self::B];

    const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum DraftKind {
    Balanced,
    Unbalanced,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Phase {
    Ready,
    Validated,
    Rejected,
    Locked,
    TransactionStaged,
    DebitStaged,
    BalancedStaged,
    NeedsEvent,
    ReturnedNew,
    ReturnedExisting,
    Aborted,
}

impl Phase {
    const fn holds_lock(self) -> bool {
        matches!(
            self,
            Self::Locked | Self::TransactionStaged | Self::DebitStaged | Self::BalancedStaged
        )
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct Attempt {
    draft: DraftKind,
    phase: Phase,
    result_owner: Option<Caller>,
}

impl Attempt {
    const fn new(draft: DraftKind) -> Self {
        Self {
            draft,
            phase: Phase::Ready,
            result_owner: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct CommittedTransaction {
    owner: Caller,
    posting_count: u8,
    net_minor: i8,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct LedgerState {
    lock_owner: Option<Caller>,
    attempts: [Attempt; 2],
    committed: Option<CommittedTransaction>,
    visible_posting_count: u8,
    event_published: bool,
}

impl LedgerState {
    fn initial(a: DraftKind, b: DraftKind) -> Self {
        Self {
            lock_owner: None,
            attempts: [Attempt::new(a), Attempt::new(b)],
            committed: None,
            visible_posting_count: 0,
            event_published: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Action {
    Validate(Caller),
    AcquireLock(Caller),
    InspectIdempotencyKey(Caller),
    InsertDebit(Caller),
    InsertCredit(Caller),
    Commit(Caller),
    Abort(Caller),
    PublishEvent(Caller),
    CrashAfterCommit(Caller),
}

#[derive(Clone, Debug)]
struct LedgerModel;

impl Model for LedgerModel {
    type State = LedgerState;
    type Action = Action;

    fn init_states(&self) -> Vec<Self::State> {
        use DraftKind::{Balanced, Unbalanced};
        vec![
            LedgerState::initial(Balanced, Balanced),
            LedgerState::initial(Balanced, Unbalanced),
            LedgerState::initial(Unbalanced, Balanced),
            LedgerState::initial(Unbalanced, Unbalanced),
        ]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        for caller in Caller::ALL {
            let attempt = state.attempts[caller.index()];
            match attempt.phase {
                Phase::Ready => actions.push(Action::Validate(caller)),
                Phase::Validated if state.lock_owner.is_none() => {
                    actions.push(Action::AcquireLock(caller));
                }
                Phase::Locked => {
                    actions.push(Action::InspectIdempotencyKey(caller));
                    actions.push(Action::Abort(caller));
                }
                Phase::TransactionStaged => {
                    actions.push(Action::InsertDebit(caller));
                    actions.push(Action::Abort(caller));
                }
                Phase::DebitStaged => {
                    actions.push(Action::InsertCredit(caller));
                    actions.push(Action::Abort(caller));
                }
                Phase::BalancedStaged => {
                    actions.push(Action::Commit(caller));
                    actions.push(Action::Abort(caller));
                }
                Phase::NeedsEvent => {
                    if !state.event_published {
                        actions.push(Action::PublishEvent(caller));
                    }
                    actions.push(Action::CrashAfterCommit(caller));
                }
                Phase::Rejected | Phase::ReturnedNew | Phase::ReturnedExisting | Phase::Aborted => {
                }
                Phase::Validated => {}
            }
        }
    }

    fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
        let mut next = state.clone();
        match action {
            Action::Validate(caller) => {
                let attempt = &mut next.attempts[caller.index()];
                attempt.phase = match attempt.draft {
                    DraftKind::Balanced => Phase::Validated,
                    DraftKind::Unbalanced => Phase::Rejected,
                };
            }
            Action::AcquireLock(caller) => {
                next.lock_owner = Some(caller);
                next.attempts[caller.index()].phase = Phase::Locked;
            }
            Action::InspectIdempotencyKey(caller) => {
                let attempt = &mut next.attempts[caller.index()];
                if let Some(committed) = next.committed {
                    attempt.phase = Phase::ReturnedExisting;
                    attempt.result_owner = Some(committed.owner);
                    next.lock_owner = None;
                } else {
                    attempt.phase = Phase::TransactionStaged;
                }
            }
            Action::InsertDebit(caller) => {
                next.attempts[caller.index()].phase = Phase::DebitStaged;
            }
            Action::InsertCredit(caller) => {
                next.attempts[caller.index()].phase = Phase::BalancedStaged;
            }
            Action::Commit(caller) => {
                next.committed = Some(CommittedTransaction {
                    owner: caller,
                    posting_count: 2,
                    net_minor: 0,
                });
                next.visible_posting_count = 2;
                next.lock_owner = None;
                let attempt = &mut next.attempts[caller.index()];
                attempt.phase = Phase::NeedsEvent;
                attempt.result_owner = Some(caller);
            }
            Action::Abort(caller) => {
                next.attempts[caller.index()].phase = Phase::Aborted;
                next.lock_owner = None;
            }
            Action::PublishEvent(caller) => {
                next.event_published = true;
                next.attempts[caller.index()].phase = Phase::ReturnedNew;
            }
            Action::CrashAfterCommit(caller) => {
                // Event publication is explicitly best-effort in production.
                // A post-commit crash may lose it, but may not duplicate the
                // transaction or expose partial postings.
                next.attempts[caller.index()].phase = Phase::ReturnedNew;
            }
        }
        Some(next)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::<Self>::always("committed transactions are balanced", |_, state| {
                state.committed.is_none_or(|transaction| {
                    transaction.posting_count >= 2 && transaction.net_minor == 0
                })
            }),
            Property::<Self>::always("postings become visible atomically", |_, state| match state
                .committed
            {
                None => state.visible_posting_count == 0,
                Some(transaction) => state.visible_posting_count == transaction.posting_count,
            }),
            Property::<Self>::always("unbalanced drafts never enter the database", |_, state| {
                state.attempts.iter().all(|attempt| {
                    attempt.draft == DraftKind::Balanced
                        || matches!(attempt.phase, Phase::Ready | Phase::Rejected)
                })
            }),
            Property::<Self>::always("the advisory lock is exclusive", |_, state| {
                let holders = Caller::ALL
                    .into_iter()
                    .filter(|caller| state.attempts[caller.index()].phase.holds_lock())
                    .collect::<Vec<_>>();
                match (state.lock_owner, holders.as_slice()) {
                    (None, []) => true,
                    (Some(owner), [holder]) => owner == *holder,
                    _ => false,
                }
            }),
            Property::<Self>::always("replays return the committed identity", |_, state| {
                state.attempts.iter().all(|attempt| {
                    if attempt.phase == Phase::ReturnedExisting {
                        attempt.result_owner == state.committed.map(|transaction| transaction.owner)
                    } else {
                        true
                    }
                })
            }),
            Property::<Self>::always("events are post-commit and winner-only", |_, state| {
                if !state.event_published {
                    return true;
                }
                let Some(transaction) = state.committed else {
                    return false;
                };
                state.attempts[transaction.owner.index()].phase == Phase::ReturnedNew
            }),
            Property::<Self>::sometimes("a balanced draft can commit", |_, state| {
                state.committed.is_some()
            }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stateright::Checker;

    #[test]
    fn bounded_posting_model_satisfies_all_properties() {
        let checker = LedgerModel.checker().threads(1).spawn_dfs().join();
        eprintln!(
            "ledger model explored {} states ({} unique)",
            checker.state_count(),
            checker.unique_state_count()
        );
        checker.assert_properties();
    }
}
