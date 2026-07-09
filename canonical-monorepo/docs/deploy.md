# Deploy

The canonical.cloud stack is a single Rust backend that serves a prebuilt Astro
site. Two ways to ship it:

## 1. Single backend image (site baked in)

```sh
git submodule update --init --recursive
./build.sh                      # builds the site, copies it to backend/static,
                                # then cargo build --release
```

Then containerize the backend with `apps/canonical-backend.rs/Dockerfile`
(distroless, nonroot). Provide the built site at `STATIC_DIR` (default
`/app/static`) — either baked into the image or mounted at runtime.

```sh
docker build -t canonical-backend apps/canonical-backend.rs
docker run -p 8081:8081 -v "$PWD/apps/canonical-backend.rs/static:/app/static:ro" canonical-backend
```

## 2. Split (nginx site + backend API)

Serve the static site from the frontend's nginx image
(`apps/canonical-frontend/Dockerfile`) and run the backend for `/api/*` +
`/healthz`, routing at the edge. Point the frontend build's base at the public
path with `PUBLIC_BASE`.

## Health checks

- Liveness/readiness: `GET /healthz` → `200 OK` (dependency-free).
- App status: `GET /api/health` and `GET /api/info`.

## Pinning for a release

```sh
scripts/pin-submodules.sh main
git commit -m "Pin canonical apps to main"
```

The committed superproject SHA is the deployable release: it references exact
app commits, so a checkout + `./build.sh` reproduces the shipped stack.
