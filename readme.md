# t2v-v2t.rs

Voice-to-text / text-to-voice / translation platform in Rust. Two separately
deployed servers over a shared workspace:

- **t2v-api** — JSON API: speech-to-text, text-to-speech, AI translation, a full
  speech-to-speech pipeline, FFT-based audio analysis, and [Vapi.ai](https://vapi.ai)
  telephony webhooks.
- **t2v-web** — **MASH** dashboard (**M**aud + **A**xum + **S**eaORM + **h**tmx),
  streaming live stats over a websocket.

Stack per the fleet conventions: **maud + axum + SeaORM (not sqlx)**, raw-HTTP
LLM clients (no provider SDKs), and a **hand-rolled DSP core with custom FFT
implementations** (no `rustfft`, no `hound`).

## Workspace

| crate | what it is |
| --- | --- |
| `crates/core` (`t2v-core`) | Zero-dependency DSP: three custom FFTs (naive DFT, recursive + iterative radix-2 Cooley-Tukey), Goertzel single-bin detection, DTMF decoder, WAV codec, G.711 mu-law, linear resampling, energy VAD, STFT + spectral features. |
| `crates/llm` (`t2v-llm`) | Raw-HTTP clients for OpenAI / Gemini / Anthropic translation, OpenAI Whisper STT, and OpenAI TTS. Response extraction is pure + unit-tested offline. |
| `crates/entity` (`t2v-entity`) | SeaORM entities for the `t2v` tables. |
| `crates/migration` (`t2v-migration`) | SeaORM migrations — **SQLite local-dev bootstrap only**; Postgres DDL is owned by `pg-defs`/dpm (see below). |
| `crates/api` (`t2v-api`) | The JSON API server + Vapi webhooks. |
| `crates/web` (`t2v-web`) | The MASH web dashboard. |

## The custom FFT core

`t2v-core` implements the Fourier transform three independent ways and
cross-checks them against each other in the test suite:

1. `dft_naive` — textbook O(n²) DFT, the correctness reference.
2. `fft_recursive` — allocating radix-2 Cooley-Tukey, the readable one.
3. `FftPlanner` — iterative in-place radix-2 with a bit-reversal permutation and
   a precomputed twiddle table. O(n log n), zero per-transform allocation. This
   is the production path used by the STFT and the `/v1/analyze` endpoint.

Tests assert all three agree, that forward∘inverse round-trips, Parseval energy
conservation, and that a known sine peaks in the expected bin. The Goertzel
detector powers a full DTMF (touch-tone) decoder — natural for telephony audio
arriving over Vapi.

## API endpoints (t2v-api, default `:8130`)

| method + path | purpose |
| --- | --- |
| `GET /healthz`, `GET /readyz`, `GET /metrics` | liveness / readiness (DB ping) / Prometheus text |
| `POST /v1/stt` | audio body → transcription (VAD-trimmed via the DSP core, resampled to 16 kHz, Whisper) |
| `POST /v1/tts` | `{text, voice?, format?}` → audio bytes (`wav`/`mp3`) |
| `POST /v1/translate` | `{text, target_lang, source_lang?, provider?}` → translation |
| `POST /v1/speech-to-speech` | audio body → translated audio (STT → translate → TTS) |
| `POST /v1/analyze` | audio body → FFT spectrum, dominant freq, spectral centroid, RMS dBFS, DTMF digits |
| `GET /v1/history/{transcriptions,translations,syntheses,vapi-calls}` | recent rows |
| `POST /vapi/webhook` | Vapi server webhook (`x-vapi-secret`); serves a live-translator assistant and a `translate_text` tool |
| `POST /vapi/call`, `GET /vapi/call/{id}` | operator passthrough to the Vapi REST API (`VAPI_API_KEY`) |

Audio bodies default to WAV; pass `?format=mulaw&rate=8000` for raw G.711
telephony audio.

### Examples

```sh
# Translate
curl -sX POST localhost:8130/v1/translate \
  -H 'content-type: application/json' \
  -d '{"text":"Good morning","target_lang":"Spanish","provider":"anthropic"}'

# Analyze a WAV with the custom FFT core (no provider key needed)
curl -sX POST --data-binary @clip.wav localhost:8130/v1/analyze

# Speech-to-speech: English WAV in, Spanish MP3 out
curl -sX POST --data-binary @hello.wav \
  'localhost:8130/v1/speech-to-speech?target_lang=Spanish&out_format=mp3' -o out.mp3
```

## Web dashboard (t2v-web, default `:8131`)

Server-rendered with maud, interactive via htmx, live stats pushed over a
websocket (`hx-ext="ws"` with out-of-band swaps). Interactive translate/TTS
actions are proxied to t2v-api (`API_BASE_URL`). Reads the `t2v` namespace
directly for the dashboard and history.

## Configuration

Both servers read config from the environment.

| var | used by | default | notes |
| --- | --- | --- | --- |
| `PORT` | both | 8130 / 8131 | listen port |
| `DATABASE_URL` | both | `sqlite://./t2v.sqlite?mode=rwc` | Postgres uses `search_path=t2v` |
| `DB_MAX_CONNECTIONS` | both | 10 / 5 | pool size |
| `OPENAI_API_KEY` | api | — | STT, TTS, and OpenAI translation |
| `GEMINI_API_KEY` (or `GOOGLE_API_KEY`) | api | — | Gemini translation |
| `ANTHROPIC_API_KEY` | api | — | Anthropic translation |
| `OPENAI_MODEL` / `GEMINI_MODEL` / `ANTHROPIC_MODEL` | api | `gpt-4o-mini` / `gemini-2.0-flash` / `claude-sonnet-5` | translation models |
| `OPENAI_STT_MODEL` / `OPENAI_TTS_MODEL` / `OPENAI_TTS_VOICE` | api | `whisper-1` / `tts-1` / `alloy` | speech models |
| `*_BASE_URL` | api | provider defaults | override endpoints (tests, proxies) |
| `VAPI_API_KEY` | api | — | operator REST passthrough |
| `VAPI_WEBHOOK_SECRET` | api | — | required `x-vapi-secret`; **unset = webhook fails closed** |
| `T2V_ALLOW_INSECURE_WEBHOOK` | api | `false` | opt into an unauthenticated webhook when the secret is unset (dev only) |
| `T2V_SERVER_AUTH_SECRET` | api | — | bearer token for operator (`/vapi/call`) + `/v1/history/*`; **unset = those routes return 503** |
| `T2V_MAX_INFLIGHT_LLM` | api | `32` | max concurrent upstream LLM calls; excess → 503 |
| `T2V_REQUEST_TIMEOUT_SECS` | api | `300` | backstop request timeout |
| `VAPI_MODEL` / `VAPI_MODEL_PROVIDER` | api | `gpt-4o` / `openai` | live-translator assistant model |
| `VAPI_VOICE_PROVIDER` / `VAPI_VOICE_ID` | api | `openai` / `alloy` | assistant TTS voice |
| `VAPI_SERVER_URL` | api | — | public `/vapi/webhook`; when set, put on the assistant's `server` block so Vapi routes callbacks here with the secret |
| `VAPI_TOOL_TIMEOUT_SECS` | api | `30` | server-tool timeout Vapi waits for the webhook |
| `API_BASE_URL` | web | `http://localhost:8130` | where the dashboard sends actions |

### Vapi.ai interop

On `assistant-request` the webhook returns a live-translator assistant with an
explicit **`voice`**, a **`server`** block (URL + `x-vapi-secret` + tool timeout,
when `VAPI_SERVER_URL` is set) that routes `tool-calls` / `status-update` /
`end-of-call-report` back here, and a `translate_text` server tool. Tool results
follow Vapi's `ToolCallResult` contract: success returns the translated text as a
bare **string** (spoken straight back to the caller), failures use the **`error`**
field. `status-update` upserts call status; `end-of-call-report` persists
transcript + summary; every event is audited to `t2v.vapi_events`.

### Security posture

- **Operator + history endpoints fail closed.** `POST /vapi/call`, `GET /vapi/call/{id}`, and every `/v1/history/*` route require `Authorization: Bearer $T2V_SERVER_AUTH_SECRET`; with no secret configured they return 503, never an unauthenticated call that spends money or leaks stored transcripts.
- **The Vapi webhook fails closed.** With no `VAPI_WEBHOOK_SECRET` it rejects all posts unless `T2V_ALLOW_INSECURE_WEBHOOK=true`. With a secret it checks `x-vapi-secret` in constant time.
- **Request body limits:** 25 MB on audio uploads (Whisper's cap), 1 MB on JSON and the webhook.
- **LLM concurrency ceiling** sheds load with 503 instead of piling up unbounded upstream calls.
- **Web dashboard** ships a CSP with `default-src 'self'` and strict **`script-src 'self'`** (no external hosts, no inline/eval scripts) — htmx is **vendored into the binary** and served from our own origin, no CDN. `style-src` also allows `'unsafe-inline'` because htmx injects a small inline indicator `<style>` at runtime (inline styles are low-risk; blocking them breaks htmx's UI — caught by the browser e2e). Plus `nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, and a Permissions-Policy.

Missing provider keys are tolerated at startup and reported per-request, so a
deployment with only one key still serves that provider.

## Database — the `t2v` Postgres namespace

The service owns a dedicated Postgres schema namespace, **`t2v`**, in the shared
`pg-defs` contract (`k8s-cluster/remote/libs/pg-defs/schema/schema.sql`). That
file is the desired state; the live database converges onto it **declaratively
via [`dpm`](https://github.com/declarative-migrations/declarative-postgres-migrate.rs)**
(`scripts/dpm.sh diff | verify | apply`) — the app never runs DDL against
Postgres. Against Postgres both servers connect with `search_path=t2v`.

The bundled `t2v-migration` crate exists **only** to self-provision SQLite for
local dev and tests; it does not run against Postgres.

## Develop

```sh
cargo test --workspace          # 60+ tests, no network required
cargo clippy --workspace --all-targets
DATABASE_URL='sqlite://./t2v.sqlite?mode=rwc' cargo run --bin t2v-api
API_BASE_URL=http://localhost:8130 cargo run --bin t2v-web
```

## Build images

```sh
docker build --build-arg BIN=t2v-api -t t2v-api:dev .
docker build --build-arg BIN=t2v-web -t t2v-web:dev .
```

## Deploy

Deployed from `k8s-cluster` as a git submodule at
`remote/deployments/t2v-v2t.rs`, with ArgoCD manifests under
`remote/argocd/dd-next-runtime/` (`dd-t2v-api.*`, `dd-t2v-web.*`). The two
binaries run as separate Deployments/Services behind their own NetworkPolicies
and PodDisruptionBudgets — see those manifests for the ports, probes, and
secret wiring.
