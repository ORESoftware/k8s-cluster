use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HTTP_DURATION_BUCKETS_SECONDS: [f64; 8] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 1.0, 5.0];
const HTTP_STATUS_CLASSES: [&str; 4] = ["2xx", "3xx", "4xx", "5xx"];

pub struct Metrics {
    pub http_requests_total: AtomicU64,
    pub auth_failures_total: AtomicU64,
    pub db_queries_total: AtomicU64,
    pub db_errors_total: AtomicU64,
    pub votes_cast_total: AtomicU64,
    pub tallies_total: AtomicU64,
    pub simulations_total: AtomicU64,
    pub contract_requests_total: AtomicU64,
    pub contract_errors_total: AtomicU64,
    http_responses_total: [AtomicU64; HTTP_STATUS_CLASSES.len()],
    http_request_duration_buckets: [AtomicU64; HTTP_DURATION_BUCKETS_SECONDS.len()],
    http_request_duration_count: AtomicU64,
    http_request_duration_micros: AtomicU64,
    process_started_at: Instant,
    process_start_time_seconds: u64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            http_requests_total: AtomicU64::new(0),
            auth_failures_total: AtomicU64::new(0),
            db_queries_total: AtomicU64::new(0),
            db_errors_total: AtomicU64::new(0),
            votes_cast_total: AtomicU64::new(0),
            tallies_total: AtomicU64::new(0),
            simulations_total: AtomicU64::new(0),
            contract_requests_total: AtomicU64::new(0),
            contract_errors_total: AtomicU64::new(0),
            http_responses_total: std::array::from_fn(|_| AtomicU64::new(0)),
            http_request_duration_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            http_request_duration_count: AtomicU64::new(0),
            http_request_duration_micros: AtomicU64::new(0),
            process_started_at: Instant::now(),
            process_start_time_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
}

impl Metrics {
    pub fn inc_http(&self) {
        self.http_requests_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_db_query(&self) {
        self.db_queries_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_db_error(&self) {
        self.db_errors_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn observe_http_response(&self, status: u16, elapsed: Duration) {
        if let Some(class) = status_class_index(status) {
            self.http_responses_total[class].fetch_add(1, Ordering::Relaxed);
        }

        let elapsed_seconds = elapsed.as_secs_f64();
        for (index, upper_bound) in HTTP_DURATION_BUCKETS_SECONDS.iter().enumerate() {
            if elapsed_seconds <= *upper_bound {
                self.http_request_duration_buckets[index].fetch_add(1, Ordering::Relaxed);
            }
        }
        self.http_request_duration_count
            .fetch_add(1, Ordering::Relaxed);
        self.http_request_duration_micros.fetch_add(
            elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn render(&self, database_configured: bool) -> String {
        let mut output = format!(
            "# HELP usacc_rest_api_info Static service info.\n\
             # TYPE usacc_rest_api_info gauge\n\
             usacc_rest_api_info{{database_configured=\"{}\"}} 1\n\
             # HELP usacc_rest_api_build_info Build information for the running service.\n\
             # TYPE usacc_rest_api_build_info gauge\n\
             usacc_rest_api_build_info{{version=\"{}\"}} 1\n\
             # HELP usacc_rest_api_process_up Whether the service process is running.\n\
             # TYPE usacc_rest_api_process_up gauge\n\
             usacc_rest_api_process_up 1\n\
             # HELP usacc_rest_api_process_start_time_seconds Process start time since Unix epoch.\n\
             # TYPE usacc_rest_api_process_start_time_seconds gauge\n\
             usacc_rest_api_process_start_time_seconds {}\n\
             # HELP usacc_rest_api_process_uptime_seconds Process uptime in seconds.\n\
             # TYPE usacc_rest_api_process_uptime_seconds gauge\n\
             usacc_rest_api_process_uptime_seconds {:.6}\n\
             # HELP usacc_rest_api_http_requests_total HTTP requests observed.\n\
             # TYPE usacc_rest_api_http_requests_total counter\n\
             usacc_rest_api_http_requests_total {}\n\
             # HELP usacc_rest_api_auth_failures_total Auth failures observed.\n\
             # TYPE usacc_rest_api_auth_failures_total counter\n\
             usacc_rest_api_auth_failures_total {}\n\
             # HELP usacc_rest_api_db_queries_total Database queries attempted.\n\
             # TYPE usacc_rest_api_db_queries_total counter\n\
             usacc_rest_api_db_queries_total {}\n\
             # HELP usacc_rest_api_db_errors_total Database query errors observed.\n\
             # TYPE usacc_rest_api_db_errors_total counter\n\
             usacc_rest_api_db_errors_total {}\n\
             # HELP usacc_rest_api_votes_cast_total Votes accepted by the API.\n\
             # TYPE usacc_rest_api_votes_cast_total counter\n\
             usacc_rest_api_votes_cast_total {}\n\
             # HELP usacc_rest_api_tallies_total Election tallies computed.\n\
             # TYPE usacc_rest_api_tallies_total counter\n\
             usacc_rest_api_tallies_total {}\n\
             # HELP usacc_rest_api_simulations_total Simulation runs executed.\n\
             # TYPE usacc_rest_api_simulations_total counter\n\
             usacc_rest_api_simulations_total {}\n\
             # HELP usacc_rest_api_contract_requests_total Contract-service proxy requests attempted.\n\
             # TYPE usacc_rest_api_contract_requests_total counter\n\
             usacc_rest_api_contract_requests_total {}\n\
             # HELP usacc_rest_api_contract_errors_total Contract-service proxy errors observed.\n\
             # TYPE usacc_rest_api_contract_errors_total counter\n\
             usacc_rest_api_contract_errors_total {}\n",
            database_configured,
            env!("CARGO_PKG_VERSION"),
            self.process_start_time_seconds,
            self.process_started_at.elapsed().as_secs_f64(),
            self.http_requests_total.load(Ordering::Relaxed),
            self.auth_failures_total.load(Ordering::Relaxed),
            self.db_queries_total.load(Ordering::Relaxed),
            self.db_errors_total.load(Ordering::Relaxed),
            self.votes_cast_total.load(Ordering::Relaxed),
            self.tallies_total.load(Ordering::Relaxed),
            self.simulations_total.load(Ordering::Relaxed),
            self.contract_requests_total.load(Ordering::Relaxed),
            self.contract_errors_total.load(Ordering::Relaxed),
        );

        output.push_str(
            "# HELP usacc_rest_api_http_responses_total HTTP responses by bounded status class.\n\
             # TYPE usacc_rest_api_http_responses_total counter\n",
        );
        for (index, class) in HTTP_STATUS_CLASSES.iter().enumerate() {
            output.push_str(&format!(
                "usacc_rest_api_http_responses_total{{status_class=\"{class}\"}} {}\n",
                self.http_responses_total[index].load(Ordering::Relaxed)
            ));
        }

        output.push_str(
            "# HELP usacc_rest_api_http_request_duration_seconds HTTP request duration.\n\
             # TYPE usacc_rest_api_http_request_duration_seconds histogram\n",
        );
        for (index, upper_bound) in HTTP_DURATION_BUCKETS_SECONDS.iter().enumerate() {
            output.push_str(&format!(
                "usacc_rest_api_http_request_duration_seconds_bucket{{le=\"{upper_bound}\"}} {}\n",
                self.http_request_duration_buckets[index].load(Ordering::Relaxed)
            ));
        }
        let duration_count = self.http_request_duration_count.load(Ordering::Relaxed);
        output.push_str(&format!(
            "usacc_rest_api_http_request_duration_seconds_bucket{{le=\"+Inf\"}} {duration_count}\n\
             usacc_rest_api_http_request_duration_seconds_sum {:.6}\n\
             usacc_rest_api_http_request_duration_seconds_count {duration_count}\n",
            self.http_request_duration_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0
        ));

        output
    }
}

fn status_class_index(status: u16) -> Option<usize> {
    match status {
        200..=299 => Some(0),
        300..=399 => Some(1),
        400..=499 => Some(2),
        500..=599 => Some(3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_bounded_prometheus_response_latency_and_process_metrics() {
        let metrics = Metrics::default();
        metrics.observe_http_response(200, Duration::from_millis(4));
        metrics.observe_http_response(503, Duration::from_millis(300));

        let rendered = metrics.render(true);

        assert!(rendered.contains("usacc_rest_api_http_responses_total{status_class=\"2xx\"} 1"));
        assert!(rendered.contains("usacc_rest_api_http_responses_total{status_class=\"5xx\"} 1"));
        assert!(
            rendered.contains("usacc_rest_api_http_request_duration_seconds_bucket{le=\"+Inf\"} 2")
        );
        assert!(rendered.contains("usacc_rest_api_http_request_duration_seconds_count 2"));
        assert!(rendered.contains("usacc_rest_api_build_info{version=\"0.1.0\"} 1"));
        assert!(rendered.contains("usacc_rest_api_process_up 1"));
        assert!(!rendered.contains("user_id"));
        assert!(!rendered.contains("request_path"));
    }
}
