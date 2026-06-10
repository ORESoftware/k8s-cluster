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
    "version": 2,
    "description": "Trading platform definitions consumed by dd-trading-server. v2 adds commodity/futures venues (TradeStation, Tradovate, Ironbeam, OANDA, Saxo, IG, CQG, AMP Futures) for 15+ active platforms. Credentials are Kubernetes secret key references, not raw secrets.",
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
        "slug": "tradestation",
        "displayName": "TradeStation",
        "provider": "tradestation",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["equities", "options", "futures", "commodities"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://sim-api.tradestation.com/v3",
          "live": "https://api.tradestation.com/v3"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["TRADESTATION_API_KEY", "TRADESTATION_API_SECRET"],
        "accountRefKey": "TRADESTATION_ACCOUNT_ID",
        "labels": ["brokerage", "futures", "commodities"],
        "metaData": {
          "notes": "Equities/options/futures broker with a documented HTTPS order API and a sim environment for paper validation."
        }
      },
      {
        "slug": "tradovate",
        "displayName": "Tradovate",
        "provider": "tradovate",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["futures", "commodities"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://demo.tradovateapi.com/v1",
          "live": "https://live.tradovateapi.com/v1"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["TRADOVATE_API_KEY", "TRADOVATE_API_SECRET"],
        "accountRefKey": "TRADOVATE_ACCOUNT_ID",
        "labels": ["futures", "commodities", "paper-first"],
        "metaData": {
          "notes": "Futures-first broker (energy, metals, ags, index) with demo and live REST hosts."
        }
      },
      {
        "slug": "ironbeam",
        "displayName": "Ironbeam",
        "provider": "ironbeam",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["futures", "commodities"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://demo.ironbeamapi.com/v2",
          "live": "https://live.ironbeamapi.com/v2"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["IRONBEAM_API_KEY", "IRONBEAM_API_SECRET"],
        "accountRefKey": "IRONBEAM_ACCOUNT_ID",
        "labels": ["futures", "commodities"],
        "metaData": {
          "notes": "CME-group commodity futures broker with a REST API and demo host."
        }
      },
      {
        "slug": "oanda",
        "displayName": "OANDA",
        "provider": "oanda",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["forex", "commodities", "metals", "indices"],
        "orderTypes": ["market", "limit", "stop", "trailing_stop"],
        "baseUrls": {
          "paper": "https://api-fxpractice.oanda.com/v3",
          "live": "https://api-fxtrade.oanda.com/v3"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["OANDA_API_TOKEN"],
        "accountRefKey": "OANDA_ACCOUNT_ID",
        "labels": ["forex", "commodities", "metals", "paper-first"],
        "metaData": {
          "notes": "v20 REST API covers spot metals (XAU/XAG) and commodity CFDs alongside FX; practice host is first-class."
        }
      },
      {
        "slug": "saxo",
        "displayName": "Saxo Bank",
        "provider": "saxo",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["futures", "commodities", "forex", "equities", "options"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://gateway.saxobank.com/sim/openapi",
          "live": "https://gateway.saxobank.com/openapi"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["SAXO_APP_KEY", "SAXO_APP_SECRET"],
        "accountRefKey": "SAXO_ACCOUNT_KEY",
        "labels": ["brokerage", "multi-asset", "commodities"],
        "metaData": {
          "notes": "OpenAPI multi-asset access including commodity futures and CFDs; SIM gateway is used for paper."
        }
      },
      {
        "slug": "ig",
        "displayName": "IG",
        "provider": "ig",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["commodities", "indices", "forex", "metals"],
        "orderTypes": ["market", "limit", "stop"],
        "baseUrls": {
          "paper": "https://demo-api.ig.com/gateway/deal",
          "live": "https://api.ig.com/gateway/deal"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["IG_API_KEY", "IG_API_SECRET"],
        "accountRefKey": "IG_ACCOUNT_ID",
        "labels": ["cfd", "commodities", "paper-first"],
        "metaData": {
          "notes": "REST dealing API for commodity/index/FX CFDs; demo gateway supports paper validation."
        }
      },
      {
        "slug": "cqg",
        "displayName": "CQG",
        "provider": "cqg",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["futures", "commodities", "metals", "energy", "agriculture"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://localhost:2845",
          "live": "https://localhost:2845"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["CQG_API_KEY", "CQG_API_SECRET"],
        "accountRefKey": "CQG_ACCOUNT_ID",
        "labels": ["futures", "commodities", "gateway"],
        "metaData": {
          "connector": "cqg-webapi-gateway",
          "notes": "CQG WebAPI is reached through an in-cluster gateway, so the base URL is the loopback gateway rather than a public host. The executor must terminate the gateway and supply CQG auth."
        }
      },
      {
        "slug": "amp-futures",
        "displayName": "AMP Futures",
        "provider": "amp-futures",
        "status": "active",
        "supportsPaper": true,
        "supportsLive": true,
        "assetClasses": ["futures", "commodities", "energy", "metals", "agriculture"],
        "orderTypes": ["market", "limit", "stop", "stop_limit"],
        "baseUrls": {
          "paper": "https://localhost:2846",
          "live": "https://localhost:2846"
        },
        "credentialSecret": "dd-trading-broker-secrets",
        "credentialKeys": ["AMP_FUTURES_API_KEY", "AMP_FUTURES_API_SECRET"],
        "accountRefKey": "AMP_FUTURES_ACCOUNT_ID",
        "labels": ["futures", "commodities", "gateway"],
        "metaData": {
          "connector": "cqg-or-rithmic-gateway",
          "notes": "AMP routes through CQG/Rithmic; orders flow via an in-cluster loopback gateway that the executor must bridge to the chosen routing network."
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
