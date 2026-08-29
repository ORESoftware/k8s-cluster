use serde_json::{json, Value};

use crate::state::{AppState, SCHEMA_VERSION, SERVICE_NAME};

pub(crate) fn source_catalog() -> Vec<Value> {
    vec![
        json!({
            "slug": "data-gov",
            "name": "Data.gov",
            "baseUrl": "https://data.gov",
            "kind": "government-open-data",
            "defaultStrategy": "native-fetch",
            "notes": "Catalog/API source for public US government datasets."
        }),
        json!({
            "slug": "science-gov",
            "name": "Science.gov",
            "baseUrl": "https://www.science.gov",
            "kind": "government-science-search",
            "defaultStrategy": "cheerio",
            "notes": "Federated science search and agency research discovery."
        }),
        json!({
            "slug": "pubmed",
            "name": "PubMed",
            "baseUrl": "https://pubmed.ncbi.nlm.nih.gov",
            "kind": "biomedical-literature",
            "defaultStrategy": "native-fetch",
            "notes": "Biomedical article metadata, abstracts, MeSH topics, and trend signals."
        }),
        json!({
            "slug": "state-libraries",
            "name": "State libraries",
            "baseUrl": "varies",
            "kind": "state-public-records",
            "defaultStrategy": "auto",
            "notes": "State-level archives, library catalogs, local reports, and historical collections."
        }),
        json!({
            "slug": "plos",
            "name": "PLOS",
            "baseUrl": "https://plos.org",
            "kind": "open-access-research",
            "defaultStrategy": "native-fetch",
            "notes": "Open-access research articles for evidence synthesis."
        }),
        json!({
            "slug": "propublica",
            "name": "ProPublica",
            "baseUrl": "https://www.propublica.org",
            "kind": "public-interest-investigations",
            "defaultStrategy": "cheerio",
            "notes": "Investigative datasets, nonprofit data, and public-interest reporting."
        }),
        json!({
            "slug": "cambridge-analytics",
            "name": "Cambridge analytics / Cambridge research sources",
            "baseUrl": "varies",
            "kind": "research-and-analytics",
            "defaultStrategy": "auto",
            "notes": "Placeholder catalog slot for approved Cambridge-linked public analytics/research sources."
        }),
        json!({
            "slug": "sbir",
            "name": "SBIR.gov",
            "baseUrl": "https://www.sbir.gov",
            "kind": "grant-opportunities",
            "defaultStrategy": "cheerio",
            "notes": "Small Business Innovation Research funding opportunities and award data."
        }),
        json!({
            "slug": "pew-research",
            "name": "Pew Research Center",
            "baseUrl": "https://www.pewresearch.org",
            "kind": "survey-and-social-trends",
            "defaultStrategy": "cheerio",
            "notes": "Survey reports, public opinion trends, and social-science datasets."
        }),
    ]
}

pub(crate) fn service_descriptor(state: &AppState) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": SCHEMA_VERSION,
        "description": "Rust public-data ingestion, webhook, scraper-orchestration, grants, analysis, graph-data, white-paper evidence, and Spark/Airflow handoff service.",
        "scraperBaseUrl": state.config.scraper_base_url,
        "auth": {
            "operatorAuth": "X-Server-Auth or Auth",
            "webhookAuth": "X-Public-Data-Webhook-Secret when PUBLIC_DATA_WEBHOOK_SECRET is configured",
            "allowUnauthenticated": state.config.allow_unauthenticated,
            "allowUnauthenticatedWebhooks": state.config.allow_unauthenticated_webhooks
        },
        "subjects": {
            "ingestRequests": state.config.ingest_request_subject,
            "ingestResults": state.config.ingest_result_subject,
            "webhookEvents": state.config.webhook_event_subject,
            "pipelineJobs": state.config.pipeline_job_subject,
            "analysisResults": state.config.analysis_result_subject,
            "runtimeEvents": state.config.runtime_event_subject,
            "queueGroup": state.config.queue_group
        },
        "endpoints": {
            "home": "GET /",
            "descriptor": "GET /descriptor",
            "sources": "GET /sources",
            "schema": "GET /schema",
            "example": "GET /example",
            "datasets": "GET /datasets",
            "jobs": "GET /jobs",
            "webhookIngest": "POST /webhooks/ingest",
            "ingest": "POST /ingest",
            "scrape": "POST /scrape",
            "grantMatch": "POST /grants/match",
            "trends": "POST /analysis/trends",
            "correlations": "POST /analysis/correlations",
            "whitePaper": "POST /briefs/white-paper",
            "pipelineJobs": "POST /pipeline/jobs",
            "healthz": "GET /healthz",
            "readyz": "GET /readyz",
            "metrics": "GET /metrics",
            "apiDocs": "GET /docs/api"
        },
        "sources": source_catalog()
    })
}

pub(crate) fn schema_payload() -> Value {
    json!({
        "ok": true,
        "schemaVersion": SCHEMA_VERSION,
        "contracts": {
            "incomingRecord": {
                "recordId": "optional stable source id",
                "datasetId": "optional dataset grouping id",
                "source": "optional source override",
                "sourceUrl": "optional public URL",
                "title": "short title",
                "summary": "bounded abstract/body text",
                "publishedAt": "source timestamp string",
                "authors": ["names"],
                "tags": ["public", "science", "grant"],
                "metrics": { "numericFeature": 1.23 },
                "grant": "optional grant opportunity object",
                "raw": "bounded JSON metadata; do not include secrets"
            },
            "pipelineJob": {
                "jobType": "spark-etl | airflow-dag | correlation-analysis | white-paper-evidence",
                "datasetIds": ["dataset tokens"],
                "analysisIds": ["analysis tokens"],
                "sink": "minio://public-data/bronze or another approved downstream sink",
                "airflowDag": "optional DAG id",
                "sparkApp": "optional Spark app name",
                "parameters": {}
            }
        },
        "outputs": [
            "normalized dataset records",
            "grant matches",
            "trend summaries",
            "pairwise metric correlations",
            "graph data suitable for chart rendering",
            "white-paper evidence markdown",
            "Spark/Airflow pipeline job intents"
        ]
    })
}

pub(crate) fn example_payload() -> Value {
    json!({
        "ingest": {
            "source": "sbir",
            "datasetId": "sbir-energy-grants",
            "tags": ["grants", "energy", "public"],
            "records": [
                {
                    "recordId": "sbir-topic-001",
                    "title": "Grid resilience research topic",
                    "summary": "Public funding opportunity for grid analytics and resilience modeling.",
                    "sourceUrl": "https://www.sbir.gov/",
                    "metrics": { "awardAmountUsd": 250000, "phase": 1 },
                    "grant": {
                        "title": "Grid resilience research topic",
                        "agency": "DOE",
                        "program": "SBIR",
                        "amount": 250000,
                        "dueDate": "2026-09-15",
                        "eligibility": "US small businesses",
                        "topics": ["energy", "resilience", "analytics"],
                        "url": "https://www.sbir.gov/"
                    }
                }
            ],
            "pipeline": {
                "enabled": true,
                "jobType": "spark-etl",
                "sink": "minio://public-data/bronze/sbir-energy-grants",
                "airflowDag": "public_data_ingest",
                "sparkApp": "public-data-normalize"
            }
        },
        "scrape": {
            "source": "pew-research",
            "url": "https://www.pewresearch.org/",
            "strategy": "auto",
            "includeLinks": true,
            "pipeline": { "enabled": true, "jobType": "airflow-dag" }
        },
        "grantMatch": {
            "applicantProfile": "Small team building mathematical public-data models for energy, health, and civic infrastructure.",
            "focusAreas": ["energy", "AI", "public data", "research"],
            "minAmount": 50000
        }
    })
}
