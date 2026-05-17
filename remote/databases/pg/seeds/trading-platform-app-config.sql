-- Seed/update default algorithmic trading platform config in RDS Postgres.
-- Apply after the shared schema in `remote/libs/pg-defs/schema/schema.sql`
-- (single source of truth for the `app_config` table this seed writes into).
--
-- This stores broker/platform metadata only. API keys, account IDs, refresh tokens,
-- and gateway credentials stay in Kubernetes/AWS Secrets Manager.

insert into app_config (scope, key, value, version, status, labels, meta_data)
values (
  'default',
  'trading.platforms.v1',
  '{
    "version": 1,
    "description": "Trading platform definitions consumed by dd-trading-server. Credentials are Kubernetes secret key references, not raw secrets.",
    "defaultPlatform": "interactive-brokers",
    "platforms": [
      {
        "slug": "interactive-brokers",
        "displayName": "Interactive Brokers",
        "provider": "interactive-brokers",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["equities", "options", "futures", "forex", "bonds", "funds"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://localhost:5000/v1/api",
          "live": "https://localhost:5000/v1/api"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["IBKR_GATEWAY_URL", "IBKR_ACCOUNT_ID"],
        "accountRefKey": "IBKR_ACCOUNT_ID",
        "labels": ["brokerage", "multi-asset", "primary"],
        "metaData": {
          "connector": "tws-or-ib-gateway",
          "notes": "The service emits intents only; an executor must bridge to Client Portal/TWS/IB Gateway."
        }
      },
      {
        "slug": "alpaca",
        "displayName": "Alpaca",
        "provider": "alpaca",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["equities", "options", "crypto"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://paper-api.alpaca.markets",
          "live": "https://api.alpaca.markets"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["ALPACA_API_KEY_ID", "ALPACA_API_SECRET_KEY"],
        "labels": ["brokerage", "paper-first"],
        "metaData": {
          "notes": "Good candidate for first executor integration because paper trading is first-class."
        }
      },
      {
        "slug": "tradier",
        "displayName": "Tradier",
        "provider": "tradier",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["equities", "options"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://sandbox.tradier.com/v1",
          "live": "https://api.tradier.com/v1"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["TRADIER_ACCESS_TOKEN", "TRADIER_ACCOUNT_ID"],
        "accountRefKey": "TRADIER_ACCOUNT_ID",
        "labels": ["brokerage", "options"],
        "metaData": {
          "notes": "Sandbox and live are represented as separate base URL modes."
        }
      },
      {
        "slug": "coinbase-advanced-trade",
        "displayName": "Coinbase Advanced Trade",
        "provider": "coinbase",
        "status": "active",
        "supportsPaper": false,
        "supportsLive": true,
        "assetClasses": ["crypto"],
        "orderTypes": ["market", "limit", "stop_limit"],
        "baseUrls": {
          "live": "https://api.coinbase.com/api/v3/brokerage"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["COINBASE_API_KEY", "COINBASE_API_SECRET"],
        "labels": ["crypto"],
        "metaData": {
          "notes": "No paper endpoint is enabled by default; live mode still emits intents only."
        }
      },
      {
        "slug": "kraken",
        "displayName": "Kraken",
        "provider": "kraken",
        "status": "active",
        "supportsPaper": false,
        "supportsLive": true,
        "assetClasses": ["crypto"],
        "orderTypes": ["market", "limit", "stop_loss", "take_profit"],
        "baseUrls": {
          "live": "https://api.kraken.com"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["KRAKEN_API_KEY", "KRAKEN_API_SECRET"],
        "labels": ["crypto"],
        "metaData": {
          "notes": "Spot REST trading config. No paper endpoint is enabled by default; live mode still emits intents only."
        }
      },
      {
        "slug": "gemini",
        "displayName": "Gemini",
        "provider": "gemini",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["crypto"],
        "orderTypes": ["market", "limit"],
        "baseUrls": {
          "paper": "https://api.sandbox.gemini.com",
          "live": "https://api.gemini.com"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["GEMINI_API_KEY", "GEMINI_API_SECRET"],
        "labels": ["crypto", "paper-first"],
        "metaData": {
          "notes": "Sandbox URL is available for paper-mode executor validation."
        }
      },
      {
        "slug": "binance-us",
        "displayName": "Binance.US",
        "provider": "binance-us",
        "status": "active",
        "supportsPaper": false,
        "supportsLive": true,
        "assetClasses": ["crypto"],
        "orderTypes": ["market", "limit", "stop_limit"],
        "baseUrls": {
          "live": "https://api.binance.us"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["BINANCE_US_API_KEY", "BINANCE_US_API_SECRET"],
        "labels": ["crypto"],
        "metaData": {
          "notes": "Exchange API trading config. No paper endpoint is enabled by default; live mode still emits intents only."
        }
      },
      {
        "slug": "polymarket",
        "displayName": "Polymarket",
        "provider": "polymarket",
        "status": "paused",
        "supportsPaper": false,
        "supportsLive": true,
        "assetClasses": ["prediction-markets", "crypto"],
        "orderTypes": ["market", "limit"],
        "baseUrls": {
          "live": "https://clob.polymarket.com"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["POLYMARKET_PRIVATE_KEY", "POLYMARKET_FUNDER_ADDRESS"],
        "labels": ["prediction-market", "crypto"],
        "metaData": {
          "notes": "Paused until the executor enforces wallet signing, CLOB auth, geo eligibility, and market-rule checks."
        }
      },
      {
        "slug": "factmachine",
        "displayName": "FactMachine",
        "provider": "factmachine",
        "status": "paused",
        "supportsPaper": false,
        "supportsLive": false,
        "assetClasses": ["prediction-markets", "data"],
        "orderTypes": [],
        "baseUrls": {},
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["FACTMACHINE_API_KEY", "FACTMACHINE_BASE_URL"],
        "labels": ["prediction-market", "research", "placeholder"],
        "metaData": {
          "endpointStatus": "not-configured",
          "notes": "Placeholder profile until a FactMachine trading/data API contract is supplied."
        }
      }
    ]
  }'::jsonb,
  1,
  'active',
  '["trading", "brokers", "order-intents"]'::jsonb,
  '{"managedBy": "remote/databases/pg/seeds/trading-platform-app-config.sql"}'::jsonb
)
on conflict (scope, key) do update set
  value = excluded.value,
  version = app_config.version + 1,
  status = excluded.status,
  labels = excluded.labels,
  meta_data = excluded.meta_data,
  is_soft_deleted = false,
  updated_at = now();
