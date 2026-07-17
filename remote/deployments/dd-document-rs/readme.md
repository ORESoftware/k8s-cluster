# `dd-document-rs`

Rust document **and image** manipulation service backed by two official SDKs over
FFI — the **Pandoc Haskell SDK** and the **ImageMagick C++ SDK (Magick++)**.

Instead of shelling out to the `pandoc` or `magick`/`convert` CLIs, the service
links native bridges that call the libraries directly:

- documents → `libdd-pandoc-bridge.so` calling
  [`Text.Pandoc`](https://hackage.haskell.org/package/pandoc)
- images → a Magick++ shim calling
  [Magick++](https://imagemagick.org/script/magick++.php), ImageMagick's official
  C++ binding

## Architecture

```
HTTP / NATS  ─▶  dd-document-rs (Rust, axum)
                      │
        documents     │  extern "C" (src/ffi.rs)
            ──────────┼────────────▶  libdd-pandoc-bridge.so  (haskell/src/PandocBridge.hs)
                      │                       └▶ Text.Pandoc  (Pandoc Haskell SDK)
        images        │  extern "C" (src/image_ffi.rs)
            ──────────┴────────────▶  dd_magick_bridge (cpp/magick_bridge.cpp, compiled by cc)
                                              └▶ Magick++  (ImageMagick C++ SDK)
```

- `haskell/` — a `native-shared` `standalone` Cabal foreign library. `standalone`
  bundles the GHC RTS and Haskell dependencies into a single `.so` and initialises
  the RTS on load, so the Rust side never calls `hs_init`. `cabal.project` enables
  pandoc's `embed_data_files` flag so templates/data are baked into the library.
- `src/ffi.rs` — safe wrapper around the pandoc `ccall` entry points. Conversions
  cross the boundary as a JSON envelope (`{ "ok", "output" | "error" }`).
- `cpp/magick_bridge.cpp` — a thin `extern "C"` shim over Magick++ (Rust can't call
  C++ directly). `src/image_ffi.rs` is the safe wrapper. `build.rs` compiles the
  shim with `cc` and discovers Magick++ via `pkg-config`.
- All native calls run on `spawn_blocking` and are serialised behind a mutex.

### Feature flags / building without the native toolchains

Both bridges are on by default (`haskell-bridge`, `magick-bridge`). Disable either
to build/run without that toolchain; the matching endpoints then return `503`.

```bash
# pure-Rust unit tests, no GHC/ImageMagick needed:
cargo test --no-default-features
# image SDK only (needs ImageMagick + pkg-config), documents return 503:
cargo build --no-default-features --features magick-bridge
# full image (both bridges) — built from the repo ROOT so the libs/ path
# dependencies are in the build context:
docker build -f deployments/dd-document-rs/Dockerfile -t dd-document-rs .
```

### PDF generation (opt-in)

PDF can't use the `runPure` sandbox — pandoc's `makePDF` shells out to an engine
and uses temp files (`runIO`). To keep the default image lean it is **opt-in**:

```bash
# default: no PDF engine, /convert-pdf returns 503 (~560MB image)
docker build -f deployments/dd-document-rs/Dockerfile -t dd-document-rs .
# with PDF via Typst (~30MB, self-contained, no TeX, no network) -> ~640MB
docker build -f deployments/dd-document-rs/Dockerfile \
  --build-arg PDF_ENGINE=typst -t dd-document-rs .
```

`/status` reports `pdfEnabled`. The service detects the `typst` binary on PATH at
startup; if absent, the PDF routes return `503`.

### Image size

The default runtime image is ~560 MB (~640 MB with the Typst PDF engine). The
heavyweight is **not** ImageMagick (~78 MB of codec libs) but the Pandoc bridge
`.so` (~234 MB stripped): `standalone` statically bundles the entire GHC runtime
+ all of pandoc + embedded templates. The multi-stage build discards the GHC
toolchain and `libmagick++-dev`.

## Supported formats

Only **text-in / text-out** Pandoc formats are supported (see `GET /formats`).
Binary formats (`docx`, `pptx`, `odt`, `epub`, `pdf`) are rejected with a clear
error — they require binary I/O that this text bridge does not expose. Format
names may carry Pandoc extensions, e.g. `markdown+hard_line_breaks`.

## HTTP API

- `GET /healthz` — liveness/readiness probe.
- `GET /metrics` — Prometheus metrics.
- `GET /status` — bridge availability and linked Pandoc version.
- `GET /formats` — supported reader/writer catalog.
- `GET /capabilities` — limits, posture, and supported format counts.
- `GET /example` — a sample convert request.
- `POST /convert` — convert `{ from, to, content }` between text formats.
- `POST /to-ast` — parse `{ from, content }` into the Pandoc JSON AST.
- `POST /from-ast` — render a Pandoc JSON AST (`{ to, content }`) to a format.
- `POST /inspect` — structure report: block counts, headers, word/character counts.
- `POST /validate` — check a convert request (formats + size) without converting.
- `POST /image/convert` — change image format (`{ content_base64, format, quality?, strip? }`).
- `POST /image/transform` — crop / resize / rotate / grayscale / auto-orient / flatten / re-encode
  (`{ content_base64, format?, resize?, crop?, rotate?, quality?, strip?, grayscale?, auto_orient?, background? }`).
- `POST /image/identify` — image metadata (format, dimensions, depth, colorspace).
- `GET /image/formats` — supported output encoders.

### Documents: text vs binary vs streaming

| Path | Transport | Use for |
| --- | --- | --- |
| `POST /convert` | JSON, text in/out | text formats (md/html/latex/rst/...) |
| `POST /convert-binary` | JSON, base64 in/out | **binary formats** (docx/odt/pptx/epub) |
| `POST /stream/convert` | raw bytes in/out, params on headers | **big documents** — no base64 inflation |
| `POST /stream/image` | raw bytes in/out, params on headers | **big images** |
| `POST /convert-pdf` | JSON, base64 PDF out | **PDF** (opt-in engine) |

PDF is also reachable via `/stream/convert` with `x-to: pdf` (raw PDF bytes out).

`/convert` and `/convert-binary` also accept `standalone` (wrap in the format's
default template) and `metadata` (`{title, author:[...], date, ...}`).

Streaming requests carry parameters on headers (`x-from`, `x-to`, `x-standalone`,
`x-metadata`; images: `x-format`, `x-resize`, `x-crop`, `x-rotate`, `x-quality`,
`x-strip`, `x-grayscale`, `x-auto-orient`, `x-background`) and the raw file as the
body; the response body is the raw output with the right `content-type`. (Pandoc
and ImageMagick need the whole buffer, so this is binary transport with a high
ceiling — `DOCUMENT_MAX_STREAM_BYTES` — not chunk-by-chunk transform.)

Generated docs are served at `/docs/api`, `/api/docs`, and `/api/docs.json`.

### Image example

```bash
curl -s localhost:8122/image/convert -H 'content-type: application/json' -d "{
  \"format\": \"jpeg\", \"quality\": 70,
  \"content_base64\": \"$(base64 -i logo.png)\"
}"
# -> { "ok": true, "outputBytes": ..., "info": { "format": "JPEG", "width": .., "height": .. },
#      "content_base64": "<jpeg bytes>" }
```

### Example

```bash
curl -s localhost:8122/convert -H 'content-type: application/json' -d '{
  "from": "markdown",
  "to": "html",
  "content": "# Hello\n\nA *small* Pandoc document.\n"
}'
```

```json
{
  "ok": true,
  "requestId": "document-convert",
  "from": "markdown",
  "to": "html",
  "outputBytes": 61,
  "output": "<h1 id=\"hello\">Hello</h1>\n<p>A <em>small</em> Pandoc document.</p>\n",
  "generatedAtMs": 1700000000000
}
```

## NATS API

When `NATS_URL` is configured, the service queue-subscribes to
`dd.remote.document.convert` (queue group `dd-document-rs`) using the same body as
`POST /convert`. Results are published to `dd.remote.document.results`, lifecycle
events to `dd.remote.events`, and failures to `dd.remote.events.critical`.

## Configuration

| Env var | Default | Purpose |
| --- | --- | --- |
| `HOST` / `PORT` | `0.0.0.0` / `8122` | HTTP listener. |
| `DOCUMENT_MAX_INPUT_BYTES` | `4194304` | Max input for the JSON `/convert` path. |
| `DOCUMENT_MAX_OUTPUT_BYTES` | `16777216` | Max converted document size. |
| `DOCUMENT_MAX_IMAGE_BYTES` | `16777216` | Max input/output image size. |
| `DOCUMENT_MAX_STREAM_BYTES` | `67108864` | Max body for `/convert-binary` + streaming routes. |
| `DOCUMENT_CACHE_CAPACITY` | `256` | Conversion cache entries (0 disables). |
| `DOCUMENT_CACHE_MAX_ENTRY_BYTES` | `1048576` | Per-entry cache cap (bounds cache memory). |
| `DOCUMENT_IMAGE_CONCURRENCY` | `4` | Max concurrent image ops. |
| `DOCUMENT_CONVERT_CONCURRENCY` | `min(cpus,16)` | Max concurrent doc/PDF conversions. |
| `DOCUMENT_PDF_ENGINE` | `typst` | PDF engine binary to detect/use. |
| `DOCUMENT_CONVERT_SUBJECT` | `dd.remote.document.convert` | NATS request subject. |
| `DOCUMENT_RESULT_SUBJECT` | `dd.remote.document.results` | NATS result subject. |
| `DOCUMENT_EVENT_SUBJECT` | `dd.remote.events` | Lifecycle event subject. |
| `NATS_CRITICAL_EVENT_SUBJECT` | `dd.remote.events.critical` | Critical event subject. |
| `NATS_URL` | _(unset)_ | Enables the NATS worker when set. |

## Warm worker pool (heavy/big work)

For heavy conversions, dd-document-rs runs as a **warm image in `dd-container-pool`**
so there's no cold start. A pool entry is seeded in
[`databases/pg/seeds/container-pool-app-config.sql`](../../databases/pg/seeds/container-pool-app-config.sql)
(slug `dd-document`, image `dd-document-rs:latest`, `requestPath: /convert`,
higher size limits). The pool keeps `minWarm` containers ready and dispatches via
`POST /pools/dd-document/dispatch` or NATS
(`dd.remote.container_pool.dd-document.requests`); a dispatch may override `path`
to hit `/convert-binary`, `/inspect`, etc.

The pool's dispatch buffers and size-limits the response, so for **big payloads
stream directly** to the warm container's `/stream/convert` or `/stream/image`
(raw bytes, no base64). Throughput notes: pandoc conversions run lock-free
(`runPure` is pure) across `spawn_blocking` workers; image ops are bounded by
`DOCUMENT_IMAGE_CONCURRENCY`; identical requests are served from an in-memory
content-hash cache.

## Hardening

Document conversions:

- Run in Pandoc's **`runPure` sandbox** (the `PandocPure` monad) — no filesystem,
  network, environment, or clock access — so a hostile document can't read files
  or reach internal hosts (SSRF), regardless of reader/writer.
- **PDF** can't use `runPure` (the engine shells out), so the untrusted input is
  still **parsed in `runPure`**; only the rendering of our own AST runs in `runIO`.
  Raw passthrough nodes (raw typst/latex/html) are **stripped before PDF
  rendering** so a document can't smuggle engine code (e.g. typst `read()` /
  includes) into the `typst` process.
- Input/output sizes are capped; caller `request_id`s are length-bounded and
  control-char stripped.
- The conversion **cache key is SHA-256** over the length-prefixed request, so an
  attacker can't craft a colliding request to be served the wrong document; the
  cache is bounded by entry count **and** per-entry size.
- Concurrent conversions are bounded (`DOCUMENT_CONVERT_CONCURRENCY`) and image
  ops too (`DOCUMENT_IMAGE_CONCURRENCY`) to cap CPU/memory under load.
- The opt-in PDF engine download can be pinned with `--build-arg TYPST_SHA256=…`.

Image processing (defence in depth):

- The shipped `policy.xml` (`cpp/policy.xml`, installed at
  `MAGICK_CONFIGURE_PATH`) disables dangerous coders/delegates
  (`URL`/`HTTPS`/`MVG`/`MSL`/`PDF`/`PS`/`SVG`/`@file` indirection, …) and sets
  resource ceilings.
- The Magick++ shim sets `ResourceLimits` (memory/area/disk/time/dimensions) to
  blunt decompression bombs, and rejects decoded **input** coders outside a safe
  raster allowlist.
- Output formats are allowlisted; resize geometry and background colour are
  charset-validated; rotation/quality are range-checked; metadata is stripped by
  default.

## Deployment

ArgoCD manifests live in
[`../../argocd/dd-next-runtime`](../../argocd/dd-next-runtime):
`dd-document-rs.deployment.yaml`, `.service.yaml`, `.networkpolicy.yaml`. Unlike
the cargo-run siblings, the Deployment runs the image built from this `Dockerfile`
because the Haskell bridge must be compiled and linked ahead of time.
