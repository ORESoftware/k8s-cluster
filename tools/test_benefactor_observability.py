"""Static contracts for Benefactor's central Prometheus wiring."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
PROMETHEUS = ROOT / "remote/argocd/observability/prometheus.configmap.yaml"
SERVICE = (
    ROOT
    / "remote/argocd/benefactor-backend-rs"
    / "benefactor-backend-rs.service.yaml"
)
PROMETHEUS_DEPLOYMENT = (
    ROOT / "remote/argocd/observability/prometheus.deployment.yaml"
)


class BenefactorObservabilityContractTest(unittest.TestCase):
    def test_prometheus_scrapes_the_backend_service(self) -> None:
        manifest = PROMETHEUS.read_text(encoding="utf-8")

        self.assertIn("- job_name: benefactor-backend-rs", manifest)
        self.assertIn(
            "- benefactor-backend-rs.default.svc.cluster.local:80",
            manifest,
        )
        self.assertIn('absent(up{job="benefactor-backend-rs"})', manifest)
        self.assertIn('up{job="benefactor-backend-rs"} == 0', manifest)
        self.assertIn("BenefactorBackendDependencyNotReady", manifest)
        self.assertIn("BenefactorBackendPipelineFailuresIncreasing", manifest)
        self.assertIn("BenefactorBackendCpuNearLimit", manifest)
        self.assertIn("BenefactorBackendMemoryNearLimit", manifest)

        deployment = PROMETHEUS_DEPLOYMENT.read_text(encoding="utf-8")
        self.assertRegex(
            deployment,
            re.compile(
                r'dd\.dev/config-revision: "[0-9]{4}-[0-9]{2}-[0-9]{2}-[a-z0-9-]+"'
            ),
        )

    def test_service_exposes_the_annotated_metrics_port(self) -> None:
        manifest = SERVICE.read_text(encoding="utf-8")

        self.assertIn("prometheus.io/path: /metrics", manifest)
        self.assertIn("prometheus.io/port: '80'", manifest)
        self.assertIn("prometheus.io/scrape: 'true'", manifest)
        self.assertIn("port: 80", manifest)
        self.assertIn("targetPort: http", manifest)


if __name__ == "__main__":
    unittest.main()
