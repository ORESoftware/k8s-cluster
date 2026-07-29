#!/usr/bin/env python3
"""Materialize DEN-666 first-wave Prometheus wiring on its feature branch."""

from pathlib import Path


def replace_once(relative: str, old: str, new: str) -> None:
    path = Path(relative)
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(
            f"{relative}: expected one anchor, found {count}: {old!r}"
        )
    path.write_text(text.replace(old, new))


replace_once(
    "remote/argocd/observability/prometheus.configmap.yaml",
    "      - job_name: dd-webrtc-signaling\n"
    "        metrics_path: /metrics\n",
    "      # First-party application scrape contract (DEN-666). These jobs\n"
    "      # intentionally coexist with OTLP export through the shared collector.\n"
    "      - job_name: fiducia-backend\n"
    "        metrics_path: /metrics\n"
    "        static_configs:\n"
    "          - targets:\n"
    "              - fiducia-backend.fiducia.svc.cluster.local:8117\n"
    "      - job_name: canonical-cloud-web\n"
    "        metrics_path: /metrics\n"
    "        static_configs:\n"
    "          - targets:\n"
    "              - canonical-cloud-web.canonical-cloud.svc.cluster.local:8081\n"
    "      - job_name: dd-akrion-web-server-rs\n"
    "        metrics_path: /metrics\n"
    "        static_configs:\n"
    "          - targets:\n"
    "              - dd-akrion-web-server-rs.default.svc.cluster.local:8127\n"
    "      - job_name: dd-sonus-auris-site\n"
    "        metrics_path: /metrics\n"
    "        static_configs:\n"
    "          - targets:\n"
    "              - dd-sonus-auris-site.default.svc.cluster.local:9113\n"
    "      - job_name: dd-webrtc-signaling\n"
    "        metrics_path: /metrics\n",
)
replace_once(
    "remote/argocd/observability/prometheus.configmap.yaml",
    "          - alert: DDPromtailTargetMissing\n",
    "          - alert: DDFirstPartyApplicationMetricsTargetDown\n"
    "            expr: up{job=~\"fiducia-backend|canonical-cloud-web|dd-akrion-web-server-rs|dd-sonus-auris-site\"} == 0\n"
    "            for: 2m\n"
    "            labels:\n"
    "              severity: warning\n"
    "              service: first-party-applications\n"
    "            annotations:\n"
    "              summary: \"Application metrics target {{ $labels.job }} is down\"\n"
    "              description: \"Prometheus cannot scrape {{ $labels.instance }}/metrics for {{ $labels.job }}.\"\n"
    "          - alert: DDPromtailTargetMissing\n",
)

replace_once(
    "remote/argocd/fiducia/fiducia-backend.deployment.yaml",
    "        dd.dev/fiducia-source: 'fiducia-cloud/fiducia-monorepo@main (apps/fiducia-customer.rs + apps/fiducia-marketing.web)'\n"
    "        dd.dev/fiducia-build: '2026-06-27-raft-locks-merge'\n",
    "        dd.dev/fiducia-source: 'fiducia-cloud/fiducia-monorepo@main (apps/fiducia-customer.rs + apps/fiducia-marketing.web)'\n"
    "        dd.dev/fiducia-build: '2026-06-27-raft-locks-merge'\n"
    "        prometheus.io/scrape: 'true'\n"
    "        prometheus.io/path: '/metrics'\n"
    "        prometheus.io/port: '8117'\n",
)
replace_once(
    "remote/argocd/fiducia/fiducia-backend.deployment.yaml",
    "---\n"
    "apiVersion: v1\n"
    "kind: Service\n"
    "metadata:\n"
    "  name: fiducia-backend\n"
    "  namespace: fiducia\n"
    "  labels:\n"
    "    app: fiducia-backend\n"
    "spec:\n",
    "---\n"
    "apiVersion: v1\n"
    "kind: Service\n"
    "metadata:\n"
    "  name: fiducia-backend\n"
    "  namespace: fiducia\n"
    "  labels:\n"
    "    app: fiducia-backend\n"
    "  annotations:\n"
    "    prometheus.io/scrape: 'true'\n"
    "    prometheus.io/path: '/metrics'\n"
    "    prometheus.io/port: '8117'\n"
    "spec:\n",
)

replace_once(
    "remote/argocd/canonical-cloud/web.deployment.yaml",
    "      annotations:\n"
    "        canonical.cloud/release-sha: \"e245ed408810455b7a0c43b9f4e81fd60b172100\"\n",
    "      annotations:\n"
    "        canonical.cloud/release-sha: \"e245ed408810455b7a0c43b9f4e81fd60b172100\"\n"
    "        prometheus.io/scrape: 'true'\n"
    "        prometheus.io/path: '/metrics'\n"
    "        prometheus.io/port: '8081'\n",
)
replace_once(
    "remote/argocd/canonical-cloud/web.service.yaml",
    "  labels:\n"
    "    app.kubernetes.io/name: canonical-web-server\n"
    "    app.kubernetes.io/component: web\n"
    "spec:\n",
    "  labels:\n"
    "    app.kubernetes.io/name: canonical-web-server\n"
    "    app.kubernetes.io/component: web\n"
    "  annotations:\n"
    "    prometheus.io/scrape: 'true'\n"
    "    prometheus.io/path: '/metrics'\n"
    "    prometheus.io/port: '8081'\n"
    "spec:\n",
)

replace_once(
    "remote/argocd/dd-next-runtime/dd-akrion-web-server-rs.deployment.yaml",
    "      annotations:\n"
    "        dd.dev/akrion-web-server-revision: 'f692449e7fc3b1c376e5c02a567fd5803c7d388d'\n",
    "      annotations:\n"
    "        dd.dev/akrion-web-server-revision: 'f692449e7fc3b1c376e5c02a567fd5803c7d388d'\n"
    "        prometheus.io/scrape: 'true'\n"
    "        prometheus.io/path: '/metrics'\n"
    "        prometheus.io/port: '8127'\n",
)
replace_once(
    "remote/argocd/dd-next-runtime/dd-akrion-web-server-rs.service.yaml",
    "  labels:\n"
    "    app: dd-akrion-web-server-rs\n"
    "spec:\n",
    "  labels:\n"
    "    app: dd-akrion-web-server-rs\n"
    "  annotations:\n"
    "    prometheus.io/scrape: 'true'\n"
    "    prometheus.io/path: '/metrics'\n"
    "    prometheus.io/port: '8127'\n"
    "spec:\n",
)

replace_once(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.configmap.yaml",
    "      location / {\n"
    "        try_files $uri $uri/ /index.html;\n"
    "      }\n",
    "      # Only the exporter sidecar may read nginx connection counters.\n"
    "      location = /stub_status {\n"
    "        access_log off;\n"
    "        allow 127.0.0.1;\n"
    "        deny all;\n"
    "        stub_status;\n"
    "      }\n"
    "\n"
    "      location / {\n"
    "        try_files $uri $uri/ /index.html;\n"
    "      }\n",
)
replace_once(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.deployment.yaml",
    "      annotations:\n"
    "        dd.dev/sonus-auris-site-revision: '2f94949c6a36533cec3cfe36022f7f9da50648de'\n",
    "      annotations:\n"
    "        dd.dev/sonus-auris-site-revision: '2f94949c6a36533cec3cfe36022f7f9da50648de'\n"
    "        prometheus.io/scrape: 'true'\n"
    "        prometheus.io/path: '/metrics'\n"
    "        prometheus.io/port: '9113'\n",
)
replace_once(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.deployment.yaml",
    "            - name: nginx-tmp\n"
    "              mountPath: /tmp\n"
    "      volumes:\n",
    "            - name: nginx-tmp\n"
    "              mountPath: /tmp\n"
    "        - name: nginx-prometheus-exporter\n"
    "          image: docker.io/nginx/nginx-prometheus-exporter:1.5.1\n"
    "          imagePullPolicy: IfNotPresent\n"
    "          args:\n"
    "            - --nginx.scrape-uri=http://127.0.0.1:8080/stub_status\n"
    "            - --web.listen-address=:9113\n"
    "            - --log.format=json\n"
    "          securityContext:\n"
    "            allowPrivilegeEscalation: false\n"
    "            readOnlyRootFilesystem: true\n"
    "            runAsNonRoot: true\n"
    "            runAsUser: 65534\n"
    "            runAsGroup: 65534\n"
    "            capabilities:\n"
    "              drop:\n"
    "                - ALL\n"
    "            seccompProfile:\n"
    "              type: RuntimeDefault\n"
    "          ports:\n"
    "            - name: metrics\n"
    "              containerPort: 9113\n"
    "              protocol: TCP\n"
    "          resources:\n"
    "            requests:\n"
    "              cpu: 5m\n"
    "              memory: 16Mi\n"
    "            limits:\n"
    "              cpu: 100m\n"
    "              memory: 64Mi\n"
    "          startupProbe:\n"
    "            httpGet:\n"
    "              path: /metrics\n"
    "              port: metrics\n"
    "            periodSeconds: 5\n"
    "            failureThreshold: 12\n"
    "          readinessProbe:\n"
    "            httpGet:\n"
    "              path: /metrics\n"
    "              port: metrics\n"
    "            periodSeconds: 10\n"
    "            timeoutSeconds: 3\n"
    "            failureThreshold: 3\n"
    "          livenessProbe:\n"
    "            httpGet:\n"
    "              path: /metrics\n"
    "              port: metrics\n"
    "            periodSeconds: 30\n"
    "            timeoutSeconds: 3\n"
    "            failureThreshold: 3\n"
    "      volumes:\n",
)
replace_once(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.service.yaml",
    "  labels:\n"
    "    app: dd-sonus-auris-site\n"
    "spec:\n",
    "  labels:\n"
    "    app: dd-sonus-auris-site\n"
    "  annotations:\n"
    "    prometheus.io/scrape: 'true'\n"
    "    prometheus.io/path: '/metrics'\n"
    "    prometheus.io/port: '9113'\n"
    "spec:\n",
)
replace_once(
    "remote/argocd/dd-next-runtime/dd-sonus-auris-site.service.yaml",
    "    - name: http\n"
    "      port: 8080\n"
    "      targetPort: http\n",
    "    - name: http\n"
    "      port: 8080\n"
    "      targetPort: http\n"
    "    - name: metrics\n"
    "      port: 9113\n"
    "      targetPort: metrics\n"
    "      protocol: TCP\n"
    "      appProtocol: http\n",
)

replace_once(
    "remote/tests/package.json",
    '    "test:cli:observability-config": "pnpm exec tsx --test general/observability-config.test.ts",\n',
    '    "test:cli:observability-config": "pnpm exec tsx --test general/observability-config.test.ts",\n'
    '    "test:cli:prometheus-direct-scrape": "pnpm exec tsx --test general/prometheus-direct-scrape-contract.test.ts",\n',
)
