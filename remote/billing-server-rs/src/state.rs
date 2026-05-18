use sqlx::PgPool;
use std::sync::Arc;

use crate::config::Config;
use crate::crypto::Sealer;
use crate::customers::CustomerService;
use crate::ledger::LedgerService;
use crate::locks::LockService;
use crate::providers::connection::ConnectionService;
use crate::solana::{AnchorService, SolanaClient};
use crate::solana::verify::Verifier;
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
    pub anchor: Arc<AnchorService>,
    pub verifier: Arc<Verifier>,
    pub scheduler: Arc<crate::scheduler::SchedulerService>,
    pub notifications: Arc<crate::notifications::NotificationService>,
}

impl AppState {
    pub fn new(cfg: Arc<Config>, pool: PgPool, sealer: Arc<Sealer>) -> Self {
        let tenants = TenantService::new(pool.clone());
        let users = UserService::new(pool.clone());
        let ledger = LedgerService::new(pool.clone());
        let customers = CustomerService::new(pool.clone(), users.clone(), ledger.clone());
        let vendors = VendorService::new(pool.clone(), users.clone(), ledger.clone());
        let connections = ConnectionService::new(pool.clone(), sealer);
        let locks = LockService::new(pool.clone());
        let notifications = Arc::new(
            crate::notifications::NotificationService::new(pool.clone()),
        );
        let scheduler = Arc::new(crate::scheduler::SchedulerService::new(pool.clone()));

        let solana_client = SolanaClient::new(&cfg);
        let anchor = Arc::new(AnchorService::new(pool.clone(), solana_client.clone()));
        let verifier = Arc::new(Verifier::new(pool.clone(), solana_client));

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
            anchor,
            verifier,
            scheduler,
            notifications,
        }
    }
}
