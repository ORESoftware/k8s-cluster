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
2. Store each identity key in a `Secret`; create high-entropy overlay and client
   auth Secrets; put public keys in the directory ConfigMap.
3. Apply the namespace, relays, directory, client, and NetworkPolicy.
4. Point in-cluster workloads at
   `socks5h://tor-client.anon-proxy.svc.cluster.local:9050`, and the dashboard at
   the `tor-client` service port `9060`.

### Notes for cluster networking

- Relay-to-relay `Extend` targets are private cluster IPs, so leave
  `TOR_EXIT_ALLOW_PRIVATE` **unset** on relays that only need to reach the public
  internet as exits. If an exit must reach an in-cluster service, set it to `1`
  on that relay (understanding the SSRF trade-off), or prefer a dedicated egress.
- The manifests mount the same `TOR_NETWORK_SECRET_FILE` on every relay and the
  client, pin `TOR_RELAY_PEERS`, require SOCKS/dashboard credentials, and limit
  ingress. Do not weaken those controls to publish a LoadBalancer on port 9050.
- Persist each relay's identity so its public key is stable across restarts
  (mount the key from a `Secret`, as the manifests do).

## Scaling relays

Add more relays to widen path diversity: create more keys/Secrets, add entries to
the directory `ConfigMap`, and raise `TOR_HOPS` if you want longer circuits
(needs at least `TOR_HOPS` relays in the directory).

## Cloud VM / EC2

The safe cloud pattern is to keep `TOR_SOCKS_LISTEN` loopback-only and reach it
through WireGuard or an SSH local forward. If a container must bind `0.0.0.0`,
publish it only on the host loopback/private interface, set
`TOR_SOCKS_ALLOW_REMOTE=1`, and configure RFC 1929 credentials as defense in
depth. Security groups should allow relay ports only between known relay hosts;
never expose the dashboard or SOCKS port to `0.0.0.0/0`.

This still proxies only TCP-aware applications. Use WireGuard itself (or another
real VPN) when the requirement is whole-device IP routing, UDP, DNS capture, or
a kill switch.
