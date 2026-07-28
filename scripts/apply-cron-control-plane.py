from pathlib import Path
import base64
import gzip
import hashlib


EXPECTED_SOURCE_SHA256 = "8ead0feb226c0ef5bc97206a5736a32a85d9f29938d14effa1bbc3f98549b099"


def replace_once(source: str, old: str, new: str, label: str) -> str:
    if new in source:
        return source
    if source.count(old) != 1:
        raise SystemExit(f"{label} insertion point changed")
    return source.replace(old, new, 1)


encoded = Path(".agent/cron.rs.gz.b64").read_text().strip()
cron_source = gzip.decompress(base64.b64decode(encoded))
actual = hashlib.sha256(cron_source).hexdigest()
if actual != EXPECTED_SOURCE_SHA256:
    raise SystemExit(f"cron source digest mismatch: {actual}")
Path("src/cron.rs").write_bytes(cron_source)

main = Path("src/main.rs")
source = main.read_text()
source = replace_once(source, "mod billing;\n", "mod billing;\nmod cron;\n", "module")
source = replace_once(
    source,
    "mod security;\npub(crate) use security::*;\n",
    "mod security;\npub(crate) use security::*;\npub(crate) use cron::*;\n",
    "module export",
)
source = replace_once(
    source,
    "const MAX_BODY_BYTES: usize = 64 * 1024;",
    "const MAX_BODY_BYTES: usize = 384 * 1024;",
    "request body limit",
)
source = replace_once(
    source,
    "        request_security,\n    };",
    "        request_security,\n        cron_services: CronServices::from_env(),\n    };",
    "configuration initialization",
)
source = replace_once(
    source,
    "    let router = Router::new()\n",
    "    let router = cron_routes(Router::new())\n",
    "router",
)
source = replace_once(
    source,
    "    request_security: RequestSecurity,\n}",
    "    request_security: RequestSecurity,\n    /// Fail-closed trusted clients for the tenant-scoped scheduler and managed\n    /// function runtime. Browser credentials are never forwarded.\n    cron_services: CronServices,\n}",
    "AppConfig",
)
source = replace_once(
    source,
    "        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::OPTIONS])",
    "        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])",
    "customer CORS methods",
)
main.write_text(source)

views = Path("src/views.rs")
source = views.read_text()
source = replace_once(source, "    ApiKeys,\n    Security,", "    ApiKeys,\n    Crons,\n    Security,", "tab enum")
source = replace_once(source, "pub(crate) fn all() -> [CustomerTab; 7] {", "pub(crate) fn all() -> [CustomerTab; 8] {", "tab count")
source = replace_once(source, "            CustomerTab::ApiKeys,\n            CustomerTab::Security,", "            CustomerTab::ApiKeys,\n            CustomerTab::Crons,\n            CustomerTab::Security,", "tab list")
source = replace_once(source, "            CustomerTab::ApiKeys => \"/app/api-keys\",\n            CustomerTab::Security", "            CustomerTab::ApiKeys => \"/app/api-keys\",\n            CustomerTab::Crons => \"/app/crons\",\n            CustomerTab::Security", "tab href")
source = replace_once(source, "            CustomerTab::ApiKeys => \"API Keys\",\n            CustomerTab::Security", "            CustomerTab::ApiKeys => \"API Keys\",\n            CustomerTab::Crons => \"Cron Jobs\",\n            CustomerTab::Security", "tab label")
source = replace_once(
    source,
    "            CustomerTab::ApiKeys => \"Create, rotate, scope, and audit customer API keys for production integrations.\",\n            CustomerTab::Security",
    "            CustomerTab::ApiKeys => \"Create, rotate, scope, and audit customer API keys for production integrations.\",\n            CustomerTab::Crons => \"Create schedules, attach managed code or webhooks, run jobs on demand, and inspect the traceable run trail.\",\n            CustomerTab::Security",
    "tab description",
)
source = replace_once(source, "        CustomerTab::ApiKeys => api_keys_markup(org_id, csrf_token),\n        CustomerTab::Security", "        CustomerTab::ApiKeys => api_keys_markup(org_id, csrf_token),\n        CustomerTab::Crons => cron_markup(org_id, csrf_token),\n        CustomerTab::Security", "tab content")
views.write_text(source)

tests = Path("src/tests.rs")
source = tests.read_text()
old = '''        request_security: RequestSecurity::new(
            "https://app.fiducia.cloud",
            b"0123456789abcdef0123456789abcdef".to_vec(),
        )
        .unwrap(),
    }
'''
new = '''        request_security: RequestSecurity::new(
            "https://app.fiducia.cloud",
            b"0123456789abcdef0123456789abcdef".to_vec(),
        )
        .unwrap(),
        cron_services: CronServices::disabled(),
    }
'''
source = replace_once(source, old, new, "test configuration")
tests.write_text(source)
