module OresSoftware.Dd.FsWs.PgSchema

open System
open System.IO
open System.Reflection
open System.Threading.Tasks
open Microsoft.Extensions.Logging
open Npgsql

// ---------------------------------------------------------------------------
// UnifiedEvent — the wire-format every ingest path converges on before
// hitting the dedup cache / fan-out graph. Kept here (the lowest module in
// the new chain) so PgListen / PgWal / PgOutbox / NatsRx can all produce it
// directly.
// ---------------------------------------------------------------------------

type EventSource =
    | NatsSrc
    | PgNotifySrc
    | PgWalSrc
    | PgOutboxSrc
    | WsPublishSrc

    member this.Label =
        match this with
        | NatsSrc      -> "nats"
        | PgNotifySrc  -> "pg-notify"
        | PgWalSrc     -> "pg-wal"
        | PgOutboxSrc  -> "pg-outbox"
        | WsPublishSrc -> "ws-publish"

[<CLIMutable>]
type UnifiedEvent = {
    /// Idempotency key — matches `fsws_events.event_id`. Two events with the
    /// same id from different sources are the same logical event; the dedup
    /// cache in PresenceFanIn keeps only the first.
    EventId:    Guid
    /// `BIGSERIAL` from the DB. -1 for events that came in via NATS without
    /// hitting the DB first.
    Seq:        int64
    Kind:       string
    ConvId:     Guid
    /// Raw JSON payload as a string. We don't bother parsing it on the way
    /// through — clients of the fan-out see the original payload verbatim.
    Payload:    string
    OccurredAt: DateTime
    /// Where this delivery came from. Useful for /v1/rx-stats/sources and
    /// for differentiating "first hit wins" semantics in the dedup cache.
    Source:     EventSource
}

/// Convert a `postgres://user:pass@host:port/db?sslmode=…` URI into the
/// key=value;key=value form Npgsql expects. Npgsql 10.x doesn't parse URIs
/// natively; ORMs (EFCore.PostgreSQL, etc.) do their own conversion. This is
/// the same shape: `System.Uri` does the URI work, we splice the bits.
///
/// Any query parameter (sslmode, application_name, statement_timeout, …) is
/// passed through verbatim so the cluster's existing `?sslmode=require` (or
/// whatever) doesn't have to be reformatted in the deployment YAML.
let pgUriToConnString (raw: string) : string =
    if String.IsNullOrWhiteSpace(raw) then ""
    elif not (raw.StartsWith("postgres://") || raw.StartsWith("postgresql://")) then
        // Assume it's already in Npgsql key=value form.
        raw
    else
        let u = Uri(raw)
        let userParts = u.UserInfo.Split(':')
        let username =
            if userParts.Length > 0 && userParts.[0] <> "" then
                Some (Uri.UnescapeDataString userParts.[0])
            else None
        let password =
            if userParts.Length > 1 && userParts.[1] <> "" then
                Some (Uri.UnescapeDataString userParts.[1])
            else None
        let host = u.Host
        let port = if u.Port > 0 then u.Port else 5432
        let database =
            if u.AbsolutePath.Length > 1 then
                Some (u.AbsolutePath.Substring(1))
            else None

        let mutable parts = [
            sprintf "Host=%s" host
            sprintf "Port=%d" port
        ]
        match username with Some u -> parts <- parts @ [sprintf "Username=%s" u] | None -> ()
        match password with Some p -> parts <- parts @ [sprintf "Password=%s" p] | None -> ()
        match database with Some d -> parts <- parts @ [sprintf "Database=%s" d] | None -> ()

        if not (String.IsNullOrEmpty u.Query) then
            let q = u.Query.TrimStart('?')
            for kv in q.Split('&') do
                if kv.Contains('=') then
                    let i = kv.IndexOf('=')
                    let k = Uri.UnescapeDataString(kv.Substring(0, i))
                    let v = Uri.UnescapeDataString(kv.Substring(i + 1))
                    parts <- parts @ [sprintf "%s=%s" k v]
        String.Join(";", parts)


/// Idempotent boot-time SQL migration.
///
/// Reads `sql/schema.sql` (relative to the publish output, where the
/// Dockerfile / `dotnet publish` copies it via the `<None CopyToOutputDirectory>`
/// item in the fsproj) and runs it as a single ExecuteNonQuery.
///
/// Every statement in `schema.sql` is `IF NOT EXISTS` / `CREATE OR REPLACE`
/// / wrapped in a DO block, so running it on every pod start is fine — same
/// pattern dd-gleamlang-presence-server uses for `presence_*`.
///
/// Graceful degrade: if `PG_DATABASE_URL` is unset the migrator is never
/// invoked from Program.fs. If it IS invoked and the connection fails, the
/// error is logged and propagated — the F# server then proceeds *without*
/// the PG paths but still serves the existing WS / Rx-advanced endpoints.

let private locateSchemaFile (logger: ILogger) : string option =
    let probe =
        [
            // 1. Next to the running assembly (where `dotnet publish` puts
            //    `sql/schema.sql` thanks to the `<Content Include="sql/**"
            //    CopyToOutputDirectory="PreserveNewest" />` in the fsproj).
            Path.Combine(
                Path.GetDirectoryName(
                    Assembly.GetExecutingAssembly().Location),
                "sql", "schema.sql")
            // 2. Current working directory — useful for `dotnet run` from
            //    the repo root.
            Path.Combine(Environment.CurrentDirectory, "sql", "schema.sql")
            // 3. `/opt/dd-next-1/remote/deployments/fsharp-ws-server/sql/schema.sql` —
            //    the hostPath mount inside the k8s deployment. Only used as
            //    a last-resort fallback; the CopyToOutputDirectory route
            //    is preferred.
            "/opt/dd-next-1/remote/deployments/fsharp-ws-server/sql/schema.sql"
        ]
    probe
    |> List.tryFind File.Exists
    |> function
        | Some p ->
            logger.LogInformation("pg-schema: located schema.sql at {Path}", p)
            Some p
        | None ->
            logger.LogWarning(
                "pg-schema: schema.sql not found in any probe path; \
                 schema migration will be skipped (PG paths will fail if \
                 the schema isn't already provisioned)")
            None

/// Run the idempotent migration. Safe to call on every boot.
let migrate
        (logger: ILogger)
        (connectionString: string)
        : Task<bool> =
    task {
        match locateSchemaFile logger with
        | None -> return false
        | Some path ->
            let sql = File.ReadAllText(path)
            try
                use conn = new NpgsqlConnection(connectionString)
                do! conn.OpenAsync()
                use cmd = new NpgsqlCommand(sql, conn)
                cmd.CommandTimeout <- 30
                let! _ = cmd.ExecuteNonQueryAsync()
                logger.LogInformation(
                    "pg-schema: migration completed ({Bytes} bytes)",
                    sql.Length)
                return true
            with ex ->
                logger.LogError(
                    ex,
                    "pg-schema: migration failed; PG-backed Rx sources \
                     will be disabled this boot")
                return false
    }

/// Ensure the per-pod WAL slot exists. Called once at boot AFTER `migrate`,
/// before PgWal subscribes. Returns the slot name actually used so PgWal can
/// log it / report it through /v1/rx-stats/sources.
///
/// Slot name format: `fsws_wal_<sanitised-machine-name>` — keeps the slot
/// pod-scoped, matching the Gleam reference's `presence_wal_<node>` pattern.
let ensureWalSlot
        (logger: ILogger)
        (connectionString: string)
        : Task<string option> =
    task {
        let raw = Environment.MachineName
        let sanitised =
            raw.ToLowerInvariant()
            |> String.map (fun c ->
                if Char.IsLetterOrDigit c || c = '_' then c else '_')
        let slot = "fsws_wal_" + sanitised
        try
            use conn = new NpgsqlConnection(connectionString)
            do! conn.OpenAsync()
            // First, check whether logical replication is even available
            // on this server. If not, log and bail — PgWal will still get
            // None back and skip subscription.
            use checkCmd = new NpgsqlCommand("SELECT fsws_wal_available()", conn)
            let! available = checkCmd.ExecuteScalarAsync()
            let availableBool =
                match available with
                | :? bool as b -> b
                | _ -> false
            if not availableBool then
                logger.LogWarning(
                    "pg-schema: WAL slot creation skipped — \
                     wal2json missing or wal_level <> logical")
                return None
            else
                use ensureCmd =
                    new NpgsqlCommand("SELECT fsws_ensure_wal_slot(@slot)", conn)
                ensureCmd.Parameters.AddWithValue("slot", slot) |> ignore
                let! _ = ensureCmd.ExecuteScalarAsync()
                logger.LogInformation("pg-schema: WAL slot {Slot} ready", slot)
                return Some slot
        with ex ->
            logger.LogWarning(
                ex,
                "pg-schema: ensureWalSlot failed; PgWal will skip itself")
            return None
    }
