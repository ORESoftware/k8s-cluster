# Deployment

## Container

```sh
docker build -t oresoftware/tor-server:0.1.0 .
docker push oresoftware/tor-server:0.1.0
```

The image bundles the binary, CA certificates, and the `docs/` folder (served at
`/docs`). It runs as a non-root user with a read-only root filesystem.

## Kubernetes

The `k8s/` folder deploys a deterministic 3-relay overlay plus a client. See
`k8s/README.md` for the step-by-step. The bootstrap order matters because the
client's directory must list each relay's public key:

1. Generate three relay keys with `tor-server keygen`; note each `pubkey`.
2. Store each secret key in a `Secret`; put the public keys in the directory
   `ConfigMap` (`k8s/directory-configmap.example.yaml`).
3. `kubectl apply` the namespace, relays, directory, and client.
4. Point in-cluster workloads at
   `socks5h://tor-client.anon-proxy.svc.cluster.local:9050`, and the dashboard at
   the `tor-client` service port `9060`.

### Notes for cluster networking

- Relay-to-relay `Extend` targets are private cluster IPs, so leave
  `TOR_EXIT_ALLOW_PRIVATE` **unset** on relays that only need to reach the public
  internet as exits. If an exit must reach an in-cluster service, set it to `1`
  on that relay (understanding the SSRF trade-off), or prefer a dedicated egress.
- Set `TOR_NETWORK_SECRET` (from a `Secret`) on every relay and the client to
  make the overlay closed.
- Persist each relay's identity so its public key is stable across restarts
  (mount the key from a `Secret`, as the manifests do).

## Scaling relays

Add more relays to widen path diversity: create more keys/Secrets, add entries to
the directory `ConfigMap`, and raise `TOR_HOPS` if you want longer circuits
(needs at least `TOR_HOPS` relays in the directory).
