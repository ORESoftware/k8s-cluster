"""Dependency-light Rust bootstrap source templates."""
from __future__ import annotations
import textwrap
from coliving_repository_specs import RepoSpec

def rust_public_core() -> str:
    return textwrap.dedent(
        """\
        //! Public, dependency-light H/HAUS resident-operation primitives.

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum PersistenceTier {
            LocalStorage,
            IndexedDb,
            Supabase,
            NeonPostgres,
        }

        impl PersistenceTier {
            pub const fn is_canonical(self) -> bool {
                matches!(self, Self::NeonPostgres)
            }
        }

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct Money {
            minor_units: u64,
            currency: [u8; 3],
        }

        impl Money {
            pub fn new(minor_units: u64, currency: &str) -> Result<Self, &'static str> {
                let bytes = currency.as_bytes();
                if bytes.len() != 3 || !bytes.iter().all(u8::is_ascii_uppercase) {
                    return Err("currency must be three uppercase ASCII letters");
                }
                Ok(Self { minor_units, currency: [bytes[0], bytes[1], bytes[2]] })
            }

            pub const fn minor_units(self) -> u64 { self.minor_units }

            pub fn validate_refund(self, refund_minor_units: u64) -> Result<(), &'static str> {
                if refund_minor_units > self.minor_units {
                    return Err("refund cannot exceed the settled amount");
                }
                Ok(())
            }
        }

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum ActorRole { Resident, Guest, HouseManager, Owner, Administrator, Developer }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn only_server_postgres_receipts_are_canonical() {
                assert!(PersistenceTier::NeonPostgres.is_canonical());
                assert!(!PersistenceTier::LocalStorage.is_canonical());
                assert!(!PersistenceTier::IndexedDb.is_canonical());
                assert!(!PersistenceTier::Supabase.is_canonical());
            }

            #[test]
            fn money_is_integer_and_refunds_are_bounded() {
                let money = Money::new(85_000, "USD").unwrap();
                assert_eq!(money.minor_units(), 85_000);
                assert!(money.validate_refund(85_000).is_ok());
                assert!(money.validate_refund(85_001).is_err());
                assert!(Money::new(1, "usd").is_err());
            }
        }
        """
    )


def rust_integrations() -> str:
    return textwrap.dedent(
        """\
        //! Provider-neutral integration boundaries for resident operations.

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum ProviderDomain { Payments, Authentication, Chat, Sync, AssignmentSolver, Telemetry }

        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct IntegrationContext {
            pub tenant_id: String,
            pub subject: String,
            pub idempotency_key: String,
            pub trace_id: Option<String>,
        }

        impl IntegrationContext {
            pub fn validate(&self) -> Result<(), &'static str> {
                if self.tenant_id.trim().is_empty() { return Err("tenant_id is required"); }
                if self.subject.trim().is_empty() { return Err("subject is required"); }
                if self.idempotency_key.len() < 16 { return Err("idempotency_key is too short"); }
                Ok(())
            }
        }

        pub trait IntegrationAdapter {
            fn domain(&self) -> ProviderDomain;
            fn health(&self) -> Result<(), &'static str>;
        }

        pub fn solver_field_allowed(field: &str) -> bool {
            let lower = field.to_ascii_lowercase();
            !["name", "email", "phone", "contact", "payment", "stripe", "agreement",
              "legal", "chat", "health", "medical", "allergy", "dietary", "document"]
                .iter().any(|forbidden| lower.contains(forbidden))
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn context_requires_tenant_subject_and_stable_idempotency() {
                let context = IntegrationContext {
                    tenant_id: "tenant-1".into(), subject: "shared-auth|resident-1".into(),
                    idempotency_key: "invoice:one:attempt:1".into(), trace_id: None,
                };
                assert!(context.validate().is_ok());
            }

            #[test]
            fn solver_boundary_rejects_protected_fields() {
                assert!(solver_field_allowed("candidate_id"));
                assert!(solver_field_allowed("resource_id"));
                assert!(!solver_field_allowed("residentEmail"));
                assert!(!solver_field_allowed("stripeCustomer"));
                assert!(!solver_field_allowed("dietaryNotes"));
            }
        }
        """
    )


def rust_worker() -> str:
    return textwrap.dedent(
        """\
        //! Deterministic background job lifecycle for H/HAUS operations.

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum JobKind { StripeReconcile, AssignmentSolve, AgreementPersist, ChatMembership, SyncReconcile }

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum JobState { Queued, Running, Succeeded, Retryable, DeadLettered }

        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct Job {
            pub idempotency_key: String,
            pub kind: JobKind,
            pub state: JobState,
            pub attempt: u16,
            pub max_attempts: u16,
        }

        impl Job {
            pub fn start(&mut self) -> Result<(), &'static str> {
                if self.state != JobState::Queued && self.state != JobState::Retryable {
                    return Err("job cannot start from current state");
                }
                if self.idempotency_key.len() < 16 { return Err("idempotency key is too short"); }
                if self.attempt >= self.max_attempts { self.state = JobState::DeadLettered; return Err("attempt budget exhausted"); }
                self.attempt += 1;
                self.state = JobState::Running;
                Ok(())
            }

            pub fn fail_retryable(&mut self) {
                self.state = if self.attempt >= self.max_attempts { JobState::DeadLettered } else { JobState::Retryable };
            }

            pub fn succeed(&mut self) -> Result<(), &'static str> {
                if self.state != JobState::Running { return Err("only running jobs may succeed"); }
                self.state = JobState::Succeeded;
                Ok(())
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            fn job() -> Job {
                Job { idempotency_key: "tenant:invoice:attempt:1".into(), kind: JobKind::StripeReconcile,
                      state: JobState::Queued, attempt: 0, max_attempts: 2 }
            }

            #[test]
            fn retries_are_bounded_and_end_in_dead_letter() {
                let mut value = job();
                value.start().unwrap();
                value.fail_retryable();
                value.start().unwrap();
                value.fail_retryable();
                assert_eq!(value.state, JobState::DeadLettered);
            }

            #[test]
            fn duplicate_success_transition_fails_closed() {
                let mut value = job();
                value.start().unwrap();
                value.succeed().unwrap();
                assert!(value.succeed().is_err());
            }
        }
        """
    )


def rust_mcp() -> str:
    return textwrap.dedent(
        """\
        //! Read-only MCP surface. Mutations remain in authenticated application/admin APIs.

        pub const METHODS: &[&str] = &[
            "inventory.list", "contracts.status", "health.summary", "audit.receipts",
            "reservations.summary", "payments.summary", "solver.summary",
        ];

        pub fn method_allowed(method: &str) -> bool { METHODS.contains(&method) }

        pub fn rejects_mutation(method: &str) -> bool {
            ["create", "update", "delete", "refund", "accept", "grant", "revoke", "check_in"]
                .iter().any(|word| method.to_ascii_lowercase().contains(word))
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            #[test]
            fn surface_is_explicitly_read_only() {
                assert!(method_allowed("inventory.list"));
                assert!(!method_allowed("rent.refund"));
                assert!(rejects_mutation("developer.grant"));
                assert!(rejects_mutation("guest.check_in"));
            }
        }
        """
    )


def rust_sidecar() -> str:
    return textwrap.dedent(
        """\
        //! Redacted process-boundary telemetry contract.

        #[derive(Clone, Debug, Eq, PartialEq)]
        pub struct LogEnvelope {
            pub level: &'static str,
            pub event: &'static str,
            pub tenant_hash: Option<String>,
            pub trace_id: Option<String>,
        }

        pub fn field_allowed(field: &str) -> bool {
            let lower = field.to_ascii_lowercase();
            !["token", "secret", "password", "authorization", "agreement_body", "ballot_identity",
              "card", "bank", "webhook_body", "dietary_detail"]
                .iter().any(|forbidden| lower.contains(forbidden))
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            #[test]
            fn sensitive_fields_never_enter_logs() {
                assert!(field_allowed("trace_id"));
                assert!(field_allowed("tenant_hash"));
                assert!(!field_allowed("authorization_token"));
                assert!(!field_allowed("raw_webhook_body"));
                assert!(!field_allowed("ballot_identity"));
            }
        }
        """
    )


def rust_web(spec: RepoSpec) -> str:
    framework = "MASH" if "mash" in spec.name else "Leptos" if "leptos" in spec.name else "Dioxus"
    return textwrap.dedent(
        f"""\
        //! {framework} route and preload policy for H/HAUS.

        pub const FRAMEWORK: &str = "{framework}";

        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum RouteClass {{ Public, Resident, Manager, Owner, Administrator }}

        pub fn route_class(path: &str) -> Option<RouteClass> {{
            match path {{
                "/" | "/pricing" | "/legal" => Some(RouteClass::Public),
                "/app/guests" | "/app/reservations" | "/app/polls" | "/app/rent" => Some(RouteClass::Resident),
                "/admin/house" => Some(RouteClass::Manager),
                "/admin/owner" => Some(RouteClass::Owner),
                "/admin/access" => Some(RouteClass::Administrator),
                _ => None,
            }}
        }}

        pub fn preload_has_side_effect(resource: &str) -> bool {{
            ["payment", "agreement-accept", "vote-submit", "guest-check-in", "admin-grant"]
                .iter().any(|word| resource.contains(word))
        }}

        #[cfg(test)]
        mod tests {{
            use super::*;
            #[test]
            fn privileged_routes_are_not_public() {{
                assert_eq!(route_class("/admin/owner"), Some(RouteClass::Owner));
                assert_ne!(route_class("/admin/access"), Some(RouteClass::Public));
            }}
            #[test]
            fn wasm_loader_prefetch_is_side_effect_free() {{
                assert!(!preload_has_side_effect("ores-wasm-loaders/core.wasm"));
                assert!(preload_has_side_effect("payment/start"));
                assert!(preload_has_side_effect("agreement-accept/v1"));
            }}
        }}
        """
    )


def rust_cli() -> str:
    return textwrap.dedent(
        """\
        use std::env;

        const COMMANDS: &[&str] = &[
            "guest-check-in", "guest-check-out", "reserve", "poll", "rent-collect",
            "refund", "access-audit", "config-validate",
        ];

        fn required_scope(command: &str) -> Option<&'static str> {
            match command {
                "guest-check-in" | "guest-check-out" => Some("guest:write"),
                "reserve" => Some("reservation:write"),
                "poll" => Some("poll:write"),
                "rent-collect" | "refund" => Some("payment:admin"),
                "access-audit" => Some("access:audit"),
                "config-validate" => Some("config:read"),
                _ => None,
            }
        }

        fn main() {
            let command = env::args().nth(1).unwrap_or_else(|| "help".into());
            match required_scope(&command) {
                Some(scope) => println!("command={command} required_scope={scope}"),
                None => {
                    eprintln!("usage: hhaus-cli <{}>", COMMANDS.join("|"));
                    std::process::exit(64);
                }
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            #[test]
            fn money_and_access_commands_require_admin_scopes() {
                assert_eq!(required_scope("refund"), Some("payment:admin"));
                assert_eq!(required_scope("access-audit"), Some("access:audit"));
                assert_eq!(required_scope("unknown"), None);
            }
        }
        """
    )

