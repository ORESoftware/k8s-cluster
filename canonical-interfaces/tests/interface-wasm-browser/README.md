# Generated interface browser contract

This harness certifies the artifact produced by:

```sh
wasm-pack build generated/rust-wasm --target web --dev -- --locked
```

It is intentionally dependency-free. `run.sh` serves the repository on loopback, launches a Chromium-family browser with external DNS blocked, and inspects the dumped DOM for a passing contract result.

The browser module imports and initializes the actual generated JavaScript/WebAssembly package, fetches its generated declarations, and verifies:

- the WebAssembly bytes are non-empty and initialize successfully;
- representative schema types remain present in the declarations;
- no `bigint`, `Map`, or unresolved `Value` type leaks into the browser contract;
- the declaration-oriented package exposes no runtime network, mutation, storage, cookie, WebSocket, beacon, or probe API;
- all browser resources stay on the loopback origin.

The harness must not call a live Canonical endpoint, add browser-only behavior to the generated schema package, or replace locked CI tools with mutable registry resolution.
