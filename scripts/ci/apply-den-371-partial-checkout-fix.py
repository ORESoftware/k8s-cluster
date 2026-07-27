#!/usr/bin/env python3
from pathlib import Path

path = Path("remote/tools/generate-api-docs.mjs")
text = path.read_text(encoding="utf-8")


def replace_once_or_verify(before: str, after: str, label: str) -> None:
    global text
    if after in text:
        return
    count = text.count(before)
    if count != 1:
        raise SystemExit(f"{label}: expected one source match, found {count}")
    text = text.replace(before, after, 1)


replace_once_or_verify(
    "import { existsSync } from 'node:fs';\n",
    "import { execFileSync } from 'node:child_process';\nimport { existsSync } from 'node:fs';\n",
    "child-process import",
)

replace_once_or_verify(
    """async function readUtf8(path) {
  return readFile(path, 'utf8');
}

function sortMethods(methods) {
""",
    """async function readUtf8(path) {
  return readFile(path, 'utf8');
}

function deploymentGitlinks() {
  const output = execFileSync(
    'git',
    ['ls-files', '--stage', '--', 'remote/deployments'],
    { cwd: repoRoot, encoding: 'utf8' },
  );
  return new Set(
    output
      .split(/\\r?\\n/)
      .map((line) => line.match(/^160000 [0-9a-f]{40} \\d+\\t(.+)$/)?.[1])
      .filter(Boolean),
  );
}

async function unavailableIndexedGitlinkServices(indexPath) {
  if (!existsSync(indexPath)) return [];
  const gitlinks = deploymentGitlinks();
  const current = JSON.parse(await readUtf8(indexPath));
  return (current.services ?? [])
    .filter((service) => {
      const generatedPath = service.generated?.[0];
      if (typeof generatedPath !== 'string') return false;
      const marker = '/generated/';
      const markerIndex = generatedPath.indexOf(marker);
      if (markerIndex < 0) return false;
      const deploymentPath = generatedPath.slice(0, markerIndex);
      return (
        gitlinks.has(deploymentPath) &&
        !existsSync(resolve(repoRoot, deploymentPath, '.git'))
      );
    })
    .map((service) => service.service)
    .filter((service) => typeof service === 'string')
    .sort();
}

function sortMethods(methods) {
""",
    "partial-checkout helpers",
)

replace_once_or_verify(
    """  if (!serviceFilter) {
    await writeOrCheck(
      resolve(repoRoot, 'remote/deployments/generated-api-docs-index.json'),
      `${JSON.stringify(indexPayload, null, 2)}\\n`,
    );
    await writeOrCheck(
      resolve(repoRoot, 'remote/deployments/generated-api-docs-index.html'),
      renderDocsIndexHtml(indexItems),
    );
  }
""",
    """  if (!serviceFilter) {
    const centralIndexJson = resolve(
      repoRoot,
      'remote/deployments/generated-api-docs-index.json',
    );
    const centralIndexHtml = resolve(
      repoRoot,
      'remote/deployments/generated-api-docs-index.html',
    );
    const unavailableServices = await unavailableIndexedGitlinkServices(centralIndexJson);
    if (unavailableServices.length > 0) {
      for (const path of [centralIndexJson, centralIndexHtml]) {
        if (!existsSync(path)) {
          throw new Error(
            `missing central API docs index during partial checkout: ${relative(repoRoot, path)}`,
          );
        }
      }
      console.log(
        `preserved central API docs index because ${unavailableServices.length} indexed gitlink service(s) are not initialized: ${unavailableServices.join(', ')}`,
      );
    } else {
      await writeOrCheck(
        centralIndexJson,
        `${JSON.stringify(indexPayload, null, 2)}\\n`,
      );
      await writeOrCheck(centralIndexHtml, renderDocsIndexHtml(indexItems));
    }
  }
""",
    "central index partial-checkout behavior",
)

path.write_text(text, encoding="utf-8")
print("Applied DEN-371 partial-checkout generator behavior.")
