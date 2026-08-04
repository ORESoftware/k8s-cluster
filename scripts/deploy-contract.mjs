import path from "node:path";

function indentation(line) {
  if (line.includes("\t")) {
    throw new Error("workflow contract does not permit tab indentation");
  }
  return line.match(/^ */)?.[0].length ?? 0;
}

function yamlScalar(value) {
  const trimmed = value.trim();
  if (
    trimmed.length >= 2 &&
    ((trimmed.startsWith('"') && trimmed.endsWith('"')) ||
      (trimmed.startsWith("'") && trimmed.endsWith("'")))
  ) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

export function publicEnvKeysForNamedStep(workflow, stepName) {
  const lines = workflow.split(/\r?\n/);
  const matches = [];

  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(/^( *)-\s+name:\s*(.+?)\s*$/);
    if (match && yamlScalar(match[2]) === stepName) {
      matches.push({ index, indent: match[1].length });
    }
  }

  if (matches.length !== 1) {
    throw new Error(
      `expected exactly one workflow step named ${JSON.stringify(stepName)}, found ${matches.length}`,
    );
  }

  const [{ index: stepStart, indent: stepIndent }] = matches;
  let stepEnd = lines.length;
  for (let index = stepStart + 1; index < lines.length; index += 1) {
    const line = lines[index];
    if (!line.trim() || line.trimStart().startsWith("#")) continue;
    if (indentation(line) <= stepIndent) {
      stepEnd = index;
      break;
    }
  }

  const envBlocks = [];
  for (let index = stepStart + 1; index < stepEnd; index += 1) {
    const match = lines[index].match(/^( *)env:\s*(?:#.*)?$/);
    if (match && match[1].length > stepIndent) {
      envBlocks.push({ index, indent: match[1].length });
    }
  }

  if (envBlocks.length !== 1) {
    throw new Error(
      `expected step ${JSON.stringify(stepName)} to contain exactly one block-style env map, ` +
        `found ${envBlocks.length}`,
    );
  }

  const [{ index: envStart, indent: envIndent }] = envBlocks;
  const keys = new Set();
  for (let index = envStart + 1; index < stepEnd; index += 1) {
    const line = lines[index];
    if (!line.trim() || line.trimStart().startsWith("#")) continue;
    const currentIndent = indentation(line);
    if (currentIndent <= envIndent) break;
    if (currentIndent !== envIndent + 2) continue;
    const match = line.match(/^\s*(PUBLIC_[A-Z0-9_]+)\s*:/);
    if (match) keys.add(match[1]);
  }
  return keys;
}

export function publicKeysFromDotenv(dotenv) {
  return new Set(
    [...dotenv.matchAll(/^\s*(?:export\s+)?(PUBLIC_[A-Z0-9_]+)\s*=/gm)].map(
      (match) => match[1],
    ),
  );
}

export function resolveBuildPath(urlPath, dist) {
  if (!urlPath.startsWith("/")) {
    throw new Error(`build path must be root-relative: ${urlPath}`);
  }

  let decoded;
  try {
    decoded = decodeURIComponent(urlPath);
  } catch (error) {
    throw new Error(`build path is not valid percent-encoding: ${urlPath}`, { cause: error });
  }

  const root = path.resolve(dist);
  const relative = decoded.replace(/^\/+/, "");
  const resolved = path.resolve(root, relative);
  if (resolved !== root && !resolved.startsWith(`${root}${path.sep}`)) {
    throw new Error(`build path escapes dist/: ${urlPath}`);
  }
  if (!relative || decoded.endsWith("/")) {
    return path.join(resolved, "index.html");
  }
  return resolved;
}

export function resolveSameOriginBuildTarget(value, productionHost, dist) {
  if (!/^https?:\/\//i.test(value)) return null;

  let candidate;
  let production;
  try {
    candidate = new URL(value);
    production = new URL(productionHost);
  } catch (error) {
    throw new Error(`metadata URL is malformed: ${value}`, { cause: error });
  }

  if (candidate.origin !== production.origin) return null;
  if (candidate.username || candidate.password) {
    throw new Error(`same-origin metadata URL must not contain credentials: ${value}`);
  }
  return resolveBuildPath(candidate.pathname, dist);
}
