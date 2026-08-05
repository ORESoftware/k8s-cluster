# dd-meshy-client

Typed Rust client and command-line adapter for using Meshy's image-to-3D APIs as an upstream Daedalus geometry provider.

The client supports:

- single-image and multi-image task creation;
- task retrieval, listing, bounded polling, and deletion;
- GLB, OBJ, FBX, STL, USDZ, and explicit 3MF output requests;
- request validation before credits are consumed;
- redacted bearer credentials;
- a Daedalus candidate-geometry envelope that always remains `machineReady: false` until the normal fabrication gates clear.

## Run

```bash
export MESHY_API_KEY='...'

cargo run \
  --manifest-path crates/meshy-client/Cargo.toml \
  --bin dd-meshy-adapter \
  -- capabilities

cargo run \
  --manifest-path crates/meshy-client/Cargo.toml \
  --bin dd-meshy-adapter \
  -- create-image examples/meshy/image-to-3d.json
```

See `docs/meshy-integration.md` for architecture, deployment, and release-boundary details.
