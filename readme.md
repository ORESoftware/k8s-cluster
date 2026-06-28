# akrion-web-server.rs

Rust web portal for Akrion Sim.

- `akrion-backend.rs` owns realtime game/simulation routes.
- `akrion-web-server.rs` owns web pages, portal UI, htmx fragments, WebSocket stats, and Supabase browser login.

## Layout

- `src/main.rs` boots tracing, state, router, socket binding, and graceful shutdown.
- `src/app.rs` owns app state, public browser config, environment helpers, and static asset paths.
- `src/routes.rs` owns axum routes, partial handlers, and the htmx WebSocket stream.
- `src/views.rs` owns maud page templates and reusable UI fragments.
- `src/data.rs` owns dashboard stats and placeholder portal rows.
- `assets/app.css` and `assets/app.js` own the browser UI layer.

## Run

```bash
PORT=8124 AKRION_BACKEND_URL=http://127.0.0.1:8113 cargo run
```

Then open `http://127.0.0.1:8124`.

To mount the app behind a path prefix, set `AKRION_WEB_BASE_PATH`:

```bash
PORT=8124 AKRION_WEB_BASE_PATH=/akrion-sim cargo run
```

## Supabase Login

The page enables Supabase auth when both values are present:

```bash
SUPABASE_URL=https://your-project.supabase.co
SUPABASE_ANON_KEY=your-public-anon-key
```

Only the public anon key is emitted to the browser. Do not put a Supabase service-role key in this server's public config.
