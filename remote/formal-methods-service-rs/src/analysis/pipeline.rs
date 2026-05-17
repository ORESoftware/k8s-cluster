//! Orchestrates a sequence of [`Analyzer`]s against a single PR head commit.

use std::sync::Arc;

use anyhow::Result;
use tracing::{info, info_span, Instrument};

use crate::analysis::cargo::{CargoCheckAnalyzer, CargoTestAnalyzer, ProptestAnalyzer};
use crate::analysis::certora::CertoraAnalyzer;
use crate::analysis::dreal::DRealAnalyzer;
use crate::analysis::kani::KaniAnalyzer;
use crate::analysis::verus::VerusAnalyzer;
use crate::analysis::workspace::{checkout, CheckoutSpec};
use crate::analysis::{Analyzer, Report};
use crate::config::Config;

#[derive(Debug)]
pub struct Pipeline {
    analyzers: Vec<Arc<dyn Analyzer>>,
}

impl Pipeline {
    pub fn from_config(cfg: &Config) -> Self {
        let mut analyzers: Vec<Arc<dyn Analyzer>> = Vec::new();

        if cfg.enable_cargo_check {
            analyzers.push(Arc::new(CargoCheckAnalyzer {
                manifest_path: cfg.contract_manifest_path.clone(),
                timeout: cfg.analyzer_timeout,
            }));
        }
        if cfg.enable_cargo_test {
            analyzers.push(Arc::new(CargoTestAnalyzer {
                manifest_path: cfg.contract_manifest_path.clone(),
                package: cfg.cargo_test_package.clone(),
                features: cfg.cargo_test_features.clone(),
                timeout: cfg.analyzer_timeout,
            }));
        }
        if cfg.enable_proptest {
            analyzers.push(Arc::new(ProptestAnalyzer {
                manifest_path: cfg.contract_manifest_path.clone(),
                package: cfg
                    .cargo_test_package
                    .clone()
                    .unwrap_or_else(|| "om-core".into()),
                test_target: cfg.proptest_test_target.clone(),
                timeout: cfg.analyzer_timeout,
            }));
        }
        if cfg.enable_kani {
            analyzers.push(Arc::new(KaniAnalyzer {
                manifest_path: cfg.contract_manifest_path.clone(),
                package: cfg
                    .cargo_test_package
                    .clone()
                    .unwrap_or_else(|| "om-core".into()),
                timeout: cfg.analyzer_timeout,
            }));
        }
        if cfg.enable_verus {
            analyzers.push(Arc::new(VerusAnalyzer {
                proof_crate_dir: cfg.verus_proof_crate_dir.clone(),
                timeout: cfg.analyzer_timeout,
            }));
        }
        if cfg.enable_dreal {
            analyzers.push(Arc::new(DRealAnalyzer {
                queries_dir: cfg.dreal_queries_dir.clone(),
                precision: cfg.dreal_precision,
                timeout: cfg.analyzer_timeout,
            }));
        }
        // Certora is always wired in but defaults to disabled. Treats every
        // invocation as `Skipped` until explicitly enabled and configured.
        analyzers.push(Arc::new(CertoraAnalyzer {
            enabled: cfg.enable_certora,
            conf_dir: cfg.certora_conf_dir.clone(),
            timeout: cfg.analyzer_timeout,
        }));

        Self { analyzers }
    }

    pub fn analyzer_names(&self) -> Vec<&str> {
        self.analyzers.iter().map(|a| a.name()).collect()
    }

    pub async fn run(&self, checkout_spec: CheckoutSpec) -> Result<Report> {
        let span = info_span!(
            "analysis",
            repo = %checkout_spec.repo_full_name,
            sha = %checkout_spec.head_sha,
        );

        async move {
            let ws = checkout(checkout_spec).await?;
            let mut steps = Vec::with_capacity(self.analyzers.len());
            for analyzer in &self.analyzers {
                info!(name = analyzer.name(), "running analyzer");
                let report = analyzer.run(&ws).await;
                info!(
                    name = %report.name,
                    status = ?report.status,
                    duration_ms = report.duration.as_millis(),
                    "analyzer finished"
                );
                steps.push(report);
            }
            Ok(Report { steps })
        }
        .instrument(span)
        .await
    }
}
