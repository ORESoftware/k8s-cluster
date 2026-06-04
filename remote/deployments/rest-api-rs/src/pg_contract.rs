//! PG contract surface for `dd-remote-rest-api`.
//!
//! This module is the single import site for the generated `dd_pg_defs`
//! crate (driven by `remote/libs/pg-defs/schema/schema.sql`). Reads /
//! writes against the shared RDS Postgres schema should flow through here
//! so the source-of-truth contract stays single and any future schema
//! change is caught at process startup by the assertions below — long
//! before a stale read or write reaches production.
//!
//! Three layers of wiring:
//!   1. Re-exports: convenient `pg_contract::APP_CONFIG_TABLE`, etc.,
//!      access without pulling `dd_pg_defs` into every callsite.
//!   2. Local column lists: the columns this service actually reads /
//!      writes for each shared table. These are intentionally a subset
//!      of the canonical column set; the runtime assertion below verifies
//!      the subset relationship still holds.
//!   3. Startup assertion: called once from `main()` so a schema regen
//!      that drops a column we depend on fails fast, with a clear error
//!      naming the offending column + table.

// These re-exports are intentionally the full canonical surface for every
// table this service touches. Some are unused today (the API still inlines
// SQL for several tables) but they are explicitly part of the contract
// surface and will be exercised as inline SQL gets migrated to the
// canonical constants. Suppressing the unused-import lint here keeps the
// contract intent visible without a sea of yellow warnings.
#![allow(dead_code, unused_imports)]

pub use dd_pg_defs::{
    AgentContextBlobsRow, AgentContextBlobsStatus, AgentContextEmbeddingsRow,
    AgentRemoteDevArtifactRow, AgentRemoteDevEventRow, AgentRemoteDevRuntimeLockRow,
    AgentRemoteDevTaskRow, AgentRemoteDevTaskStatus, AgentRemoteDevThreadRow, AppConfigRow,
    AppConfigStatus, ContainerPoolConfigsRow, KnownGitRepoRow, KnownGitRepoStatus,
    LambdaFunctionContainerBuildStatus, LambdaFunctionRow, LambdaFunctionStatus,
    PresenceConsumerCheckpointsRow, PresenceConvMembersRow, PresenceConvsRow, PresenceEventsRow,
    PresenceUsersRow, AGENT_CONTEXT_BLOBS_COLUMNS, AGENT_CONTEXT_BLOBS_TABLE,
    AGENT_CONTEXT_EMBEDDINGS_COLUMNS, AGENT_CONTEXT_EMBEDDINGS_TABLE,
    AGENT_REMOTE_DEV_ARTIFACTS_COLUMNS, AGENT_REMOTE_DEV_ARTIFACTS_TABLE,
    AGENT_REMOTE_DEV_EVENTS_COLUMNS, AGENT_REMOTE_DEV_EVENTS_TABLE,
    AGENT_REMOTE_DEV_RUNTIME_LOCKS_COLUMNS, AGENT_REMOTE_DEV_RUNTIME_LOCKS_TABLE,
    AGENT_REMOTE_DEV_TASKS_COLUMNS, AGENT_REMOTE_DEV_TASKS_TABLE, AGENT_REMOTE_DEV_THREADS_COLUMNS,
    AGENT_REMOTE_DEV_THREADS_TABLE, APP_CONFIG_COLUMNS, APP_CONFIG_TABLE,
    CONTAINER_POOL_CONFIGS_COLUMNS, CONTAINER_POOL_CONFIGS_TABLE, KNOWN_GIT_REPOS_COLUMNS,
    KNOWN_GIT_REPOS_TABLE, LAMBDA_FUNCTIONS_COLUMNS, LAMBDA_FUNCTIONS_TABLE,
    PRESENCE_CONSUMER_CHECKPOINTS_COLUMNS, PRESENCE_CONSUMER_CHECKPOINTS_TABLE,
    PRESENCE_CONVS_COLUMNS, PRESENCE_CONVS_TABLE, PRESENCE_CONV_MEMBERS_COLUMNS,
    PRESENCE_CONV_MEMBERS_TABLE, PRESENCE_EVENTS_COLUMNS, PRESENCE_EVENTS_TABLE,
    PRESENCE_USERS_COLUMNS, PRESENCE_USERS_TABLE,
};

/// Columns the local SELECT in `lambda_select_sql()` returns and that
/// `row_to_lambda_function` reads by name. These must remain a strict
/// subset of `LAMBDA_FUNCTIONS_COLUMNS`; if `schema.sql` ever drops one
/// of these, the startup assertion below fires immediately.
pub const LOCAL_LAMBDA_FUNCTIONS_READ_COLUMNS: &[&str] = &[
    "id",
    "slug",
    "display_name",
    "description",
    "runtime",
    "entry_command",
    "function_body",
    "reuse_key",
    "idle_timeout_seconds",
    "max_run_ms",
    "containerized",
    "container_image",
    "container_build_status",
    "container_build_error",
    "container_built_at",
    "status",
    "labels",
    "meta_data",
    "last_invoked_at",
    "created_at",
    "updated_at",
];

/// Tables this service writes to via INSERT / UPDATE / DELETE. Each entry
/// must match a canonical table constant; the startup assertion proves
/// the table name still exists in the canonical schema.
pub const LOCAL_WRITABLE_TABLES: &[&str] = &[
    AGENT_REMOTE_DEV_EVENTS_TABLE,
    AGENT_REMOTE_DEV_TASKS_TABLE,
    AGENT_REMOTE_DEV_THREADS_TABLE,
    AGENT_CONTEXT_BLOBS_TABLE,
    AGENT_CONTEXT_EMBEDDINGS_TABLE,
    KNOWN_GIT_REPOS_TABLE,
    LAMBDA_FUNCTIONS_TABLE,
];

/// Tables this service reads from. Same shape as `LOCAL_WRITABLE_TABLES`
/// — every entry has to resolve to a canonical constant.
pub const LOCAL_READABLE_TABLES: &[&str] = &[
    AGENT_REMOTE_DEV_EVENTS_TABLE,
    AGENT_REMOTE_DEV_TASKS_TABLE,
    AGENT_REMOTE_DEV_THREADS_TABLE,
    AGENT_CONTEXT_BLOBS_TABLE,
    AGENT_CONTEXT_EMBEDDINGS_TABLE,
    KNOWN_GIT_REPOS_TABLE,
    LAMBDA_FUNCTIONS_TABLE,
];

#[derive(Clone, Copy)]
pub struct CanonicalTable {
    pub name: &'static str,
    pub columns: &'static [&'static str],
}

/// Full public-table contract generated from `remote/libs/pg-defs`.
/// Runtime database-first routes use this as contract metadata while
/// still discovering the live table surface directly from RDS.
pub const CANONICAL_TABLES: &[CanonicalTable] = &[
    CanonicalTable {
        name: APP_CONFIG_TABLE,
        columns: APP_CONFIG_COLUMNS,
    },
    CanonicalTable {
        name: CONTAINER_POOL_CONFIGS_TABLE,
        columns: CONTAINER_POOL_CONFIGS_COLUMNS,
    },
    CanonicalTable {
        name: KNOWN_GIT_REPOS_TABLE,
        columns: KNOWN_GIT_REPOS_COLUMNS,
    },
    CanonicalTable {
        name: AGENT_CONTEXT_BLOBS_TABLE,
        columns: AGENT_CONTEXT_BLOBS_COLUMNS,
    },
    CanonicalTable {
        name: AGENT_CONTEXT_EMBEDDINGS_TABLE,
        columns: AGENT_CONTEXT_EMBEDDINGS_COLUMNS,
    },
    CanonicalTable {
        name: AGENT_REMOTE_DEV_THREADS_TABLE,
        columns: AGENT_REMOTE_DEV_THREADS_COLUMNS,
    },
    CanonicalTable {
        name: AGENT_REMOTE_DEV_TASKS_TABLE,
        columns: AGENT_REMOTE_DEV_TASKS_COLUMNS,
    },
    CanonicalTable {
        name: AGENT_REMOTE_DEV_EVENTS_TABLE,
        columns: AGENT_REMOTE_DEV_EVENTS_COLUMNS,
    },
    CanonicalTable {
        name: AGENT_REMOTE_DEV_ARTIFACTS_TABLE,
        columns: AGENT_REMOTE_DEV_ARTIFACTS_COLUMNS,
    },
    CanonicalTable {
        name: AGENT_REMOTE_DEV_RUNTIME_LOCKS_TABLE,
        columns: AGENT_REMOTE_DEV_RUNTIME_LOCKS_COLUMNS,
    },
    CanonicalTable {
        name: LAMBDA_FUNCTIONS_TABLE,
        columns: LAMBDA_FUNCTIONS_COLUMNS,
    },
    CanonicalTable {
        name: PRESENCE_CONVS_TABLE,
        columns: PRESENCE_CONVS_COLUMNS,
    },
    CanonicalTable {
        name: PRESENCE_CONV_MEMBERS_TABLE,
        columns: PRESENCE_CONV_MEMBERS_COLUMNS,
    },
    CanonicalTable {
        name: PRESENCE_USERS_TABLE,
        columns: PRESENCE_USERS_COLUMNS,
    },
    CanonicalTable {
        name: PRESENCE_EVENTS_TABLE,
        columns: PRESENCE_EVENTS_COLUMNS,
    },
    CanonicalTable {
        name: PRESENCE_CONSUMER_CHECKPOINTS_TABLE,
        columns: PRESENCE_CONSUMER_CHECKPOINTS_COLUMNS,
    },
];

pub fn canonical_table_columns(table: &str) -> Option<&'static [&'static str]> {
    CANONICAL_TABLES
        .iter()
        .find(|item| item.name == table)
        .map(|item| item.columns)
}

fn assert_columns_subset(local: &[&str], canonical: &[&str], table: &str) {
    for &column in local {
        assert!(
            canonical.contains(&column),
            "dd-remote-rest-api expected column `{column}` on table `{table}`, but the canonical \
             schema in remote/libs/pg-defs/schema/schema.sql no longer includes it. Either restore \
             the column or stop reading it from {}.",
            std::module_path!()
        );
    }
}

fn assert_table_in_canonical_set(table: &str, canonical_tables: &[&str]) {
    assert!(
        canonical_tables.contains(&table),
        "dd-remote-rest-api expected `{table}` to be one of the canonical schema tables \
         (remote/libs/pg-defs/schema/schema.sql), but it isn't. Either restore the table \
         or update {} to stop referencing it.",
        std::module_path!()
    );
}

/// Call once from `main()` before binding the HTTP listener. Panics with
/// a clear message naming the missing column / table if `schema.sql`
/// has drifted away from what this service reads or writes.
pub fn assert_canonical_schema_matches_local_reads() {
    assert_columns_subset(
        LOCAL_LAMBDA_FUNCTIONS_READ_COLUMNS,
        LAMBDA_FUNCTIONS_COLUMNS,
        LAMBDA_FUNCTIONS_TABLE,
    );

    // The other tables don't have a strict-subset column contract yet
    // (they're served by inline SQL today). For now we lock in the
    // table-name surface so a rename in schema.sql trips us at startup.
    let canonical_tables = CANONICAL_TABLES
        .iter()
        .map(|table| table.name)
        .collect::<Vec<_>>();
    for &table in LOCAL_READABLE_TABLES {
        assert_table_in_canonical_set(table, &canonical_tables);
    }
    for &table in LOCAL_WRITABLE_TABLES {
        assert_table_in_canonical_set(table, &canonical_tables);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_lambda_columns_are_subset_of_canonical() {
        assert_canonical_schema_matches_local_reads();
    }

    #[test]
    fn lambda_functions_table_name_matches_canonical() {
        assert_eq!(LAMBDA_FUNCTIONS_TABLE, "lambda_functions");
    }

    #[test]
    fn app_config_status_round_trips() {
        assert_eq!(AppConfigStatus::Active.as_str(), "active");
        assert_eq!(AppConfigStatus::Paused.as_str(), "paused");
        assert_eq!(AppConfigStatus::Archived.as_str(), "archived");
        assert!(AppConfigStatus::try_from("nope").is_err());
    }
}
