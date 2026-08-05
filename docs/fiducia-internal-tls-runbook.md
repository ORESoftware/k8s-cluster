# Fiducia internal TLS migration, rotation, and rollback runbook

Status: ESO-first verified-HTTPS stage implemented; every remaining direct client and final plaintext-port removal remain production gates.  
Tracks: DEN-438

## Trust model

cert-manager generates a private ECDSA CA in the `fiducia` namespace and stores its private key only in the `fiducia-internal-ca` Secret. The namespace Issuer signs a 30-day server certificate for the exact Fiducia load-balancer Service DNS names. The serving Secret contains `tls.crt`, `tls.key`, and the public `ca.crt` chain; no key or certificate bytes belong in Git, Linear, Actions output, Argo diffs, screenshots, or operator transcripts.

The server certificate covers:

- `fiducia-load-balance`;
- `fiducia-load-balance.fiducia`;
- `fiducia-load-balance.fiducia.svc`;
- `fiducia-load-balance.fiducia.svc.cluster.local`.

Clients must use one of those names and the pinned Fiducia CA. Connecting by Pod IP, ClusterIP, an unlisted alias, or a caller-controlled Host value is forbidden.

## Listener behavior during migration

With `FIDUCIA_TLS_CERT_PATH` and `FIDUCIA_TLS_KEY_PATH` present, the load balancer serves the application router on HTTPS port 8443. Its existing HTTP port 8088 remains only for `/healthz` and `/readyz`; every other request receives `426 Upgrade Required` and is neither redirected nor proxied. This avoids replaying credential-bearing bodies to a Host-derived location.

ESO is the first migrated client. Its `ClusterSecretStore/dd-fiducia-kv` uses the full service DNS name on 8443 and pins `ca.crt` from `Secret/fiducia-load-balance-tls`. NetworkPolicy gives the ESO controller no route to 8088.

Other audited clients remain a migration queue. Do not describe DEN-438 as complete until each one has a reviewed custom-CA path and verified-hostname evidence:

1. `fiducia-auth` durable API-key storage;
2. `dd-remote-gateway`;
3. `dd-contract-service`;
4. `dd-billing-server`;
5. `dd-build-server`;
6. legacy `dd-fabrication-server` before it is ever scaled above zero;
7. any newly discovered direct caller.

## Initial deployment gates

1. Confirm cert-manager and `ClusterIssuer/selfsigned` are Ready.
2. Sync the Fiducia application and wait for `Certificate/fiducia-internal-ca` and `Certificate/fiducia-load-balance-tls` to become Ready.
3. Confirm `Secret/fiducia-load-balance-tls` contains `tls.crt`, `tls.key`, and `ca.crt` without printing their values.
4. Confirm the load-balancer rollout becomes Ready and the HTTPS Service endpoint is populated.
5. From an allowed ESO-equivalent test pod, verify a request to `https://fiducia-load-balance.fiducia.svc.cluster.local:8443/healthz` succeeds only when the generated CA is supplied.
6. Verify the same request fails with an unknown CA and with a hostname mismatch such as the ClusterIP or an unlisted alias.
7. Verify a non-probe HTTP request to port 8088 returns `426 Upgrade Required` and is not proxied.
8. Refresh a representative `ExternalSecret` backed by `dd-fiducia-kv` and confirm it reaches Ready through verified HTTPS.

Never use `curl -k`, `danger_accept_invalid_certs`, disabled hostname checks, or an empty/custom trust store that accepts arbitrary certificates.

## Fault tests

Run these in a disposable namespace or controlled maintenance window. Record only status, bounded error class, certificate serial/fingerprint, and timestamps.

- **Unknown CA:** use an unrelated CA bundle. The handshake must fail before an HTTP request is sent.
- **Hostname mismatch:** connect using an unlisted DNS name or IP while keeping the Fiducia CA. The handshake must fail.
- **Expired certificate:** issue a short-lived test certificate or validate with a controlled test clock. The handshake must fail.
- **Malformed or missing serving Secret:** stage a deployment with an absent/incomplete Secret. The pod must not become Ready; it must not silently fall back to full plaintext proxying.
- **Plaintext downgrade:** send a credential-bearing request to port 8088. It must return 426 and must not reach a Fiducia node.
- **NetworkPolicy:** an ESO-labelled test pod may reach 8443 but not 8088; an unlabelled namespace may reach neither.

## Rotation with overlap

The leaf certificate renews ten days before expiry. Because the current Rust listener loads PEM files at process start, a certificate Secret update alone does not hot-reload the listener. Rotation therefore uses an overlap and controlled rollout:

1. Observe the existing certificate serial and `notAfter` without printing private material.
2. Wait for cert-manager to publish the renewed Secret and confirm the new certificate chains to the currently trusted CA.
3. Keep the CA unchanged during ordinary leaf rotation, so old and new leaf certificates are simultaneously trusted.
4. Trigger a rolling restart of `Deployment/fiducia-load-balance`; `maxUnavailable: 0` keeps at least one old-certificate pod available while a new-certificate pod becomes Ready.
5. Confirm clients can connect throughout the overlap and that new connections eventually report the new serial/fingerprint.
6. Roll back the Deployment revision if the new pod cannot complete verified handshakes. The previous pod and certificate remain trusted during the overlap.

For a **CA rotation**, publish a trust bundle containing old and new CA certificates first, migrate every direct client, issue/roll the server certificate from the new CA, verify all clients, then remove the old CA in a separate change. Never rotate the CA and leaf with no overlap.

## Alerting and evidence

Required production signals:

- certificate Ready condition false;
- certificate expiry thresholds at 14, 7, 3, and 1 day;
- TLS listener startup/read failures;
- handshake failures grouped by bounded reason such as unknown CA, expiry, protocol, or hostname mismatch;
- sustained HTTP 426 responses, which indicate an un-migrated or downgrade-attempting client;
- ESO `ClusterSecretStore` or `ExternalSecret` Ready failures;
- rollout stuck after a certificate renewal.

Do not log private keys, bearer values, API keys, complete request headers, or unbounded certificate bodies.

## Final plaintext removal production gate

After every direct client uses verified HTTPS and the observability probes have either moved to HTTPS or a separately isolated health-only Service, complete a separate reviewed cutover:

1. remove port 8088 from the application-facing Service;
2. remove port 8088 from every application-client NetworkPolicy rule;
3. retain only an explicitly isolated health listener if operationally required, with no route to the application router;
4. assert rendered manifests contain no `http://fiducia-load-balance` client endpoints;
5. prove plaintext application traffic is unreachable, not merely rejected;
6. run the full valid/unknown-CA/expiry/hostname/downgrade/rotation suite;
7. attach bounded evidence and rollback steps to DEN-438.

DEN-438 remains open until this production gate is complete.
