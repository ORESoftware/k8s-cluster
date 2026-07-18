//! Hand-rolled Prometheus counters, same pattern as the rest of the Rust
//! fleet: AtomicU64s exposed in text format at /metrics.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Metrics {
    pub http_requests_total: AtomicU64,
    pub stt_total: AtomicU64,
    pub tts_total: AtomicU64,
    pub translations_total: AtomicU64,
    pub pipeline_total: AtomicU64,
    pub analyze_total: AtomicU64,
    pub vapi_webhook_events_total: AtomicU64,
    pub vapi_webhook_unauthorized_total: AtomicU64,
    pub vapi_tool_calls_total: AtomicU64,
    pub llm_overloaded_total: AtomicU64,
    pub errors_total: AtomicU64,
}

impl Metrics {
    pub fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn render(&self) -> String {
        let mut out = String::with_capacity(1500);
        let mut emit = |name: &str, help: &str, value: u64| {
            out.push_str(&format!(
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}\n"
            ));
        };
        emit(
            "t2v_api_http_requests_total",
            "HTTP requests handled by t2v-api.",
            self.http_requests_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_stt_total",
            "Speech-to-text requests.",
            self.stt_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_tts_total",
            "Text-to-speech requests.",
            self.tts_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_translations_total",
            "Translation requests across all providers.",
            self.translations_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_pipeline_total",
            "Full speech-to-speech pipeline runs.",
            self.pipeline_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_analyze_total",
            "FFT analysis requests.",
            self.analyze_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_vapi_webhook_events_total",
            "Vapi webhook events received.",
            self.vapi_webhook_events_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_vapi_webhook_unauthorized_total",
            "Vapi webhook events rejected for a bad secret.",
            self.vapi_webhook_unauthorized_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_vapi_tool_calls_total",
            "Vapi tool calls executed.",
            self.vapi_tool_calls_total.load(Ordering::Relaxed),
        );
        emit(
            "t2v_api_errors_total",
            "Requests that ended in an error response.",
            self.errors_total.load(Ordering::Relaxed),
        );
        out
    }
}
