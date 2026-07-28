#!/usr/bin/env python3
"""Preserve operation-specific metadata while normalizing fleet routes."""

from pathlib import Path


GENERATOR = Path("remote/tools/generate-api-docs.mjs")
CHECK = Path(
    "remote/deployments/gleamlang-presence-server/scripts/check-openapi.sh"
)


def replace_once(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return source.replace(old, new, 1)


def replace_function(source: str, start_marker: str, end_marker: str, new: str) -> str:
    if source.count(start_marker) != 1 or source.count(end_marker) < 1:
        raise SystemExit(f"unable to locate function boundaries for {start_marker}")
    start = source.index(start_marker)
    end = source.index(end_marker, start)
    return source[:start] + new + source[end:]


def update_generator() -> None:
    source = GENERATOR.read_text(encoding="utf-8")
    method_aware_merge = r'''function mergeRoutes(routes) {
  const byPathAndMethod = new Map();
  for (const route of routes) {
    if (!route.path) {
      continue;
    }
    const methods = sortMethods(route.methods ?? []);
    if (methods.length === 0) {
      throw new Error(`route ${route.path} has no HTTP method`);
    }

    for (const method of methods) {
      const key = JSON.stringify([route.path, method]);
      let current = byPathAndMethod.get(key);
      if (!current) {
        current = {
          ...route,
          methods: [method],
          handlers: [],
          sourceFiles: new Set(),
        };
        byPathAndMethod.set(key, current);
      }

      if (route.handler) {
        current.handlers.push(route.handler);
      }
      for (const handler of route.handlers ?? []) {
        current.handlers.push(handler);
      }
      if (route.sourceFile) {
        current.sourceFiles.add(route.sourceFile);
      }
      for (const sourceFile of route.sourceFiles ?? []) {
        current.sourceFiles.add(sourceFile);
      }

      for (const hint of ['visibilityHint', 'authHint', 'routeTypeHint']) {
        if (route[hint] === undefined) continue;
        if (current[hint] !== undefined && current[hint] !== route[hint]) {
          throw new Error(
            `ambiguous ${hint} for ${method} ${route.path}: ${JSON.stringify(current[hint])} versus ${JSON.stringify(route[hint])}`,
          );
        }
        current[hint] = route[hint];
      }

      for (const hint of ['purposeHint', 'notes']) {
        if (!route[hint]) continue;
        if (current[hint] && current[hint] !== route[hint]) {
          throw new Error(
            `ambiguous ${hint} for ${method} ${route.path}: ${JSON.stringify(current[hint])} versus ${JSON.stringify(route[hint])}`,
          );
        }
        current[hint] = route[hint];
      }
    }
  }

  return [...byPathAndMethod.values()]
    .map((route) => ({
      ...route,
      handlers: [...new Set(route.handlers)].sort(),
      sourceFiles: [...route.sourceFiles].sort(),
    }))
    .sort((left, right) => {
      const pathOrder = left.path.localeCompare(right.path);
      if (pathOrder !== 0) return pathOrder;
      return left.methods[0].localeCompare(right.methods[0]);
    });
}'''
    source = replace_function(
        source,
        "function mergeRoutes(routes) {",
        "\n\nfunction normalizeRoutes",
        method_aware_merge,
    )

    old_existing_merge = r'''      const existing = pathItem[methodName];
      if (existing) {
        if (existing['x-dd-auth'] !== route.auth || existing['x-dd-visibility'] !== visibility || existing['x-dd-route-type'] !== route.routeType) {
          throw new Error(`ambiguous OpenAPI merge for ${docs.service} ${method} ${path}: query/path variants must share auth, visibility, and route type`);
        }
        existing.parameters = mergeOpenApiParameters(
          existing.parameters ?? [],
          openApiQueryParameters(route.path),
        );
        existing['x-dd-handlers'] = [...new Set([...(existing['x-dd-handlers'] ?? []), ...(route.handlers ?? [])])].sort();
        existing['x-dd-source-files'] = [...new Set([...(existing['x-dd-source-files'] ?? []), ...(route.sourceFiles ?? [])])].sort();
        existing['x-dd-source-paths'] = [...new Set([...(existing['x-dd-source-paths'] ?? []), route.path])].sort();
        continue;
      }'''
    new_existing_merge = r'''      const existing = pathItem[methodName];
      if (existing) {
        const invariantFields = {
          'x-dd-auth': route.auth,
          'x-dd-implementation': route.implementation,
          'x-dd-route-type': route.routeType,
          'x-dd-visibility': visibility,
        };
        for (const [field, value] of Object.entries(invariantFields)) {
          if (existing[field] !== value) {
            throw new Error(
              `ambiguous OpenAPI ${field} for ${docs.service} ${method} ${path}: ${JSON.stringify(existing[field])} versus ${JSON.stringify(value)}`,
            );
          }
        }
        const expectedSecurity = openApiSecurity(route);
        if (JSON.stringify(existing.security ?? null) !== JSON.stringify(expectedSecurity ?? null)) {
          throw new Error(
            `ambiguous OpenAPI security for ${docs.service} ${method} ${path}: query/path variants must share effective security`,
          );
        }

        const variants = existing['x-dd-source-variants'] ?? [
          {
            path: existing['x-dd-source-path'],
            summary: existing.summary,
            description: existing.description,
          },
        ];
        const candidate = {
          path: route.path,
          summary: route.purpose,
          description: route.notes || route.purpose,
        };
        const samePath = variants.find((variant) => variant.path === candidate.path);
        if (
          samePath &&
          (samePath.summary !== candidate.summary || samePath.description !== candidate.description)
        ) {
          throw new Error(
            `ambiguous OpenAPI source variant for ${docs.service} ${method} ${candidate.path}`,
          );
        }
        if (!samePath) {
          variants.push(candidate);
        }
        variants.sort((left, right) => {
          const pathOrder = left.path.localeCompare(right.path);
          if (pathOrder !== 0) return pathOrder;
          const summaryOrder = left.summary.localeCompare(right.summary);
          if (summaryOrder !== 0) return summaryOrder;
          return left.description.localeCompare(right.description);
        });
        existing['x-dd-source-variants'] = variants;

        const summaries = [...new Set(variants.map((variant) => variant.summary))];
        const descriptions = [...new Set(variants.map((variant) => variant.description))];
        existing.summary =
          summaries.length === 1 ? summaries[0] : `Route variants for ${method} ${path}`;
        existing.description =
          descriptions.length === 1
            ? descriptions[0]
            : [
                'Runtime route variants:',
                ...variants.map(
                  (variant) => `- ${variant.path}: ${variant.description}`,
                ),
              ].join('\n');

        existing.parameters = mergeOpenApiParameters(
          existing.parameters ?? [],
          openApiQueryParameters(route.path),
        );
        existing['x-dd-handlers'] = [...new Set([...(existing['x-dd-handlers'] ?? []), ...(route.handlers ?? [])])].sort();
        existing['x-dd-source-files'] = [...new Set([...(existing['x-dd-source-files'] ?? []), ...(route.sourceFiles ?? [])])].sort();
        existing['x-dd-source-paths'] = [...new Set([...(existing['x-dd-source-paths'] ?? []), route.path])].sort();
        continue;
      }'''
    source = replace_once(
        source,
        old_existing_merge,
        new_existing_merge,
        "preserve query variants without collapsing method metadata",
    )
    GENERATOR.write_text(source, encoding="utf-8")


def update_check() -> None:
    source = CHECK.read_text(encoding="utf-8")
    source = replace_once(
        source,
        "from collections import defaultdict\n",
        "",
        "remove obsolete path-level handler import",
    )
    old_handlers = r'''native_handlers_by_path = defaultdict(set)
for (path, _), operation in native_operations.items():
    native_handlers_by_path[path].add(operation['operationId'])

for key, native in native_operations.items():
    path, method = key
    projected = projected_operations[key]
    projected_handlers = projected['x-dd-handlers']
    assert len(projected_handlers) == len(set(projected_handlers)), (key, projected)
    assert set(projected_handlers) == native_handlers_by_path[path], (key, projected)
    assert native['operationId'] in projected_handlers, (key, projected)'''
    new_handlers = r'''for key, native in native_operations.items():
    path, method = key
    projected = projected_operations[key]
    projected_handlers = projected['x-dd-handlers']
    assert projected_handlers == [native['operationId']], (key, projected)'''
    source = replace_once(
        source,
        old_handlers,
        new_handlers,
        "require method-specific native handler provenance",
    )
    old_cleanliness = r'''git diff --cached --quiet
git diff --quiet
test -z "$(git status --short)"'''
    new_cleanliness = r'''if [[ "${OPENAPI_CHECK_ALLOW_DIRTY:-0}" != '1' ]]; then
  git diff --cached --quiet
  git diff --quiet
  test -z "$(git status --short)"
fi'''
    source = replace_once(
        source,
        old_cleanliness,
        new_cleanliness,
        "allow pre-commit regeneration validation",
    )
    CHECK.write_text(source, encoding="utf-8")


def main() -> None:
    update_generator()
    update_check()


if __name__ == "__main__":
    main()
