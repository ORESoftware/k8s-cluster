# `dd-music-rs`

Rust music server for generated tracks from the `discrete-event-system.rs`
music-production module.

## Shape

- Generates WAV songs in-process through `des_engine::des::general::music_production`.
- Curates candidates with a simple listenability score and stores discarded attempts in Postgres.
- Uploads published audio to S3 by default with S3-managed server-side encryption.
- Serves a public HTMX landing page with native browser audio players, not an embedded third-party
  player.
- Stores song metadata and anonymous votes in RDS Postgres through the canonical
  `remote/libs/pg-defs/schema/schema.sql` contract.
- Uses Redis only for short-lived coordination: daily target cache, generation lock, and vote
  throttling.

## Routes

- `GET /` — public landing page, song shelf, and player.
- `GET /songs` — latest published songs.
- `GET /songs/shelf` — HTMX-rendered latest-song shelf.
- `GET /songs/:song_id` — one song.
- `GET /songs/:song_id/audio` — increments `play_count` and redirects to the stored audio URL.
- `POST /songs/:song_id/votes` — anonymous up/down vote; durable state is one vote per visitor hash.
  JSON clients receive JSON, while HTMX requests receive an updated song card.
- `POST /internal/generate` — server-authenticated manual generation.
- `GET /healthz`, `GET /readyz`, `GET /metrics`.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json`.

At the public gateway, `/music/`, `/music/songs`, `/music/songs/:song_id`, and
`/music/songs/:song_id/audio` are anonymous read routes. Anonymous vote writes are limited to
`POST /music/songs/:song_id/votes` with gateway rate limiting. Internal generation, health,
readiness, metrics, and generated API docs are operator-authenticated at the gateway.

## Environment

| Var | Default | Notes |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | Bind host. |
| `PORT` | `8115` | Bind port. |
| `MUSIC_RDS_DATABASE_URL` | falls back to `AGENT_TASKS_RDS_DATABASE_URL`, `RDS_DATABASE_URL`, `DATABASE_URL` | Postgres URL. |
| `MUSIC_STORAGE_PROVIDER` | `s3` | `s3` or `local`. |
| `MUSIC_S3_BUCKET` / `S3_BUCKET` | unset | Required for S3 publishing. |
| `MUSIC_S3_PUBLIC_BASE_URL` / `S3_PUBLIC_BASE_URL` | unset | Public base URL for S3 objects. |
| `MUSIC_S3_KEY_PREFIX` | `music/generated` | Object key prefix. |
| `MUSIC_REDIS_URL` / `REDIS_URL` | in-cluster `dd-redis-cache` | Optional but recommended. |
| `MUSIC_GENERATOR_ENABLED` | `false` | Enables the daily background generator. |
| `MUSIC_DAILY_TARGET_MIN` / `MUSIC_DAILY_TARGET_MAX` | `3` / `5` | Daily published-song target. |
| `MUSIC_SONG_DURATION_SECONDS` | `180` | Duration per generated song; clamped to 10-600 seconds. |
| `MUSIC_MIN_LISTENABILITY_SCORE` | `0.55` | Candidates below this are discarded. |
| `MUSIC_SERVER_AUTH_SECRET` / `SERVER_AUTH_SECRET` | unset | Required for `/internal/generate` unless local unauth is enabled. |
| `MUSIC_VOTE_HASH_SALT` | `SERVER_AUTH_SECRET` or local fallback | Salt for anonymous visitor hashes; readiness requires either this or `SERVER_AUTH_SECRET`. |

`/readyz` reports HTTP readiness for Kubernetes and includes `generationReady` plus degraded-mode
fields for Postgres, storage, internal auth, and vote hash salt configuration. Public read pages stay
reachable in degraded mode; the background generator only starts after Postgres and storage are
configured.

## Local Smoke

```bash
cd remote/deployments/dd-music-rs
MUSIC_STORAGE_PROVIDER=local \
MUSIC_LOCAL_STORAGE_ROOT=/tmp/dd-music-rs/audio \
MUSIC_LOCAL_PUBLIC_BASE_URL=/local-audio \
MUSIC_ALLOW_UNAUTHENTICATED_INTERNAL=true \
MUSIC_SONG_DURATION_SECONDS=15 \
cargo run
```

The local route can render the page without credentials, but `/songs` and generation need a
Postgres URL pointed at a database with the `music_songs` and `music_song_votes` tables applied.

> **ORM policy:** prefer **SeaORM** over sqlx for new database code (MASH stack: maud, axum, SeaORM, supabase, htmx).
