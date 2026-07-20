//! Thin Axum adapter for additive-manufacturing preflight.

use axum::{
    extract::Extension,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use crate::realtime::EventHub;

use super::{analysis, model::PrintPreflightRequest, ADDITIVE_PREFLIGHT_SCHEMA};

pub(super) fn router<S>(hub: EventHub) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/printing/preflight", post(preflight))
        .route("/fabrication/printing/preflight", post(preflight))
        .route("/printing/preflight/catalog", get(catalog))
        .route("/fabrication/printing/preflight/catalog", get(catalog))
        .layer(Extension(hub))
}

async fn preflight(
    Extension(hub): Extension<EventHub>,
    Json(request): Json<PrintPreflightRequest>,
) -> Response {
    match analysis::analyze(request) {
        Ok(response) => {
            let payload = serde_json::to_value(&response).unwrap_or_else(|_| json!({}));
            hub.publish_payload(
                "dd-fabrication-server",
                "printer.preflight.completed",
                payload,
            );
            Json(response).into_response()
        }
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "ok": false,
                "schemaVersion": ADDITIVE_PREFLIGHT_SCHEMA,
                "error": error.to_string(),
            })),
        )
            .into_response(),
    }
}

async fn catalog() -> Json<serde_json::Value> {
    Json(json!({
        "schemaVersion": ADDITIVE_PREFLIGHT_SCHEMA,
        "processes": {
            "fdm": {
                "releaseGates": [
                    "build-volume",
                    "layer-and-line-geometry",
                    "volumetric-flow",
                    "material-temperature",
                    "material-drying",
                    "enclosure",
                    "overhang-support",
                    "minimum-wall",
                    "multi-material-capacity-and-purge"
                ]
            },
            "resin": {
                "releaseGates": [
                    "build-volume",
                    "layer-and-exposure-profile",
                    "minimum-wall",
                    "island-support",
                    "enclosed-volume-drainage",
                    "peel-and-suction-force",
                    "wash",
                    "post-cure"
                ]
            }
        },
        "releasePolicy": "Preflight is evidence, not machine authorization; slicing, simulation, first-article inspection, and operator/controller release remain required."
    }))
}

#[cfg(test)]
mod tests {
    use crate::realtime::ServiceSurface;

    use super::*;

    fn valid_fdm_request() -> PrintPreflightRequest {
        serde_json::from_value(json!({
            "process": "fdm",
            "requestId": "http-fdm-1",
            "part": {
                "dimensions": {"xMm": 20.0, "yMm": 20.0, "zMm": 20.0},
                "minWallMm": 1.2,
                "maxOverhangDegrees": 30.0
            },
            "machine": {
                "buildVolume": {"xMm": 220.0, "yMm": 220.0, "zMm": 250.0},
                "nozzleDiameterMm": 0.4,
                "maxVolumetricFlowMm3S": 15.0,
                "enclosed": true,
                "maxMaterials": 1
            },
            "material": {
                "name": "PLA",
                "nozzleTempMinC": 190.0,
                "nozzleTempMaxC": 220.0,
                "bedTempMinC": 50.0,
                "bedTempMaxC": 65.0
            },
            "profile": {
                "layerHeightMm": 0.2,
                "firstLayerHeightMm": 0.24,
                "lineWidthMm": 0.44,
                "printSpeedMmS": 50.0,
                "nozzleTempC": 205.0,
                "bedTempC": 60.0,
                "supportsEnabled": true
            }
        }))
        .expect("valid FDM request")
    }

    #[tokio::test]
    async fn successful_http_preflight_publishes_the_same_realtime_payload() {
        let hub = EventHub::new(ServiceSurface::Fabrication, 8);
        let response = preflight(Extension(hub.clone()), Json(valid_fdm_request())).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(hub.latest().kind, "printer.preflight.completed");
        assert_eq!(hub.latest().payload["requestId"], "http-fdm-1");
        assert_eq!(hub.latest().payload["releaseReady"], true);
    }

    #[tokio::test]
    async fn catalog_describes_fdm_and_resin_release_gates() {
        let Json(catalog) = catalog().await;
        assert_eq!(catalog["schemaVersion"], ADDITIVE_PREFLIGHT_SCHEMA);
        assert!(catalog["processes"]["fdm"]["releaseGates"]
            .as_array()
            .is_some_and(|gates| gates.len() >= 8));
        assert!(catalog["processes"]["resin"]["releaseGates"]
            .as_array()
            .is_some_and(|gates| gates.len() >= 8));
    }
}
