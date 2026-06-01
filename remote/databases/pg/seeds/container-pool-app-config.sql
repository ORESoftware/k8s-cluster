-- Seed/update the default warm container pool config in RDS Postgres.
-- Apply after the shared schema in `remote/libs/pg-defs/schema/schema.sql`
-- (single source of truth for the `app_config` table this seed writes into).

insert into app_config (scope, key, value, version, status, labels, meta_data)
values (
  'default',
  'container-pool.runtime-pools.v1',
  '{
    "version": 1,
    "description": "Warm container pool definitions consumed by dd-container-pool.",
    "runtimeContract": {
      "defaultRequestPath": "/invoke",
      "defaultHealthPath": "/healthz",
      "defaultContainerPort": 8080,
      "managerInjectedEnv": [
        "PORT",
        "DD_POOL_ID",
        "DD_POOL_SLUG",
        "DD_POOL_CONTAINER_NAME",
        "DD_POOL_REQUEST_PATH",
        "DD_POOL_HEALTH_PATH",
        "NATS_URL",
        "DD_POOL_NATS_EVENT_SUBJECT",
        "DD_POOL_NATS_HEARTBEAT_SUBJECT"
      ],
      "natsEventPattern": "dd.remote.container_pool.<poolSlug>.events",
      "natsHeartbeatPattern": "dd.remote.container_pool.<poolSlug>.heartbeats"
    },
    "baseImages": [
      {
        "runtime": "nodejs",
        "image": "docker.io/library/dd-container-pool-nodejs-runtime:dev",
        "dockerfile": "remote/deployments/container-pool-rs/runtime-images/nodejs.Dockerfile",
        "buildContext": "remote/deployments/container-pool-rs"
      },
      {
        "runtime": "nodejs-chat-claude",
        "image": "docker.io/library/dd-dev-server:dev",
        "dockerfile": "remote/deployments/dev-server/Dockerfile",
        "buildContext": "remote/deployments/dev-server"
      },
      {
        "runtime": "rust",
        "image": "docker.io/library/dd-container-pool-rust-runtime:dev",
        "dockerfile": "remote/deployments/container-pool-rs/runtime-images/rust.Dockerfile",
        "buildContext": "remote/deployments/container-pool-rs"
      },
      {
        "runtime": "golang",
        "image": "docker.io/library/dd-container-pool-golang-runtime:dev",
        "dockerfile": "remote/deployments/container-pool-rs/runtime-images/golang.Dockerfile",
        "buildContext": "remote/deployments/container-pool-rs"
      },
      {
        "runtime": "python3",
        "image": "docker.io/library/dd-container-pool-python3-runtime:dev",
        "dockerfile": "remote/deployments/container-pool-rs/runtime-images/python3.Dockerfile",
        "buildContext": "remote/deployments/container-pool-rs"
      },
      {
        "runtime": "dart",
        "image": "docker.io/library/dd-container-pool-dart-runtime:dev",
        "dockerfile": "remote/deployments/container-pool-rs/runtime-images/dart.Dockerfile",
        "buildContext": "remote/deployments/container-pool-rs"
      },
      {
        "runtime": "gleamlang",
        "image": "docker.io/library/dd-container-pool-gleamlang-runtime:dev",
        "dockerfile": "remote/deployments/container-pool-rs/runtime-images/gleamlang.Dockerfile",
        "buildContext": "remote/deployments/container-pool-rs"
      },
      {
        "runtime": "erlang",
        "image": "docker.io/library/dd-container-pool-erlang-runtime:dev",
        "dockerfile": "remote/deployments/container-pool-rs/runtime-images/erlang.Dockerfile",
        "buildContext": "remote/deployments/container-pool-rs"
      },
      {
        "runtime": "browser-jobs",
        "image": "docker.io/library/dd-browser-job-worker:dev",
        "dockerfile": "remote/deployments/browser-job-runner-rs/worker/Dockerfile",
        "buildContext": "remote/deployments/browser-job-runner-rs/worker"
      }
    ],
    "pools": [
      {
        "slug": "nodejs",
        "displayName": "Node.js warm runtime",
        "image": "docker.io/library/dd-container-pool-nodejs-runtime:dev",
        "command": [],
        "env": {
          "DD_POOL_RUNTIME": "nodejs",
          "DD_POOL_HANDLER": "node /opt/dd-container-pool/handlers/nodejs-handler.mjs"
        },
        "requestPath": "/invoke",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "minWarm": 0,
        "maxWarm": 1,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 30000,
        "idleTtlSeconds": 900,
        "natsSubject": "dd.remote.container_pool.nodejs.requests",
        "labels": ["runtime", "nodejs"]
      },
      {
        "slug": "nodejs-chat-claude-live-mutex-dev",
        "displayName": "Node.js chat/Claude warm workers for ORESoftware/live-mutex dev",
        "image": "docker.io/library/dd-dev-server:dev",
        "command": [],
        "env": {
          "DD_REPO_URL": "git@github.com:ORESoftware/live-mutex.git",
          "BASE_BRANCH": "dev",
          "WORKER_BIND_MODE": "repo",
          "AGENT_PROVIDER": "claude-sdk",
          "WORKER_FANOUT_WS_BASE_URL": "ws://dd-gleamlang-server.default.svc.cluster.local:8081/worker-ws"
        },
        "requestPath": "/tasks",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "readOnly": false,
        "user": "1000:1000",
        "minWarm": 2,
        "maxWarm": 4,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 180000,
        "idleTtlSeconds": 1800,
        "natsSubject": "dd.remote.container_pool.nodejs-chat-claude-live-mutex-dev.requests",
        "labels": ["runtime", "nodejs", "agent", "claude", "repo:live-mutex"]
      },
      {
        "slug": "nodejs-chat-claude-k8s-cluster-dev",
        "displayName": "Node.js chat/Claude warm workers for ORESoftware/k8s-cluster dev",
        "image": "docker.io/library/dd-dev-server:dev",
        "command": [],
        "env": {
          "DD_REPO_URL": "git@github.com:ORESoftware/k8s-cluster.git",
          "BASE_BRANCH": "dev",
          "WORKER_BIND_MODE": "repo",
          "AGENT_PROVIDER": "claude-sdk",
          "WORKER_FANOUT_WS_BASE_URL": "ws://dd-gleamlang-server.default.svc.cluster.local:8081/worker-ws"
        },
        "requestPath": "/tasks",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "readOnly": false,
        "user": "1000:1000",
        "minWarm": 2,
        "maxWarm": 4,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 180000,
        "idleTtlSeconds": 1800,
        "natsSubject": "dd.remote.container_pool.nodejs-chat-claude-k8s-cluster-dev.requests",
        "labels": ["runtime", "nodejs", "agent", "claude", "repo:k8s-cluster"]
      },
      {
        "slug": "nodejs-chat-claude-us-anti-corruption-court-project-main",
        "displayName": "Node.js chat/Claude warm workers for ORESoftware/us-anti-corruption-court-project main",
        "image": "docker.io/library/dd-dev-server:dev",
        "command": [],
        "env": {
          "DD_REPO_URL": "git@github.com:ORESoftware/us-anti-corruption-court-project.git",
          "BASE_BRANCH": "main",
          "WORKER_BIND_MODE": "repo",
          "AGENT_PROVIDER": "claude-sdk",
          "WORKER_FANOUT_WS_BASE_URL": "ws://dd-gleamlang-server.default.svc.cluster.local:8081/worker-ws"
        },
        "requestPath": "/tasks",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "readOnly": false,
        "user": "1000:1000",
        "minWarm": 2,
        "maxWarm": 4,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 180000,
        "idleTtlSeconds": 1800,
        "natsSubject": "dd.remote.container_pool.nodejs-chat-claude-us-anti-corruption-court-project-main.requests",
        "labels": ["runtime", "nodejs", "agent", "claude", "repo:us-anti-corruption-court-project"]
      },
      {
        "slug": "rust",
        "displayName": "Rust warm runtime",
        "image": "docker.io/library/dd-container-pool-rust-runtime:dev",
        "command": [],
        "env": {
          "DD_POOL_RUNTIME": "rust",
          "DD_POOL_HANDLER": "/usr/local/bin/dd-pool-rust-handler"
        },
        "requestPath": "/invoke",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "minWarm": 0,
        "maxWarm": 2,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 45000,
        "idleTtlSeconds": 900,
        "natsSubject": "dd.remote.container_pool.rust.requests",
        "labels": ["runtime", "rust"]
      },
      {
        "slug": "golang",
        "displayName": "Go warm runtime",
        "image": "docker.io/library/dd-container-pool-golang-runtime:dev",
        "command": [],
        "env": {
          "DD_POOL_RUNTIME": "golang",
          "DD_POOL_HANDLER": "/usr/local/bin/dd-pool-golang-handler"
        },
        "requestPath": "/invoke",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "minWarm": 0,
        "maxWarm": 2,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 45000,
        "idleTtlSeconds": 900,
        "natsSubject": "dd.remote.container_pool.golang.requests",
        "labels": ["runtime", "go", "golang"]
      },
      {
        "slug": "python3",
        "displayName": "Python 3 warm runtime",
        "image": "docker.io/library/dd-container-pool-python3-runtime:dev",
        "command": [],
        "env": {
          "DD_POOL_RUNTIME": "python3",
          "DD_POOL_HANDLER": "python3 /opt/dd-container-pool/handlers/python3_handler.py"
        },
        "requestPath": "/invoke",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "minWarm": 0,
        "maxWarm": 1,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 30000,
        "idleTtlSeconds": 900,
        "natsSubject": "dd.remote.container_pool.python3.requests",
        "labels": ["runtime", "python3", "python"]
      },
      {
        "slug": "dart",
        "displayName": "Dart warm runtime",
        "image": "docker.io/library/dd-container-pool-dart-runtime:dev",
        "command": [],
        "env": {
          "DD_POOL_RUNTIME": "dart",
          "DD_POOL_HANDLER": "/usr/local/bin/dd-pool-dart-handler"
        },
        "requestPath": "/invoke",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "minWarm": 0,
        "maxWarm": 2,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 45000,
        "idleTtlSeconds": 900,
        "natsSubject": "dd.remote.container_pool.dart.requests",
        "labels": ["runtime", "dart"]
      },
      {
        "slug": "gleamlang",
        "displayName": "Gleam warm runtime",
        "image": "docker.io/library/dd-container-pool-gleamlang-runtime:dev",
        "command": [],
        "env": {
          "DD_POOL_RUNTIME": "gleamlang",
          "DD_POOL_HANDLER": "escript /opt/dd-container-pool/handlers/erlang-handler.escript"
        },
        "requestPath": "/invoke",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "minWarm": 0,
        "maxWarm": 2,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 45000,
        "idleTtlSeconds": 900,
        "natsSubject": "dd.remote.container_pool.gleamlang.requests",
        "labels": ["runtime", "gleam", "gleamlang", "erlang"]
      },
      {
        "slug": "erlang",
        "displayName": "Erlang warm runtime",
        "image": "docker.io/library/dd-container-pool-erlang-runtime:dev",
        "command": [],
        "env": {
          "DD_POOL_RUNTIME": "erlang",
          "DD_POOL_HANDLER": "escript /opt/dd-container-pool/handlers/erlang-handler.escript"
        },
        "requestPath": "/invoke",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "minWarm": 0,
        "maxWarm": 2,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 45000,
        "idleTtlSeconds": 900,
        "natsSubject": "dd.remote.container_pool.erlang.requests",
        "labels": ["runtime", "erlang", "beam"]
      },
      {
        "slug": "browser-jobs",
        "displayName": "Ephemeral Playwright/Puppeteer scraping workers",
        "image": "docker.io/library/dd-browser-job-worker:dev",
        "command": [],
        "env": {
          "BROWSER_JOB_HEADLESS": "true",
          "BROWSER_JOB_ALLOW_EVALUATE": "false",
          "BROWSER_JOB_MAX_MS": "540000"
        },
        "requestPath": "/run",
        "healthPath": "/healthz",
        "containerPort": 8080,
        "readOnly": false,
        "minWarm": 1,
        "maxWarm": 3,
        "maxConcurrencyPerContainer": 1,
        "requestTimeoutMs": 540000,
        "idleTtlSeconds": 1800,
        "natsSubject": "dd.remote.container_pool.browser-jobs.requests",
        "labels": ["runtime", "browser", "playwright", "puppeteer", "scraping"]
      }
    ]
  }'::jsonb,
  1,
  'active',
  '["container-pool", "runtime-images"]'::jsonb,
  '{"managedBy": "remote/databases/pg/seeds/container-pool-app-config.sql"}'::jsonb
)
on conflict (scope, key) do update set
  value = excluded.value,
  version = app_config.version + 1,
  status = excluded.status,
  labels = excluded.labels,
  meta_data = excluded.meta_data,
  is_soft_deleted = false,
  updated_at = now();
