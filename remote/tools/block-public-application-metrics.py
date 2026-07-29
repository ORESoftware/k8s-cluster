#!/usr/bin/env python3
"""Keep first-party /metrics endpoints scrapeable in-cluster but off public routes."""

from pathlib import Path


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {old!r}")
    path.write_text(text.replace(old, new))


gateway = Path("remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml")
replace_once(
    gateway,
    """      server_name app.fiducia.cloud;

      ssl_certificate /etc/nginx/tls/tls.crt;
      ssl_certificate_key /etc/nginx/tls/tls.key;
      ssl_protocols TLSv1.2 TLSv1.3;
      add_header Strict-Transport-Security \"max-age=15552000\" always;
      # proxy_pass uses a variable so nginx can re-resolve the Service after
      # endpoint churn; variable upstreams require an explicit DNS resolver.
      resolver kube-dns.kube-system.svc.cluster.local valid=10s ipv6=off;
      resolver_timeout 2s;

      location / {
""",
    """      server_name app.fiducia.cloud;

      ssl_certificate /etc/nginx/tls/tls.crt;
      ssl_certificate_key /etc/nginx/tls/tls.key;
      ssl_protocols TLSv1.2 TLSv1.3;
      add_header Strict-Transport-Security \"max-age=15552000\" always;
      # proxy_pass uses a variable so nginx can re-resolve the Service after
      # endpoint churn; variable upstreams require an explicit DNS resolver.
      resolver kube-dns.kube-system.svc.cluster.local valid=10s ipv6=off;
      resolver_timeout 2s;

      # Prometheus reaches this endpoint directly through the ClusterIP Service.
      # Never publish process or request metrics on the customer-facing origin.
      location ^~ /metrics {
        return 404;
      }

      location / {
""",
)
replace_once(
    gateway,
    """      location /akrion-sim/ {
""",
    """      # The portal has an operator-authenticated public prefix, but metrics
      # remain an in-cluster-only control-plane surface.
      location ^~ /akrion-sim/metrics {
        return 404;
      }

      location /akrion-sim/ {
""",
)

test_path = Path("remote/tests/general/prometheus-direct-scrape-contract.test.ts")
replace_once(
    test_path,
    """test(\"machine-readable inventory routes every discovered project workstream to Linear\", async () => {
""",
    """test(\"public gateway routes do not publish application metrics\", async () => {
  const gateway = await readRepoFile(
    \"remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml\",
  );

  assert.match(
    gateway,
    /server_name app\\.fiducia\\.cloud;[\\s\\S]*?location \\^~ \\/metrics \\{\\s*return 404;/,
  );
  assert.match(
    gateway,
    /location \\^~ \\/akrion-sim\\/metrics \\{\\s*return 404;/,
  );
  assert.doesNotMatch(
    gateway,
    /canonical-cloud-web\\.canonical-cloud\\.svc\\.cluster\\.local/,
    \"the dormant Canonical Service must not be publicly routed before activation\",
  );
  assert.doesNotMatch(
    gateway,
    /dd-sonus-auris-site\\.default\\.svc\\.cluster\\.local:9113/,
    \"the Sonus exporter port must never be exposed by the gateway\",
  );
});

test(\"machine-readable inventory routes every discovered project workstream to Linear\", async () => {
""",
)
