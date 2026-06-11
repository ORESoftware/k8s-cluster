# dd-ocr-rs

Optical character recognition service. A thin Rust/[axum] front end that fans a
single HTTP + NATS API out to several OCR engines and normalises their output
into one response shape.

## Engines

| name           | kind                | backing library / API                         |
| -------------- | ------------------- | --------------------------------------------- |
| `tesseract`    | local, open-source  | Tesseract C++ API + Leptonica (FFI bridge)    |
| `google`       | paid, third-party   | Google Cloud Vision `images:annotate`         |
| `aws-textract` | paid, third-party   | AWS Textract `DetectDocumentText` (SigV4)     |
| `azure`        | paid, third-party   | Azure AI Vision 4.0 `read` feature            |

The **local** path is fully open-source: [Tesseract] (the Google-originated OCR
engine) and [Leptonica] do the recognition over an FFI bridge
(`cpp/tesseract_bridge.cpp`), and the pure-Rust [`image`] + [`imageproc`] crates
decode the input and Otsu-binarise it first (`src/preprocess.rs`) so Tesseract
reads a clean bi-level page.

The three **cloud** backends (`src/cloud.rs`) are called over HTTPS with
[`reqwest`]. AWS Textract is signed with a self-contained SigV4 implementation
(no AWS SDK). Each backend is enabled only when its credentials are present, so
the service degrades to whatever is configured instead of failing to boot;
`engine: auto` picks the first available one (local Tesseract first, then cloud).

## HTTP API

| route                    | method | purpose                                          |
| ------------------------ | ------ | ------------------------------------------------ |
| `/`                      | GET    | service banner + endpoint map                    |
| `/healthz`               | GET    | liveness/readiness probe                         |
| `/status`                | GET    | engine availability + config                     |
| `/engines`               | GET    | per-engine availability + provider               |
| `/capabilities`          | GET    | input formats, limits, NATS subjects             |
| `/example`               | GET    | a ready-to-POST sample request                   |
| `/ocr`                   | POST   | JSON `{ imageBase64, engine?, languages?, ... }` |
| `/ocr/stream`            | POST   | raw image body; knobs via query string           |
| `/metrics`               | GET    | Prometheus counters                              |
| `/docs/api`, `/api/docs` | GET    | generated API docs (see API Docs Contract)       |

```bash
# JSON
curl -s localhost:8123/ocr -H 'content-type: application/json' \
  -d '{"engine":"tesseract","languages":"eng","imageBase64":"<base64 png>"}'

# raw bytes
curl -s --data-binary @scan.png \
  'localhost:8123/ocr/stream?engine=auto&lang=eng&binarize=true'
```

## NATS

When `NATS_URL` is set the service also consumes OCR requests off
`dd.remote.ocr.requests` (queue group `dd-ocr-rs`) and publishes results to
`dd.remote.ocr.results`, with lifecycle/critical events on the shared
`dd.remote.events` / `dd.remote.events.critical` subjects.

## Configuration

| env                                            | default                 | meaning                                  |
| ---------------------------------------------- | ----------------------- | ---------------------------------------- |
| `PORT`                                         | `8123`                  | HTTP listen port                         |
| `OCR_MAX_IMAGE_BYTES`                          | `16777216`              | max accepted image payload               |
| `OCR_MAX_IMAGE_DIM`                            | `10000`                 | max decoded width/height (anti-bomb)     |
| `OCR_MAX_DECODE_ALLOC_BYTES`                   | `268435456`             | decoder allocation cap (anti-bomb)       |
| `OCR_DEFAULT_LANGUAGES`                        | `eng`                   | Tesseract language(s), e.g. `eng+deu`    |
| `OCR_CONCURRENCY`                              | CPU-derived             | concurrent OCR jobs                      |
| `OCR_UPSCALE_MIN_DIM`                          | `0` (off)               | upscale scans whose short side is below  |
| `OCR_HTTP_TIMEOUT_SECS`                        | `30`                    | cloud backend request timeout            |
| `GOOGLE_VISION_API_KEY`                        | —                       | enables the `google` engine              |
| `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`  | —                       | enables the `aws-textract` engine        |
| `AWS_REGION`                                   | `us-east-1`             | Textract region                          |
| `AZURE_VISION_ENDPOINT` / `AZURE_VISION_KEY`   | —                       | enables the `azure` engine               |
| `NATS_URL`                                     | —                       | enables the NATS worker                  |

Cloud credentials come from the `dd-ocr-rs-secrets` bundle (see
`remote/argocd/dd-next-runtime/dd-ocr-rs.externalsecret.yaml`).

## Security posture

Input is untrusted. The service:

- **Allowlists image formats** (png/jpeg/webp/tiff/bmp/gif) by magic-byte sniff
  *before* decoding or forwarding — the heavier `image` codecs (AVIF, OpenEXR,
  …) compiled in transitively by `imageproc` are never reached, and junk is
  rejected before any paid cloud call.
- **Caps decode work** with `image::Limits` (dimension + allocation), so a
  small "decompression bomb" can't exhaust memory; concurrency is bounded by a
  semaphore and the NATS worker sheds load rather than queueing unbounded.
- **Keeps credentials out of URLs/logs**: the Google key rides an
  `X-goog-api-key` header, transport errors are redacted, and config is logged
  only as booleans. AWS Textract is SigV4-signed (verified against AWS's
  official known-answer vector); the Azure endpoint must be `https`.
- **Restricts egress**: the NetworkPolicy blocks RFC1918, CGNAT, and the cloud
  metadata endpoint (IMDS), allowing only public `:443`; the HTTP client is
  `https_only`, no-redirect, with a connect timeout.
- Runs **non-root, read-only rootfs, all caps dropped, seccomp RuntimeDefault,
  no service-account token**, with a bounded `emptyDir` scratch.

Use IAM credentials scoped to `textract:DetectDocumentText` only;
temporary/STS credentials are supported via `AWS_SESSION_TOKEN`.

## Build

The crate links the native Tesseract bridge at build time, so it ships as a
built image rather than the cargo-run-from-hostPath pattern. Build from the repo
root so the `libs/` path-dependencies are in the context:

```bash
docker build -f deployments/dd-ocr-rs/Dockerfile -t dd-ocr-rs .
```

To compile without the native OCR toolchain (cloud engines only), disable the
default feature — the `tesseract` engine then reports unavailable:

```bash
cargo build --no-default-features
```

[axum]: https://github.com/tokio-rs/axum
[Tesseract]: https://github.com/tesseract-ocr/tesseract
[Leptonica]: http://www.leptonica.org/
[`image`]: https://crates.io/crates/image
[`imageproc`]: https://crates.io/crates/imageproc
[`reqwest`]: https://crates.io/crates/reqwest
