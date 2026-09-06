from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CARRIER = ROOT / ".github/workflows/den-2786-hardening-carrier.yml"
PATCH_SCRIPT = Path(__file__).resolve()


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, content: str) -> None:
    (ROOT / path).write_text(content, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one exact match, found {count}")
    return text.replace(old, new, 1)


def regex_once(text: str, pattern: str, replacement: str, label: str) -> str:
    updated, count = re.subn(pattern, lambda _: replacement, text)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return updated


provider_path = "remote/deployments/benefactor-orchestrator/provider-diagnostics-bridge.mjs"
provider = read(provider_path)
provider = replace_once(
    provider,
    "const PROVIDER_DIAGNOSTICS_VERSION = 'benefactor.provider-diagnostics.v1';\nconst INSTALL_MARKER = Symbol.for('benefactor.providerDiagnostics.installed');\nconst STATE_MARKER = Symbol.for('benefactor.providerDiagnostics.state');\n",
    "const PROVIDER_DIAGNOSTICS_VERSION = 'benefactor.provider-diagnostics.v1';\n",
    "provider marker removal",
)
provider = replace_once(
    provider,
    "function createProviderState() {",
    "export function createProviderState() {",
    "provider state export",
)
provider = regex_once(
    provider,
    r"\nexport function installProviderDiagnostics\([\s\S]*\Z",
    "\n" + 'export function formatProviderDiagnosticsLine(state = createProviderState()) {\n  return `BENEFACTOR_PROVIDER_DIAGNOSTICS ${JSON.stringify(buildProviderDiagnostics(state))}`;\n}\n',
    "provider installer removal",
)
write(provider_path, provider)

scraper_path = "remote/deployments/benefactor-orchestrator/scraper-contact-bridge.mjs"
scraper = read(scraper_path)
scraper = replace_once(
    scraper,
    "const BRIDGE_INSTALLED = Symbol.for('benefactor.scraperContactBridge.installed');\n",
    "",
    "scraper marker removal",
)
scraper = regex_once(
    scraper,
    r"\nexport function installScraperContactBridge\([\s\S]*\Z",
    "\n",
    "scraper installer removal",
)
write(scraper_path, scraper)

provider_test_path = "remote/deployments/benefactor-orchestrator/provider-diagnostics-bridge.test.mjs"
provider_test = read(provider_test_path)
provider_test = replace_once(
    provider_test,
    '  buildProviderDiagnostics,\n  createProviderDiagnosticsFetch,\n  installProviderDiagnostics,\n  providerFailureCode,\n',
    '  buildProviderDiagnostics,\n  createProviderDiagnosticsFetch,\n  createProviderState,\n  formatProviderDiagnosticsLine,\n  providerFailureCode,\n',
    "provider test imports",
)
provider_test = replace_once(
    provider_test,
    'function state() {\n  return {\n    brave: { requests: 0, failures: 0, failureCodes: {}, pendingFailureWarnings: 0 },\n    serper: { requests: 0, failures: 0, failureCodes: {}, pendingFailureWarnings: 0 },\n  };\n}\n',
    'function state() {\n  return createProviderState();\n}\n',
    "provider test state factory",
)
provider_test = regex_once(
    provider_test,
    r"\ntest\('installed bridge emits diagnostics only after a pipeline report',[\s\S]*\Z",
    "\n" + "test('explicit diagnostics formatting emits a bounded line without replacing globals', async () => {\n  const diagnostics = createProviderState();\n  const originalGlobalFetch = globalThis.fetch;\n  const originalConsoleLog = console.log;\n  const originalConsoleWarn = console.warn;\n  const wrapped = createProviderDiagnosticsFetch(\n    async () => new Response('{}', { status: 200 }),\n    diagnostics,\n  );\n\n  await wrapped('https://google.serper.dev/search');\n  assert.equal(\n    recordProviderWarning(\n      diagnostics,\n      '[benefactor-pipeline] provider=serper search_failed ResponseLimitError',\n    ),\n    true,\n  );\n\n  const prefix = 'BENEFACTOR_PROVIDER_DIAGNOSTICS ';\n  const line = formatProviderDiagnosticsLine(diagnostics);\n  assert.ok(line.startsWith(prefix));\n  const diagnostic = JSON.parse(line.slice(prefix.length));\n  assert.deepEqual(diagnostic.providers.find((item) => item.provider === 'serper'), {\n    provider: 'serper',\n    requests: 1,\n    successes: 0,\n    failures: 1,\n    failureCodes: { response_limit: 1 },\n  });\n  assert.doesNotMatch(JSON.stringify(diagnostic), /sensitive|quota|query|url/i);\n  assert.equal(globalThis.fetch, originalGlobalFetch);\n  assert.equal(console.log, originalConsoleLog);\n  assert.equal(console.warn, originalConsoleWarn);\n});\n",
    "provider test replacement",
)
write(provider_test_path, provider_test)

scraper_test_path = "remote/deployments/benefactor-orchestrator/scraper-contact-bridge.test.mjs"
scraper_test = read(scraper_test_path)
scraper_test = regex_once(
    scraper_test,
    r"\ntest\('container startup preloads only the contact compatibility bridge',[\s\S]*\Z",
    "\n" + "test('runtime integration is explicit and container startup has no preload hook', () => {\n  const dockerfile = readFileSync(new URL('./Dockerfile', import.meta.url), 'utf8');\n  const runtime = readFileSync(new URL('./orchestrator-runtime.mjs', import.meta.url), 'utf8');\n  const orchestrator = readFileSync(new URL('./orchestrate.mjs', import.meta.url), 'utf8');\n\n  assert.doesNotMatch(dockerfile, /NODE_OPTIONS|scraper-contact-preload/);\n  assert.match(runtime, /createScraperContactFetch/);\n  assert.match(runtime, /createProviderDiagnosticsFetch/);\n  assert.match(orchestrator, /createBenefactorRuntime/);\n  assert.match(orchestrator, /runtime\\.fetch/);\n});\n",
    "scraper test replacement",
)
write(scraper_test_path, scraper_test)

orchestrate_path = "remote/deployments/benefactor-orchestrator/orchestrate.mjs"
orchestrate = read(orchestrate_path)
orchestrate = replace_once(orchestrate, "import { createSearchProviders } from './providers/index.mjs';\n", "import { createSearchProviders } from './providers/index.mjs';\nimport { createBenefactorRuntime } from './orchestrator-runtime.mjs';\n", "runtime import")
orchestrate = replace_once(orchestrate, "if (!/^[a-z0-9][a-z0-9._:-]{0,127}$/i.test(config.scrapeRequestType)) {\n  throw new Error('SCRAPE_REQUEST_TYPE contains unsupported characters');\n}\nconst statuses = providerStatuses(config);\n", "if (!/^[a-z0-9][a-z0-9._:-]{0,127}$/i.test(config.scrapeRequestType)) {\n  throw new Error('SCRAPE_REQUEST_TYPE contains unsupported characters');\n}\nconst runtime = createBenefactorRuntime({\n  scraperUrl: config.scraperUrl,\n  requireRoleEmail: config.requireRoleEmail,\n});\nconst statuses = providerStatuses(config);\n", "runtime construction")
orchestrate = replace_once(orchestrate, 'function providerLog(provider, message) {\n  console.warn(`[benefactor-pipeline] provider=${provider} ${sanitizeLogValue(message)}`);\n}\n', 'function providerLog(provider, message) {\n  const line = `[benefactor-pipeline] provider=${provider} ${sanitizeLogValue(message)}`;\n  console.warn(line);\n  runtime.recordProviderWarning(line);\n}\n', "warning instrumentation")
orchestrate = replace_once(orchestrate, '    const response = await fetch(url, { ...init, signal: controller.signal });', '    const response = await runtime.fetch(url, { ...init, signal: controller.signal });', "explicit runtime fetch")
orchestrate = replace_once(orchestrate, "  console.log(`BENEFACTOR_PIPELINE_REPORT ${canonicalJson(report)}`);\n  console.log(`[benefactor-pipeline] done category=${config.category} mode=${config.dryRun ? 'dry-run' : 'persist'} contacts=${collected.size} inserted=${counters.leadsInserted} report=${report.reportDigest}`);\n", "  console.log(`BENEFACTOR_PIPELINE_REPORT ${canonicalJson(report)}`);\n  console.log(runtime.providerDiagnosticsLine());\n  console.log(`[benefactor-pipeline] done category=${config.category} mode=${config.dryRun ? 'dry-run' : 'persist'} contacts=${collected.size} inserted=${counters.leadsInserted} report=${report.reportDigest}`);\n", "explicit diagnostics emission")
write(orchestrate_path, orchestrate)

dockerfile_path = "remote/deployments/benefactor-orchestrator/Dockerfile"
dockerfile = read(dockerfile_path)
dockerfile = replace_once(
    dockerfile,
    "\nENV NODE_OPTIONS=--import=/work/scraper-contact-preload.mjs\n",
    "\n",
    "Docker preload removal",
)
write(dockerfile_path, dockerfile)

package_path = ROOT / "remote/deployments/benefactor-orchestrator/package.json"
package = json.loads(package_path.read_text(encoding="utf-8"))
package["scripts"]["check"] = 'node --check pipeline-lib.mjs && node --check providers/serper.mjs && node --check providers/brave.mjs && node --check providers/index.mjs && node --check provider-diagnostics-bridge.mjs && node --check scraper-contact-bridge.mjs && node --check orchestrator-runtime.mjs && node --check orchestrate.mjs && node --check contact-batch-lib.mjs && node --check contact-batch.mjs && node --check hubspot-sync-lib.mjs && node --check hubspot-sync.mjs && node --check provider-diagnostics-bridge.test.mjs && node --check scraper-contact-bridge.test.mjs && node --check orchestrator-runtime.test.mjs'
package["scripts"]["test"] = 'node --test orchestrate.test.mjs contact-batch.test.mjs hubspot-sync.test.mjs scraper-contact-bridge.test.mjs provider-diagnostics-bridge.test.mjs orchestrator-runtime.test.mjs'
package_path.write_text(json.dumps(package, indent=2) + "\n", encoding="utf-8")

preload = ROOT / "remote/deployments/benefactor-orchestrator/scraper-contact-preload.mjs"
if not preload.is_file():
    raise SystemExit("expected scraper preload file is absent")
preload.unlink()

for path in (
    provider_path,
    scraper_path,
    provider_test_path,
    scraper_test_path,
    orchestrate_path,
    "remote/deployments/benefactor-orchestrator/orchestrator-runtime.mjs",
    "remote/deployments/benefactor-orchestrator/orchestrator-runtime.test.mjs",
):
    source = read(path)
    for token in (
        "globalThis.fetch =",
        "target.fetch =",
        "console.warn =",
        "console.log =",
        "installProviderDiagnostics",
        "installScraperContactBridge",
    ):
        if token in source:
            raise SystemExit(f"{path} retains forbidden compatibility token: {token}")

if not CARRIER.is_file():
    raise SystemExit("hardening carrier file is absent")
CARRIER.unlink()
PATCH_SCRIPT.unlink()
