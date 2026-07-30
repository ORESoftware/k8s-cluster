#!/usr/bin/env python3
"""Apply the large DEN-666 Prometheus and gateway edits exactly once."""

from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {old!r}")
    file.write_text(text.replace(old, new))


prometheus = "remote/argocd/observability/prometheus.configmap.yaml"
replace_once(
    prometheus,
    """      - job_name: dd-webrtc-signaling
        metrics_path: /metrics
""",
    """      # DEN-666 first-party application endpoints. These direct scrapes
      # complement the existing OTLP pipeline and retain the global 15s cadence.
      - job_name: fiducia-backend
        metrics_path: /metrics
        static_configs:
          - targets:
              - fiducia-backend.fiducia.svc.cluster.local:8117
      - job_name: canonical-cloud-web
        metrics_path: /metrics
        static_configs:
          - targets:
              - canonical-cloud-web.canonical-cloud.svc.cluster.local:8081
      - job_name: dd-akrion-web-server-rs
        metrics_path: /metrics
        static_configs:
          - targets:
              - dd-akrion-web-server-rs.default.svc.cluster.local:8127
      - job_name: dd-sonus-auris-site
        metrics_path: /metrics
        static_configs:
          - targets:
              - dd-sonus-auris-site.default.svc.cluster.local:9113
      - job_name: dd-webrtc-signaling
        metrics_path: /metrics
""",
)
replace_once(
    prometheus,
    """          - alert: DDPromtailTargetMissing
""",
    """          - alert: DDFirstPartyApplicationMetricsTargetDown
            expr: up{job=~\"fiducia-backend|canonical-cloud-web|dd-akrion-web-server-rs|dd-sonus-auris-site\"} == 0
            for: 2m
            labels:
              severity: warning
              service: first-party-applications
            annotations:
              summary: \"Application metrics target {{ $labels.job }} is down\"
              description: \"Prometheus cannot scrape {{ $labels.instance }}/metrics for {{ $labels.job }}.\"
          - alert: DDSonusAurisNginxExporterScrapeFailed
            expr: absent(nginx_up{job=\"dd-sonus-auris-site\"}) or nginx_up{job=\"dd-sonus-auris-site\"} == 0
            for: 2m
            labels:
              severity: warning
              service: dd-sonus-auris-site
            annotations:
              summary: Sonus Auris nginx exporter cannot scrape nginx
              description: \"The exporter endpoint is reachable but nginx_up is absent or zero; verify the loopback stub_status route.\"
          - alert: DDPromtailTargetMissing
""",
)

gateway = "remote/argocd/dd-next-runtime/dd-remote-gateway.configmap.yaml"
replace_once(
    gateway,
    """    server {
      listen 443 ssl;
      server_name app.fiducia.cloud;

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
    """    server {
      listen 443 ssl;
      server_name app.fiducia.cloud;

      ssl_certificate /etc/nginx/tls/tls.crt;
      ssl_certificate_key /etc/nginx/tls/tls.key;
      ssl_protocols TLSv1.2 TLSv1.3;
      add_header Strict-Transport-Security \"max-age=15552000\" always;
      # proxy_pass uses a variable so nginx can re-resolve the Service after
      # endpoint churn; variable upstreams require an explicit DNS resolver.
      resolver kube-dns.kube-system.svc.cluster.local valid=10s ipv6=off;
      resolver_timeout 2s;

      # Prometheus scrapes the ClusterIP Service directly. Do not publish
      # process/request telemetry on the customer-facing Fiducia origin.
      location ^~ /metrics {
        return 404;
      }

      location / {
""",
)
replace_once(
    gateway,
    """      location /akrion-sim/ {
        if ($dd_gateway_auth_ok = 0) {
""",
    """      # Keep Akrion process metrics on the in-cluster Service even for
      # authenticated gateway users.
      location ^~ /akrion-sim/metrics {
        return 404;
      }

      location /akrion-sim/ {
        if ($dd_gateway_auth_ok = 0) {
""",
)
