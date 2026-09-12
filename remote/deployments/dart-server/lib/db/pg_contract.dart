/// Single import site for the canonical pg-defs contract surface.
///
/// This module mirrors the role of [`rest-api-rs/src/pg_contract.rs`](../../../../rest-api-rs/src/pg_contract.rs)
/// for dd-dart-server. Reads / writes against the shared RDS Postgres
/// schema flow through here so the source-of-truth contract stays
/// single, and any future schema regen is caught at process startup
/// long before a stale read or write reaches production.
///
/// Three layers:
///
///   1. **Re-exports** — convenient `pg_contract.appConfigTable`, etc.
///      access without pulling `dd_pg_defs` into every callsite.
///   2. **Local table lists** — the tables this service actually reads
///      from / writes to. These get checked against the canonical
///      constants at startup.
///   3. **Startup assertion** — [assertPgContract] is called once from
///      `main()` so a schema regen that drops a table we depend on
///      fails fast, with a clear error naming the offending table.
///
/// We deliberately do **not** generate SQL or table DDL here. The schema
/// authority is `remote/libs/pg-defs/schema/schema.sql`; we are an
/// adapter only. See `AGENTS.md` for the full rule.
library;

import 'package:dd_pg_defs/pg_defs.dart' as pg;

// ---------------------------------------------------------------------------
// 1. Re-exports — table-name + select-SQL constants and Row classes for
//    every pg-defs table this service is allowed to consume. Keep this
//    block in alphabetical order so audits stay readable.
// ---------------------------------------------------------------------------

const appConfigTable = pg.appConfigTable;
const appConfigSelectSql = pg.appConfigSelectSql;
const appConfigStatusValues = pg.appConfigStatusValues;
typedef AppConfigRow = pg.AppConfigRow;

const containerPoolConfigsTable = pg.containerPoolConfigsTable;
const containerPoolConfigsSelectSql = pg.containerPoolConfigsSelectSql;
const containerPoolConfigsStatusValues = pg.containerPoolConfigsStatusValues;
typedef ContainerPoolConfigsRow = pg.ContainerPoolConfigsRow;

const knownGitRepoTable = pg.knownGitRepoTable;
const knownGitRepoSelectSql = pg.knownGitRepoSelectSql;
typedef KnownGitRepoRow = pg.KnownGitRepoRow;

const presenceConvsTable = pg.presenceConvsTable;
const presenceConvsSelectSql = pg.presenceConvsSelectSql;
const presenceConvsStatusValues = pg.presenceConvsStatusValues;
typedef PresenceConvsRow = pg.PresenceConvsRow;

const presenceConvMembersTable = pg.presenceConvMembersTable;
const presenceConvMembersSelectSql = pg.presenceConvMembersSelectSql;
const presenceConvMembersRoleValues = pg.presenceConvMembersRoleValues;
const presenceConvMembersStatusValues = pg.presenceConvMembersStatusValues;
typedef PresenceConvMembersRow = pg.PresenceConvMembersRow;

const presenceUsersTable = pg.presenceUsersTable;
const presenceUsersSelectSql = pg.presenceUsersSelectSql;
typedef PresenceUsersRow = pg.PresenceUsersRow;

const presenceEventsTable = pg.presenceEventsTable;
const presenceEventsSelectSql = pg.presenceEventsSelectSql;
const presenceEventsOpValues = pg.presenceEventsOpValues;
typedef PresenceEventsRow = pg.PresenceEventsRow;

const presenceConsumerCheckpointsTable = pg.presenceConsumerCheckpointsTable;
const presenceConsumerCheckpointsSelectSql =
    pg.presenceConsumerCheckpointsSelectSql;
typedef PresenceConsumerCheckpointsRow = pg.PresenceConsumerCheckpointsRow;

// ---------------------------------------------------------------------------
// 2. Local table lists — what this service touches. Both lists must
//    only contain canonical table names that exist in pg-defs; the
//    startup assertion below proves that.
// ---------------------------------------------------------------------------

/// Tables this service reads from (SELECTs).
const localReadableTables = <String>[
  presenceConvsTable,
  presenceConvMembersTable,
  presenceUsersTable,
  presenceEventsTable,
  presenceConsumerCheckpointsTable,
  appConfigTable,
];

/// Tables this service writes to (INSERT / UPDATE / DELETE).
///
/// The `presence_events` table is write-only here for the outbox: every
/// presence/conversation mutation that should fan out across pods rides
/// through the canonical insert + LISTEN/NOTIFY pipeline declared in
/// `schema.sql`. We do not write to `presence_*` data tables directly
/// from individual session isolates — the supervisor on the main isolate
/// owns those writes (and those will land in a follow-up PR; this
/// contract module is the surface they will use).
const localWritableTables = <String>[
  presenceConvsTable,
  presenceConvMembersTable,
  presenceUsersTable,
  presenceEventsTable,
  presenceConsumerCheckpointsTable,
];

/// Every canonical pg-defs table this service knows about (read or
/// write). Used by [assertPgContract] to verify the bindings are still
/// in sync with the canonical schema after a regen.
final allReferencedTables = <String>{
  ...localReadableTables,
  ...localWritableTables,
};

/// Snapshot of every canonical table we currently re-export. Used as a
/// belt-and-braces check against a regen that renames a table.
const _exportedTables = <String>[
  appConfigTable,
  containerPoolConfigsTable,
  knownGitRepoTable,
  presenceConvsTable,
  presenceConvMembersTable,
  presenceUsersTable,
  presenceEventsTable,
  presenceConsumerCheckpointsTable,
];

// ---------------------------------------------------------------------------
// 3. Startup assertion.
// ---------------------------------------------------------------------------

/// Verify our local table-name lists still match the canonical pg-defs
/// constants. Throws if a referenced table does not exist in the
/// generated adapter — that means the schema was regenerated, the table
/// was renamed/dropped, and our wiring needs to follow.
///
/// Call this exactly once from `main()`, ideally before binding the
/// HTTP server, so a regen mismatch fails fast and loudly rather than
/// surfacing as a runtime SQL error against a live RDS hours later.
void assertPgContract() {
  final canonicalTables = <String>{
    pg.appConfigTable,
    pg.containerPoolConfigsTable,
    pg.knownGitRepoTable,
    pg.agentContextBlobsTable,
    pg.agentContextEmbeddingsTable,
    pg.agentRemoteDevThreadTable,
    pg.agentRemoteDevTaskTable,
    pg.agentRemoteDevEventTable,
    pg.agentRemoteDevBreadcrumbTable,
    pg.agentRemoteDevArtifactTable,
    pg.agentRemoteDevRuntimeLockTable,
    pg.lambdaFunctionTable,
    pg.containerPoolImageRevisionsTable,
    pg.containerPoolBuildRunsTable,
    pg.presenceConvsTable,
    pg.presenceConvMembersTable,
    pg.presenceUsersTable,
    pg.presenceEventsTable,
    pg.presenceConsumerCheckpointsTable,
  };

  final missingExports = _exportedTables
      .where((t) => !canonicalTables.contains(t))
      .toList(growable: false);
  if (missingExports.isNotEmpty) {
    throw StateError(
      'pg_contract: re-exported tables no longer exist in dd_pg_defs: '
      '${missingExports.join(', ')}. '
      'Schema regen detected; update pg_contract.dart to follow.',
    );
  }

  final missingReferences = allReferencedTables
      .where((t) => !canonicalTables.contains(t))
      .toList(growable: false);
  if (missingReferences.isNotEmpty) {
    throw StateError(
      'pg_contract: localReadableTables / localWritableTables reference '
      'tables not present in dd_pg_defs: ${missingReferences.join(', ')}. '
      'Either regenerate pg-defs to restore them, or remove them from '
      'this service\'s contract surface.',
    );
  }
}

/// Returns a snapshot suitable for logging at boot or surfacing on
/// `/dart/admin/db`. Pure data — no side effects.
Map<String, Object?> pgContractSnapshot() => <String, Object?>{
      'exported': _exportedTables,
      'readable': localReadableTables,
      'writable': localWritableTables,
    };
