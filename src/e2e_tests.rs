//! Full-stack HTTP end-to-end tests.
//!
//! [`route_authorization_tests`](crate::route_authorization_tests) proves the
//! gate rejects anonymous callers; these tests prove the composed application
//! actually *serves* once a caller is authorized. Every case below drives a
//! real request through [`build_router`] — body-limit layer, the
//! `require_operator` gate, method routing, the JSON extractors, the handler,
//! and response serialization — using the [`SharedAuthVerifier::for_test`]
//! seam so a fixed operator stands in for the live Supabase/shared-auth race.
//!
//! Persistence is [`Persistence::Disabled`] and the stores are the in-memory
//! implementations, so a `POST /plan` followed by `GET /jobs/{id}` is a genuine
//! store round-trip with no external database.
use super::*;
use axum::body::{to_bytes, Body};
use axum::http::Request as HttpRequest;
use serde_json::{json, Value};
use tower::ServiceExt;

// `shared_auth_lib` is deliberately not `use`d here: `shared_auth.rs` is its one
// module owner (enforced by `protocol_implementations_have_exactly_one_source_owner`),
// so, like the web_server tests, this file names `Authority` inline instead.

/// Any non-empty token authorizes as the fixed operator under `for_test`.
const OPERATOR_TOKEN: &str = "e2e-operator-token";

fn operator() -> crate::shared_auth::Operator {
    crate::shared_auth::Operator {
        subject: "shared-operator-e2e".to_string(),
        email: Some("operator@example.com".to_string()),
        roles: vec!["daedalus-operator".to_string()],
        authority: shared_auth_lib::Authority::SharedAuth,
    }
}

/// A state whose gate accepts `OPERATOR_TOKEN` and rejects everything the real
/// `bearer_token`/verifier would (empty or oversized), backed entirely by
/// in-process stores so no test needs a database or a NATS broker.
fn authenticated_state() -> AppState {
    AppState {
        verifier: Some(Arc::new(crate::shared_auth::SharedAuthVerifier::for_test(
            operator(),
        ))),
        nats: None,
        persistence: Persistence::Disabled,
        realtime: EventHub::new(ServiceSurface::Fabrication, 8),
        request_subject: FABRICATION_REQUESTS_SUBJECT.to_string(),
        queue_group: FABRICATION_REQUESTS_QUEUE_GROUP.to_string(),
        result_subject: FABRICATION_RESULTS_SUBJECT.to_string(),
        event_subject: RUNTIME_EVENTS_SUBJECT.to_string(),
        mdp_subject: MDP_OPTIMIZE_SUBJECT.to_string(),
        mdp_autopublish: false,
        nats_inflight: Arc::new(Semaphore::new(1)),
        coordination: Arc::new(NoopCoordination::default()),
        lease_ttl: Duration::from_millis(coordination::DEFAULT_LEASE_TTL_MS),
        metrics: Arc::new(Metrics::default()),
        jobs: Arc::new(stores::InMemoryJobStore::default()),
        learning: Arc::new(stores::InMemoryLearningStore::default()),
    }
}

fn authenticated_app() -> Router {
    let state = authenticated_state();
    let hub = state.realtime.clone();
    build_router(state, hub)
}

struct Reply {
    status: StatusCode,
    body: Value,
}

impl Reply {
    fn ok(&self) -> bool {
        self.status == StatusCode::OK
    }
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Reply {
    let mut builder = HttpRequest::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&value).expect("serialize body"),
            ))
            .expect("build request"),
        None => builder.body(Body::empty()).expect("build request"),
    };
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("router is infallible");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .expect("read response body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply { status, body }
}

async fn get_auth(app: &Router, path: &str) -> Reply {
    send(app, "GET", path, Some(OPERATOR_TOKEN), None).await
}

async fn post_auth(app: &Router, path: &str, body: Value) -> Reply {
    send(app, "POST", path, Some(OPERATOR_TOKEN), Some(body)).await
}

// ---------------------------------------------------------------------------
// Read surface: catalogs serve real payloads once authorized.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn machine_catalog_serves_the_default_fleet_to_an_operator() {
    let app = authenticated_app();
    let reply = get_auth(&app, "/machines/catalog").await;
    assert!(reply.ok(), "status {}", reply.status);
    assert_eq!(
        reply.body["schemaVersion"],
        "dd.fabrication.machine-catalog.v1"
    );
    assert!(
        reply.body["machineCount"].as_u64().unwrap_or(0) > 0,
        "machine catalog served an empty fleet"
    );
    assert!(
        reply.body["processClassCounts"]["additive"]
            .as_u64()
            .unwrap_or(0)
            >= 5,
        "expected at least five additive machines in the default fleet"
    );
}

#[tokio::test]
async fn fdm_printer_catalog_advertises_creality_k1_and_k2_over_http() {
    let app = authenticated_app();
    let reply = get_auth(&app, "/fdm-printer/catalog").await;
    assert!(reply.ok(), "status {}", reply.status);

    let models = reply.body["supportedPrinterModels"]
        .as_array()
        .expect("supportedPrinterModels array");
    for expected in ["creality-k1", "creality-k2", "creality-k2-cfs"] {
        assert!(
            models.iter().any(|model| model["model"] == expected),
            "supported printer models missing {expected}"
        );
    }

    let k2 = models
        .iter()
        .find(|model| model["model"] == "creality-k2")
        .expect("creality-k2 model");
    assert_eq!(k2["machineKind"], "fdm-printer");
    assert_eq!(k2["maxMaterials"], 1);
    let k2_cfs = models
        .iter()
        .find(|model| model["model"] == "creality-k2-cfs")
        .expect("creality-k2-cfs model");
    assert_eq!(k2_cfs["machineKind"], "multi-material-fdm-printer");
    assert_eq!(k2_cfs["maxMaterials"], 16);

    let printers = reply.body["fdmPrinters"]
        .as_array()
        .expect("fdmPrinters array");
    for expected in ["creality-k1-1", "creality-k2-1", "creality-k2-cfs-1"] {
        assert!(
            printers.iter().any(|printer| printer["id"] == expected),
            "fleet missing {expected}"
        );
    }
    let k1 = printers
        .iter()
        .find(|printer| printer["id"] == "creality-k1-1")
        .expect("creality-k1-1 fleet entry");
    assert!(
        k1["acceptedInstructionLanguages"]
            .as_array()
            .is_some_and(|languages| languages.iter().any(|language| language == "klipper-gcode")),
        "K1 should advertise the klipper-gcode dialect end to end"
    );
}

#[tokio::test]
async fn printer_catalog_lists_the_creality_fleet_over_http() {
    let app = authenticated_app();
    let reply = get_auth(&app, "/printers/catalog").await;
    assert!(reply.ok(), "status {}", reply.status);
    let printers = reply.body["printers"].as_array().expect("printers array");
    for expected in ["creality-k1-1", "creality-k2-1", "creality-k2-cfs-1"] {
        assert!(
            printers.iter().any(|printer| printer["id"] == expected),
            "printer catalog missing {expected}"
        );
    }
}

#[tokio::test]
async fn fdm_printer_catalog_advertises_post_2024_models_over_http() {
    let app = authenticated_app();
    let reply = get_auth(&app, "/fdm-printer/catalog").await;
    assert!(reply.ok(), "status {}", reply.status);
    let models = reply.body["supportedPrinterModels"]
        .as_array()
        .expect("supportedPrinterModels array");
    for expected in [
        "bambu-h2d",
        "prusa-core-one",
        "creality-k1c",
        "elegoo-centauri-carbon",
    ] {
        assert!(
            models.iter().any(|model| model["model"] == expected),
            "over-the-wire catalog missing {expected}"
        );
    }
    // The Prusa CORE One is Buddy/Marlin, so its fleet entry advertises the
    // marlin dialect rather than klipper — verified end to end.
    let printers = reply.body["fdmPrinters"]
        .as_array()
        .expect("fdmPrinters array");
    let core_one = printers
        .iter()
        .find(|printer| printer["id"] == "prusa-core-one-1")
        .expect("prusa-core-one-1 fleet entry");
    assert!(core_one["acceptedInstructionLanguages"]
        .as_array()
        .is_some_and(|languages| languages.iter().any(|language| language == "marlin-gcode")));
}

#[tokio::test]
async fn fdm_printer_catalog_advertises_three_additional_makes_over_http() {
    let app = authenticated_app();
    let reply = get_auth(&app, "/fdm-printer/catalog").await;
    assert!(reply.ok(), "status {}", reply.status);

    let models = reply.body["supportedPrinterModels"]
        .as_array()
        .expect("supportedPrinterModels array");
    for expected in [
        "anycubic-kobra-3-combo",
        "anycubic-kobra-s1-combo",
        "qidi-q1-pro",
        "qidi-plus4",
        "flashforge-adventurer-5m",
        "flashforge-adventurer-5m-pro",
    ] {
        assert!(
            models.iter().any(|model| model["model"] == expected),
            "over-the-wire catalog missing {expected}"
        );
    }

    let kobra_s1 = models
        .iter()
        .find(|model| model["model"] == "anycubic-kobra-s1-combo")
        .expect("Anycubic Kobra S1 Combo model");
    assert_eq!(kobra_s1["machineKind"], "multi-material-fdm-printer");
    assert_eq!(kobra_s1["maxMaterials"], 8);
    assert_eq!(kobra_s1["enclosed"], true);

    let plus4 = models
        .iter()
        .find(|model| model["model"] == "qidi-plus4")
        .expect("QIDI Plus4 model");
    assert_eq!(plus4["workEnvelopeMm"], json!([305.0, 305.0, 280.0]));
    assert_eq!(plus4["maxNozzleTempC"], 370.0);

    let printers = reply.body["fdmPrinters"]
        .as_array()
        .expect("fdmPrinters array");
    let adventurer_5m_pro = printers
        .iter()
        .find(|printer| printer["id"] == "flashforge-adventurer-5m-pro-1")
        .expect("FlashForge Adventurer 5M Pro fleet entry");
    assert!(adventurer_5m_pro["acceptedInstructionLanguages"]
        .as_array()
        .is_some_and(|languages| languages
            .iter()
            .any(|language| language == "flashforge-gcode")));
}

#[tokio::test]
async fn turning_catalog_advertises_named_lathe_profiles_over_http() {
    let app = authenticated_app();
    let reply = get_auth(&app, "/turning/catalog").await;
    assert!(reply.ok(), "status {} body {}", reply.status, reply.body);
    assert_eq!(reply.body["supportedTurningModelCount"], 2);

    let models = reply.body["supportedTurningModels"]
        .as_array()
        .expect("supportedTurningModels array");
    for expected in ["haas-st-20", "dn-solutions-lynx-2100b-fanuc"] {
        assert!(
            models.iter().any(|model| model["model"] == expected),
            "named turning catalog missing {expected}"
        );
    }

    let machines = reply.body["turningMachines"]
        .as_array()
        .expect("turningMachines array");
    let haas = machines
        .iter()
        .find(|machine| machine["id"] == "haas-st-20-1")
        .expect("Haas ST-20 fleet entry");
    assert!(haas["acceptedInstructionLanguages"]
        .as_array()
        .is_some_and(|languages| languages.iter().any(|language| language == "haas-gcode")));
    let lynx = machines
        .iter()
        .find(|machine| machine["id"] == "dn-solutions-lynx-2100b-fanuc-1")
        .expect("Lynx 2100B Fanuc fleet entry");
    assert!(lynx["acceptedInstructionLanguages"]
        .as_array()
        .is_some_and(|languages| languages.iter().any(|language| language == "fanuc-gcode")));

    let lathe_reply = get_auth(&app, "/lathe/catalog").await;
    assert!(
        lathe_reply.ok(),
        "status {} body {}",
        lathe_reply.status,
        lathe_reply.body
    );
    assert_eq!(lathe_reply.body["supportedTurningModelCount"], 2);
    assert!(lathe_reply.body["latheMachines"]
        .as_array()
        .is_some_and(|lathe_machines| lathe_machines
            .iter()
            .any(|machine| machine["id"] == "haas-st-20-1")));
}

#[tokio::test]
async fn elegoo_centauri_carbon_fiber_job_passes_preflight_over_http() {
    let app = authenticated_app();
    // A carbon-fiber PA job on a Centauri-class machine (320 C nozzle, 110 C bed,
    // ~25 mm3/s hotend) at a sane flow: release-ready end to end.
    let payload = json!({
        "process": "fdm",
        "requestId": "e2e-centauri-cf",
        "part": {
            "dimensions": {"xMm": 80.0, "yMm": 60.0, "zMm": 40.0},
            "minWallMm": 1.2,
            "maxOverhangDegrees": 30.0
        },
        "machine": {
            "buildVolume": {"xMm": 256.0, "yMm": 256.0, "zMm": 256.0},
            "nozzleDiameterMm": 0.4,
            "maxVolumetricFlowMm3S": 25.0,
            "enclosed": true,
            "maxMaterials": 1
        },
        "material": {
            "name": "PA-CF",
            "nozzleTempMinC": 280.0,
            "nozzleTempMaxC": 320.0,
            "bedTempMinC": 90.0,
            "bedTempMaxC": 110.0,
            "dryingRequired": true,
            "enclosureRequired": true
        },
        "profile": {
            "layerHeightMm": 0.2,
            "firstLayerHeightMm": 0.24,
            "lineWidthMm": 0.44,
            "printSpeedMmS": 200.0,
            "nozzleTempC": 300.0,
            "bedTempC": 100.0,
            "supportsEnabled": true,
            "driedHours": 8.0
        }
    });
    let reply = post_auth(&app, "/printing/preflight", payload).await;
    assert!(reply.ok(), "status {} body {}", reply.status, reply.body);
    assert_eq!(reply.body["releaseReady"], true, "body {}", reply.body);
}

#[tokio::test]
async fn narrative_and_capability_surfaces_serve_to_an_operator() {
    let app = authenticated_app();
    for path in [
        "/",
        "/capabilities",
        "/how-it-works",
        "/printing/preflight/catalog",
    ] {
        let reply = get_auth(&app, path).await;
        assert_eq!(reply.status, StatusCode::OK, "{path} did not serve");
    }
}

#[tokio::test]
async fn additive_preflight_catalog_describes_release_gates_over_http() {
    let app = authenticated_app();
    let reply = get_auth(&app, "/printing/preflight/catalog").await;
    assert!(reply.ok(), "status {}", reply.status);
    assert!(
        reply.body["processes"]["fdm"]["releaseGates"]
            .as_array()
            .is_some_and(|gates| gates.len() >= 8),
        "FDM release gates missing from the preflight catalog"
    );
    assert!(reply.body["processes"]["resin"]["releaseGates"]
        .as_array()
        .is_some_and(|gates| gates.len() >= 8));
}

// ---------------------------------------------------------------------------
// FDM preflight: the Creality K1/K2 payloads driven through the real handler.
// ---------------------------------------------------------------------------

fn creality_k1_payload(print_speed_mm_s: f64) -> Value {
    json!({
        "process": "fdm",
        "requestId": "e2e-creality-k1",
        "part": {
            "dimensions": {"xMm": 50.0, "yMm": 40.0, "zMm": 30.0},
            "minWallMm": 1.2,
            "maxOverhangDegrees": 35.0
        },
        "machine": {
            "buildVolume": {"xMm": 220.0, "yMm": 220.0, "zMm": 250.0},
            "nozzleDiameterMm": 0.4,
            "maxVolumetricFlowMm3S": 32.0,
            "enclosed": true,
            "maxMaterials": 1
        },
        "material": {
            "name": "PETG",
            "nozzleTempMinC": 225.0,
            "nozzleTempMaxC": 260.0,
            "bedTempMinC": 70.0,
            "bedTempMaxC": 90.0,
            "dryingRequired": true
        },
        "profile": {
            "layerHeightMm": 0.2,
            "firstLayerHeightMm": 0.24,
            "lineWidthMm": 0.42,
            "printSpeedMmS": print_speed_mm_s,
            "nozzleTempC": 250.0,
            "bedTempC": 80.0,
            "supportsEnabled": true,
            "driedHours": 6.0
        }
    })
}

fn creality_k2_cfs_payload() -> Value {
    json!({
        "process": "fdm",
        "requestId": "e2e-creality-k2-cfs",
        "part": {
            "dimensions": {"xMm": 50.0, "yMm": 40.0, "zMm": 30.0},
            "minWallMm": 1.2,
            "maxOverhangDegrees": 35.0
        },
        "machine": {
            "buildVolume": {"xMm": 260.0, "yMm": 260.0, "zMm": 260.0},
            "nozzleDiameterMm": 0.4,
            "maxVolumetricFlowMm3S": 32.0,
            "enclosed": true,
            "maxMaterials": 16
        },
        "material": {
            "name": "PLA",
            "nozzleTempMinC": 190.0,
            "nozzleTempMaxC": 230.0,
            "bedTempMinC": 50.0,
            "bedTempMaxC": 65.0
        },
        "profile": {
            "layerHeightMm": 0.2,
            "firstLayerHeightMm": 0.24,
            "lineWidthMm": 0.42,
            "printSpeedMmS": 300.0,
            "nozzleTempC": 220.0,
            "bedTempC": 60.0,
            "supportsEnabled": true,
            "materialCount": 4,
            "toolChanges": 24,
            "purgeVolumePerChangeMm3": 45.0
        }
    })
}

#[tokio::test]
async fn creality_k1_high_speed_job_passes_preflight_over_http() {
    let app = authenticated_app();
    // 0.2 * 0.42 * 300 = 25.2 mm3/s, within the K1's ~32 mm3/s hotend ceiling.
    let reply = post_auth(&app, "/printing/preflight", creality_k1_payload(300.0)).await;
    assert!(reply.ok(), "status {} body {}", reply.status, reply.body);
    assert_eq!(reply.body["releaseReady"], true, "body {}", reply.body);
    assert_eq!(reply.body["process"], "fdm");
    assert_eq!(reply.body["derived"]["volumetricFlowMm3S"], 25.2);
}

#[tokio::test]
async fn creality_k1_overspeed_is_flagged_for_volumetric_flow_over_http() {
    let app = authenticated_app();
    // 0.2 * 0.42 * 600 = 50.4 mm3/s, past the hotend ceiling: a release gate,
    // not a request error, so the handler still answers 200 with the finding.
    let reply = post_auth(&app, "/printing/preflight", creality_k1_payload(600.0)).await;
    assert!(reply.ok(), "status {} body {}", reply.status, reply.body);
    assert_eq!(reply.body["releaseReady"], false);
    let findings = reply.body["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|finding| finding["code"] == "fdm.volumetric-flow-exceeded"),
        "expected a volumetric-flow blocker, got {}",
        reply.body["findings"]
    );
}

fn resin_payload(max_cross_section_area_mm2: f64, lift_speed_mm_min: f64) -> Value {
    json!({
        "process": "resin",
        "requestId": "e2e-resin",
        "part": {
            "dimensions": {"xMm": 50.0, "yMm": 40.0, "zMm": 30.0},
            "minWallMm": 1.2,
            "maxOverhangDegrees": 35.0,
            "maxCrossSectionAreaMm2": max_cross_section_area_mm2
        },
        "machine": {
            "buildVolume": {"xMm": 130.0, "yMm": 80.0, "zMm": 160.0},
            "buildPlateAreaMm2": 10400.0,
            "minWallMm": 0.8
        },
        "material": {
            "name": "tough-resin",
            "exposureMinS": 2.0,
            "exposureMaxS": 3.0
        },
        "profile": {
            "layerHeightMm": 0.05,
            "exposureS": 2.5,
            "supportsEnabled": true,
            "drainHoleCount": 2,
            "washMinutes": 5.0,
            "cureMinutes": 10.0,
            "liftSpeedMmMin": lift_speed_mm_min
        }
    })
}

#[tokio::test]
async fn a_clean_resin_job_passes_preflight_over_http() {
    let app = authenticated_app();
    // Small cross-section, gentle lift: no peel risk.
    let reply = post_auth(&app, "/printing/preflight", resin_payload(1_000.0, 60.0)).await;
    assert!(reply.ok(), "status {} body {}", reply.status, reply.body);
    assert_eq!(reply.body["process"], "resin");
    assert_eq!(reply.body["releaseReady"], true, "body {}", reply.body);
}

#[tokio::test]
async fn a_high_peel_resin_job_is_flagged_over_http() {
    let app = authenticated_app();
    // 8000 / 10400 = 0.77 cross-section utilization at 120 mm/min lift: a
    // peel-force blocker, still answered 200 with the finding attached.
    let reply = post_auth(&app, "/printing/preflight", resin_payload(8_000.0, 120.0)).await;
    assert!(reply.ok(), "status {} body {}", reply.status, reply.body);
    assert_eq!(reply.body["releaseReady"], false);
    let findings = reply.body["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|finding| finding["code"] == "resin.peel-force-risk"),
        "expected a peel-force blocker, got {}",
        reply.body["findings"]
    );
}

#[tokio::test]
async fn creality_k2_cfs_multi_material_job_passes_preflight_over_http() {
    let app = authenticated_app();
    let reply = post_auth(&app, "/printing/preflight", creality_k2_cfs_payload()).await;
    assert!(reply.ok(), "status {} body {}", reply.status, reply.body);
    assert_eq!(reply.body["releaseReady"], true, "body {}", reply.body);
    assert_eq!(reply.body["requestId"], "e2e-creality-k2-cfs");
}

#[tokio::test]
async fn preflight_rejects_a_structurally_invalid_request_with_422() {
    let app = authenticated_app();
    let mut payload = creality_k1_payload(300.0);
    // A negative build-volume axis is caught by request validation, which is a
    // 422 (unprocessable) rather than a release finding.
    payload["machine"]["buildVolume"]["xMm"] = json!(-5.0);
    let reply = post_auth(&app, "/printing/preflight", payload).await;
    assert_eq!(
        reply.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "body {}",
        reply.body
    );
    assert_eq!(reply.body["ok"], false);
}

#[tokio::test]
async fn preflight_rejects_an_unknown_process_tag() {
    let app = authenticated_app();
    let reply = post_auth(
        &app,
        "/printing/preflight",
        json!({ "process": "laser-sinter" }),
    )
    .await;
    assert!(
        reply.status.is_client_error(),
        "an unknown process tag must be a client error, got {}",
        reply.status
    );
}

// ---------------------------------------------------------------------------
// Plan -> job store round-trip and learning, entirely in-process.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_plan_creates_a_job_that_is_retrievable_over_http() {
    let app = authenticated_app();
    let plan = post_auth(
        &app,
        "/plan",
        json!({
            "requestId": "e2e-plan-1",
            "objective": "PLA enclosure panel for the Creality K1",
            "material": { "name": "pla", "family": "polymer" }
        }),
    )
    .await;
    assert!(plan.ok(), "status {} body {}", plan.status, plan.body);
    let job_id = plan.body["jobId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .expect("plan response should carry a job id");

    let job = get_auth(&app, &format!("/jobs/{job_id}")).await;
    assert!(job.ok(), "job fetch status {}", job.status);
    assert_eq!(job.body["ok"], true);

    let list = get_auth(&app, "/jobs").await;
    assert!(list.ok(), "jobs list status {}", list.status);
    assert_eq!(list.body["ok"], true);
    assert!(
        list.body["count"].as_u64().unwrap_or(0) >= 1,
        "the created job should appear in the list"
    );
}

#[tokio::test]
async fn an_unknown_job_id_is_a_404_over_http() {
    let app = authenticated_app();
    let reply = get_auth(&app, "/jobs/no-such-job-e2e").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.body["ok"], false);
}

#[tokio::test]
async fn learning_observe_then_policy_round_trips_over_http() {
    let app = authenticated_app();
    let observed = post_auth(
        &app,
        "/learning/observe",
        json!({
            "requestId": "e2e-observe-1",
            "machineKind": "fdm-printer",
            "outcome": "print completed clean on the K1",
            "completed": true
        }),
    )
    .await;
    assert!(
        observed.ok(),
        "status {} body {}",
        observed.status,
        observed.body
    );
    assert_eq!(observed.body["ok"], true);
    assert!(
        observed.body.get("learning").is_some(),
        "missing learning block"
    );
    assert!(
        observed.body.get("policy").is_some(),
        "missing policy block"
    );

    let policy = get_auth(&app, "/learning/policy").await;
    assert!(policy.ok(), "policy status {}", policy.status);
}

// ---------------------------------------------------------------------------
// Auth and protocol boundaries, exercised through the composed application.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn representative_routes_reject_an_anonymous_caller() {
    let app = authenticated_app();
    for (method, path, body) in [
        ("GET", "/machines/catalog", None),
        ("GET", "/fdm-printer/catalog", None),
        ("POST", "/plan", Some(json!({ "objective": "x" }))),
        (
            "POST",
            "/printing/preflight",
            Some(creality_k1_payload(300.0)),
        ),
    ] {
        let reply = send(&app, method, path, None, body).await;
        assert_eq!(
            reply.status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} served an anonymous caller"
        );
    }
}

#[tokio::test]
async fn an_empty_bearer_token_is_rejected() {
    let app = authenticated_app();
    let reply = send(&app, "GET", "/machines/catalog", Some(""), None).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_oversized_bearer_token_is_rejected() {
    let app = authenticated_app();
    let giant = "a".repeat(17 * 1024);
    let reply = send(&app, "GET", "/machines/catalog", Some(&giant), None).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_unknown_path_is_a_404_even_with_a_valid_token() {
    let app = authenticated_app();
    // A `route_layer` gate leaves an unmatched path as a 404, not a 401 that
    // would confirm the path is real.
    let reply = get_auth(&app, "/no/such/route/e2e").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_wrong_method_on_a_real_route_is_405() {
    let app = authenticated_app();
    let reply = send(
        &app,
        "DELETE",
        "/machines/catalog",
        Some(OPERATOR_TOKEN),
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn an_oversized_body_is_413_before_the_handler() {
    let app = authenticated_app();
    let request = HttpRequest::builder()
        .method("POST")
        .uri("/plan")
        .header("authorization", format!("Bearer {OPERATOR_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(vec![b'a'; MAX_HTTP_BODY_BYTES + 1]))
        .expect("build request");
    let status = app
        .clone()
        .oneshot(request)
        .await
        .expect("router is infallible")
        .status();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_malformed_json_body_is_a_client_error() {
    let app = authenticated_app();
    let request = HttpRequest::builder()
        .method("POST")
        .uri("/plan")
        .header("authorization", format!("Bearer {OPERATOR_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from("{ this is not valid json "))
        .expect("build request");
    let status = app
        .clone()
        .oneshot(request)
        .await
        .expect("router is infallible")
        .status();
    assert!(
        status.is_client_error(),
        "malformed JSON must be a client error, got {status}"
    );
}

// ---------------------------------------------------------------------------
// Over the wire: bind a real ephemeral socket, serve the composed app, and
// drive it with an HTTP client so the TCP transport and body streaming are in
// the path too — not just the in-memory `oneshot` service call.
// ---------------------------------------------------------------------------

/// Serve [`authenticated_app`] on `127.0.0.1:0` and return its base URL. The
/// server task is detached; dropping its handle does not stop it (only `abort`
/// would), so it keeps answering for the life of the test.
async fn spawn_server() -> String {
    let app = authenticated_app();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn health_probe_answers_over_the_wire_without_a_token() {
    let base = spawn_server().await;
    let response = reqwest::Client::new()
        .get(format!("{base}/healthz"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("json body");
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn anonymous_request_is_401_over_the_wire() {
    let base = spawn_server().await;
    let response = reqwest::Client::new()
        .get(format!("{base}/machines/catalog"))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn creality_catalog_is_served_over_the_wire_to_an_operator() {
    let base = spawn_server().await;
    let response = reqwest::Client::new()
        .get(format!("{base}/fdm-printer/catalog"))
        .bearer_auth(OPERATOR_TOKEN)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("json body");
    let models = body["supportedPrinterModels"]
        .as_array()
        .expect("supportedPrinterModels array");
    for expected in ["creality-k1", "creality-k2", "creality-k2-cfs"] {
        assert!(
            models.iter().any(|model| model["model"] == expected),
            "over-the-wire catalog missing {expected}"
        );
    }
}

#[tokio::test]
async fn creality_k1_preflight_runs_over_the_wire() {
    let base = spawn_server().await;
    let response = reqwest::Client::new()
        .post(format!("{base}/printing/preflight"))
        .bearer_auth(OPERATOR_TOKEN)
        .json(&creality_k1_payload(300.0))
        .send()
        .await
        .expect("request");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = response.json().await.expect("json body");
    assert_eq!(body["releaseReady"], true, "body {body}");
    assert_eq!(body["derived"]["volumetricFlowMm3S"], 25.2);
}
