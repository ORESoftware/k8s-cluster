#![cfg(test)]

//! Bounded explicit-state model for ledger posting.
//!
//! The production service performs validation before opening a database
//! transaction, serializes same-key calls with a Postgres advisory lock, stages
//! the transaction and postings in one database transaction, commits, releases
//! the lock, and only then publishes a best-effort event. This model makes those
//! ordering assumptions executable.

use stateright::{Model, Property};

// Compile and execute the production canonicalization vectors in the
// standalone formal crate, which is the repository's buildable CI boundary.
#[path = "../../../src/ledger/fingerprint.rs"]
mod production_fingerprint;

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
enum Intent {
    Primary,
    Alternate,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum StoredIntent {
    Legacy,
    Versioned(Intent),
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
    RejectedConflict,
    RejectedLegacy,
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
    intent: Intent,
    phase: Phase,
    result_owner: Option<Caller>,
}

impl Attempt {
    const fn new(draft: DraftKind, intent: Intent) -> Self {
        Self {
            draft,
            intent,
            phase: Phase::Ready,
            result_owner: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct CommittedTransaction {
    owner: Caller,
    stored_intent: StoredIntent,
    posting_count: u8,
    net_minor: i8,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct LedgerState {
    lock_owner: Option<Caller>,
    attempts: [Attempt; 2],
    committed: Option<CommittedTransaction>,
    visible_posting_count: u8,
    event_publisher: Option<Caller>,
}

impl LedgerState {
    fn fresh(a: DraftKind, a_intent: Intent, b: DraftKind, b_intent: Intent) -> Self {
        Self {
            lock_owner: None,
            attempts: [Attempt::new(a, a_intent), Attempt::new(b, b_intent)],
            committed: None,
            visible_posting_count: 0,
            event_publisher: None,
        }
    }

    fn legacy(a_intent: Intent, b_intent: Intent) -> Self {
        Self {
            lock_owner: None,
            attempts: [
                Attempt::new(DraftKind::Balanced, a_intent),
                Attempt::new(DraftKind::Balanced, b_intent),
            ],
            committed: Some(CommittedTransaction {
                owner: Caller::A,
                stored_intent: StoredIntent::Legacy,
                posting_count: 2,
                net_minor: 0,
            }),
            visible_posting_count: 2,
            event_publisher: None,
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
        use Intent::{Alternate, Primary};

        let mut states = Vec::new();
        for a_draft in [Balanced, Unbalanced] {
            for b_draft in [Balanced, Unbalanced] {
                for a_intent in [Primary, Alternate] {
                    for b_intent in [Primary, Alternate] {
                        states.push(LedgerState::fresh(a_draft, a_intent, b_draft, b_intent));
                    }
                }
            }
        }

        for a_intent in [Primary, Alternate] {
            for b_intent in [Primary, Alternate] {
                states.push(LedgerState::legacy(a_intent, b_intent));
            }
        }
        states
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
                    if state.event_publisher.is_none() {
                        actions.push(Action::PublishEvent(caller));
                    }
                    actions.push(Action::CrashAfterCommit(caller));
                }
                Phase::Rejected
                | Phase::ReturnedNew
                | Phase::ReturnedExisting
                | Phase::RejectedConflict
                | Phase::RejectedLegacy
                | Phase::Aborted => {}
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
                    match committed.stored_intent {
                        StoredIntent::Legacy => {
                            attempt.phase = Phase::RejectedLegacy;
                        }
                        StoredIntent::Versioned(intent) if intent == attempt.intent => {
                            attempt.phase = Phase::ReturnedExisting;
                            attempt.result_owner = Some(committed.owner);
                        }
                        StoredIntent::Versioned(_) => {
                            attempt.phase = Phase::RejectedConflict;
                        }
                    }
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
                let intent = next.attempts[caller.index()].intent;
                next.committed = Some(CommittedTransaction {
                    owner: caller,
                    stored_intent: StoredIntent::Versioned(intent),
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
                next.event_publisher = Some(caller);
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
                        state.committed.is_some_and(|transaction| {
                            transaction.stored_intent == StoredIntent::Versioned(attempt.intent)
                                && attempt.result_owner == Some(transaction.owner)
                        })
                    } else {
                        true
                    }
                })
            }),
            Property::<Self>::always("only the committed intent can succeed", |_, state| {
                state.attempts.iter().all(|attempt| {
                    if !matches!(
                        attempt.phase,
                        Phase::NeedsEvent | Phase::ReturnedNew | Phase::ReturnedExisting
                    ) {
                        return true;
                    }
                    state.committed.is_some_and(|transaction| {
                        transaction.stored_intent == StoredIntent::Versioned(attempt.intent)
                    })
                })
            }),
            Property::<Self>::always("different-intent replays fail closed", |_, state| {
                state.attempts.iter().enumerate().all(|(index, attempt)| {
                    if attempt.phase != Phase::RejectedConflict {
                        return true;
                    }
                    state.committed.is_some_and(|transaction| {
                        matches!(
                            transaction.stored_intent,
                            StoredIntent::Versioned(intent) if intent != attempt.intent
                        ) && attempt.result_owner.is_none()
                            && state.lock_owner != Some(Caller::ALL[index])
                    })
                })
            }),
            Property::<Self>::always("legacy replays fail closed", |_, state| {
                state.attempts.iter().enumerate().all(|(index, attempt)| {
                    if attempt.phase != Phase::RejectedLegacy {
                        return true;
                    }
                    state.committed.is_some_and(|transaction| {
                        transaction.stored_intent == StoredIntent::Legacy
                            && attempt.result_owner.is_none()
                            && state.lock_owner != Some(Caller::ALL[index])
                    })
                })
            }),
            Property::<Self>::always(
                "rejected replays cannot publish or mutate committed intent",
                |_, state| {
                    state.attempts.iter().enumerate().all(|(index, attempt)| {
                        !matches!(
                            attempt.phase,
                            Phase::RejectedConflict | Phase::RejectedLegacy
                        ) || (state.event_publisher != Some(Caller::ALL[index])
                            && !attempt.phase.holds_lock())
                    })
                },
            ),
            Property::<Self>::always("events are post-commit and winner-only", |_, state| {
                let Some(publisher) = state.event_publisher else {
                    return true;
                };
                let Some(transaction) = state.committed else {
                    return false;
                };
                publisher == transaction.owner
                    && state.attempts[transaction.owner.index()].phase == Phase::ReturnedNew
            }),
            Property::<Self>::sometimes("a balanced draft can commit", |_, state| {
                state.committed.is_some_and(|transaction| {
                    matches!(transaction.stored_intent, StoredIntent::Versioned(_))
                })
            }),
            Property::<Self>::sometimes("an identical intent can replay", |_, state| {
                state
                    .attempts
                    .iter()
                    .any(|attempt| attempt.phase == Phase::ReturnedExisting)
            }),
            Property::<Self>::sometimes("a different intent is rejected", |_, state| {
                state
                    .attempts
                    .iter()
                    .any(|attempt| attempt.phase == Phase::RejectedConflict)
            }),
            Property::<Self>::sometimes("a legacy replay is rejected", |_, state| {
                state
                    .attempts
                    .iter()
                    .any(|attempt| attempt.phase == Phase::RejectedLegacy)
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
