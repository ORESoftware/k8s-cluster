use sqlx::PgPool;
use std::sync::Arc;

use crate::config::Config;
use crate::crypto::Sealer;
use crate::customer_locks::CustomerLockBroker;
use crate::customers::CustomerService;
use crate::ledger::LedgerService;
use crate::locks::LockService;
use crate::providers::connection::ConnectionService;
use crate::providers::plaid::PlaidWebhookVerifier;
use crate::solana::verify::Verifier;
use crate::solana::{AnchorService, SolanaClient};
use crate::tenants::TenantService;
use crate::users::UserService;
use crate::vendors::VendorService;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub pool: PgPool,
    pub tenants: TenantService,
    pub users: UserService,
    pub ledger: LedgerService,
    pub customers: CustomerService,
    pub vendors: VendorService,
    pub connections: ConnectionService,
    pub locks: LockService,
    pub solana_client: SolanaClient,
    pub anchor: Arc<AnchorService>,
    pub verifier: Arc<Verifier>,
    pub scheduler: Arc<crate::scheduler::SchedulerService>,
    pub notifications: Arc<crate::notifications::NotificationService>,
    pub plaid_webhook_verifier: PlaidWebhookVerifier,
}

impl AppState {
    pub fn new(cfg: Arc<Config>, pool: PgPool, sealer: Arc<Sealer>) -> Self {
        let tenants = TenantService::new(pool.clone());
        let users = UserService::new(pool.clone());
        let customer_locks = CustomerLockBroker::from_config(&cfg);
        let ledger = LedgerService::new(pool.clone(), customer_locks.clone());
        let customers = CustomerService::new(pool.clone(), users.clone(), customer_locks.clone());
        let vendors = VendorService::new(pool.clone(), users.clone(), ledger.clone());
        let connections = ConnectionService::new(pool.clone(), sealer);
        let locks = LockService::new(pool.clone());
        let notifications = Arc::new(crate::notifications::NotificationService::new(pool.clone()));
        let scheduler = Arc::new(crate::scheduler::SchedulerService::new(pool.clone()));

        let solana_client = SolanaClient::new(&cfg);
        let anchor = Arc::new(AnchorService::new(pool.clone(), solana_client.clone()));
        let verifier = Arc::new(Verifier::new(pool.clone(), solana_client.clone()));

        Self {
            cfg,
            pool,
            tenants,
            users,
            ledger,
            customers,
            vendors,
            connections,
            locks,
            solana_client,
            anchor,
            verifier,
            scheduler,
            notifications,
            plaid_webhook_verifier: PlaidWebhookVerifier::new(),
        }
    }
}
