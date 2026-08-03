#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Strength {
    Strong,
    Weak,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Caller {
    Service,
    Member(Strength),
    NonMember(Strength),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    None,
    Named,
    Invalid,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Read,
    Write,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Policy {
    pub users_only: bool,
    pub strong_write: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Allow,
    Deny,
}

pub fn decide(caller: Caller, scope: Scope, operation: Operation, policy: Policy) -> Decision {
    match scope {
        Scope::Invalid => return Decision::Deny,
        Scope::Named => match caller {
            Caller::NonMember(_) => return Decision::Deny,
            Caller::Service if policy.users_only => return Decision::Deny,
            Caller::Service | Caller::Member(_) => {}
        },
        Scope::None => {}
    }
    if operation == Operation::Read || !policy.strong_write {
        return Decision::Allow;
    }
    match caller {
        Caller::Service if scope == Scope::Named => Decision::Deny,
        Caller::Service => Decision::Allow,
        Caller::Member(Strength::Strong) => Decision::Allow,
        Caller::Member(Strength::Weak) | Caller::NonMember(_) => Decision::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALLERS: [Caller; 5] = [
        Caller::Service,
        Caller::Member(Strength::Strong),
        Caller::Member(Strength::Weak),
        Caller::NonMember(Strength::Strong),
        Caller::NonMember(Strength::Weak),
    ];
    const SCOPES: [Scope; 3] = [Scope::None, Scope::Named, Scope::Invalid];
    const OPERATIONS: [Operation; 2] = [Operation::Read, Operation::Write];
    const BOOLS: [bool; 2] = [false, true];

    #[test]
    fn shared_service_credential_cannot_write_named_scope_when_hardened() {
        for users_only in BOOLS {
            assert_eq!(
                decide(
                    Caller::Service,
                    Scope::Named,
                    Operation::Write,
                    Policy {
                        users_only,
                        strong_write: true,
                    },
                ),
                Decision::Deny,
            );
        }
    }

    #[test]
    fn only_strong_members_write_named_scope_under_full_policy() {
        let policy = Policy {
            users_only: true,
            strong_write: true,
        };
        for caller in CALLERS {
            let expected = if caller == Caller::Member(Strength::Strong) {
                Decision::Allow
            } else {
                Decision::Deny
            };
            assert_eq!(
                decide(caller, Scope::Named, Operation::Write, policy),
                expected
            );
        }
    }

    #[test]
    fn invalid_scope_always_denies() {
        for caller in CALLERS {
            for operation in OPERATIONS {
                for users_only in BOOLS {
                    for strong_write in BOOLS {
                        assert_eq!(
                            decide(
                                caller,
                                Scope::Invalid,
                                operation,
                                Policy {
                                    users_only,
                                    strong_write,
                                },
                            ),
                            Decision::Deny,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn all_states_are_deterministic() {
        let mut visited = 0;
        for caller in CALLERS {
            for scope in SCOPES {
                for operation in OPERATIONS {
                    for users_only in BOOLS {
                        for strong_write in BOOLS {
                            let policy = Policy {
                                users_only,
                                strong_write,
                            };
                            assert_eq!(
                                decide(caller, scope, operation, policy),
                                decide(caller, scope, operation, policy)
                            );
                            visited += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(visited, 5 * 3 * 2 * 2 * 2);
    }
}
