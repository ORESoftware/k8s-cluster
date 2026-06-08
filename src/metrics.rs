use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
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

    pub fn render(&self, database_configured: bool) -> String {
        format!(
            "# HELP usacc_rest_api_info Static service info.\n\
             # TYPE usacc_rest_api_info gauge\n\
             usacc_rest_api_info{{database_configured=\"{}\"}} 1\n\
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
            self.http_requests_total.load(Ordering::Relaxed),
            self.auth_failures_total.load(Ordering::Relaxed),
            self.db_queries_total.load(Ordering::Relaxed),
            self.db_errors_total.load(Ordering::Relaxed),
            self.votes_cast_total.load(Ordering::Relaxed),
            self.tallies_total.load(Ordering::Relaxed),
            self.simulations_total.load(Ordering::Relaxed),
            self.contract_requests_total.load(Ordering::Relaxed),
            self.contract_errors_total.load(Ordering::Relaxed),
        )
    }
}
