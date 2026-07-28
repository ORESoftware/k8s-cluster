#!/usr/bin/env python3

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GENERATOR = ROOT / "remote/tools/generate-api-docs.mjs"
CONTRACT = ROOT / "remote/deployments/browser-test-server/src/api-contract.ts"


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    source = path.read_text(encoding="utf-8")
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{path.relative_to(ROOT)}: {label}: expected one match, found {count}")
    path.write_text(source.replace(old, new, 1), encoding="utf-8")


replace_once(
    GENERATOR,
    """    const deploymentDir = resolve(repoRoot, spec.deploymentDir ?? dirname(dirname(files[0])));
    const openapiFile = join(deploymentDir, 'generated/openapi.json');
    const rawRoutes = (await pathExists(openapiFile))
      ? extractOpenApiRoutes(JSON.parse(await readUtf8(openapiFile)), openapiFile)
      : [];
""",
    """    const deploymentDir = resolve(repoRoot, spec.deploymentDir ?? dirname(dirname(files[0])));
    const openapiFile = join(deploymentDir, 'generated/openapi.json');
    const canonicalOpenApi = (await pathExists(openapiFile))
      ? JSON.parse(await readUtf8(openapiFile))
      : null;
    const rawRoutes = canonicalOpenApi
      ? extractOpenApiRoutes(canonicalOpenApi, openapiFile)
      : [];
""",
    "retain the native OpenAPI document instead of reducing it to route metadata",
)

replace_once(
    GENERATOR,
    """      moduleDir: dirname(files[0]),
      outputName: spec.outputName ?? 'api-docs',
      routes: normalizeRoutes(spec.service, rawRoutes),
""",
    """      moduleDir: dirname(files[0]),
      outputName: spec.outputName ?? 'api-docs',
      canonicalOpenApi,
      routes: normalizeRoutes(spec.service, rawRoutes),
""",
    "carry the native document into artifact generation",
)

replace_once(
    GENERATOR,
    """    const docs = buildDocs(service);
    const internalOpenapi = buildOpenApi(docs);
    const publicOpenapi = buildPublicOpenApi(internalOpenapi);
""",
    """    const docs = buildDocs(service);
    const internalOpenapi = service.canonicalOpenApi
      ? structuredClone(service.canonicalOpenApi)
      : buildOpenApi(docs);
    const publicOpenapi = buildPublicOpenApi(internalOpenapi);
""",
    "use the executable native document as the canonical internal artifact",
)

replace_once(
    CONTRACT,
    "Fail-closed public subset. Only explicitly public operations are included.",
    "Fail-closed public subset. Only operations explicitly marked public are included.",
    "align the executable public projection description with the fleet projection",
)

print("applied canonical browser-test OpenAPI synchronization patch")
