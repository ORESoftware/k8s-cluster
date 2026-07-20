#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const releasePath = path.join(root, "gitops", "release.json");
const dataPlanePath = path.join(root, "gitops", "data-plane");
const clusters = ["civo", "hetzner", "vultr"];
const components = [
  {
    repo: "fiducia-node.rs",
    image: "ghcr.io/fiducia-cloud/fiducia-node",
  },
  {
    repo: "fiducia-node-sidecar.rs",
    image: "ghcr.io/fiducia-cloud/fiducia-node-sidecar",
  },
  {
    repo: "fiducia-brain.rs",
    image: "ghcr.io/fiducia-cloud/fiducia-brain",
  },
  {
    repo: "fiducia-load-balance.rs",
    image: "ghcr.io/fiducia-cloud/fiducia-load-balance",
  },
];

const shaPattern = /^[0-9a-f]{40}$/;
const digestPattern = /^sha256:[0-9a-f]{64}$/;

function fail(message) {
  throw new Error(message);
}

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
    ...options,
  }).trim();
}

function sha256(text) {
  return `sha256:${createHash("sha256").update(text).digest("hex")}`;
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function gitlink(repo) {
  const commit = run("git", ["rev-parse", `HEAD:apps/${repo}`]);
  if (!shaPattern.test(commit)) {
    fail(`apps/${repo} is not an exact gitlink commit: ${commit}`);
  }
  return commit;
}

function requireCheckedOutGitlink(repo, commit) {
  const checkout = path.join(root, "apps", repo);
  if (!existsSync(path.join(checkout, ".git"))) {
    fail(`apps/${repo} is not initialized`);
  }
  const actual = run("git", ["-C", checkout, "rev-parse", "HEAD"]);
  if (actual !== commit) {
    fail(`apps/${repo} checkout ${actual} differs from gitlink ${commit}`);
  }
}

function parsePromoteArguments(args) {
  let sourcePin;
  const digests = new Map();
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--source") {
      sourcePin = args[++index];
    } else if (argument === "--digest") {
      const value = args[++index] || "";
      const separator = value.indexOf("=");
      if (separator < 1) fail(`invalid --digest value: ${value}`);
      digests.set(value.slice(0, separator), value.slice(separator + 1));
    } else {
      fail(`unknown promote argument: ${argument}`);
    }
  }

  if (!shaPattern.test(sourcePin || "")) {
    fail("promote requires --source with the reviewed 40-character monorepo commit");
  }
  for (const { repo } of components) {
    const digest = digests.get(repo);
    if (!digestPattern.test(digest || "")) {
      fail(`promote requires a sha256 registry digest for ${repo}`);
    }
  }
  if (digests.size !== components.length) {
    fail("promote received an unknown or duplicate component digest");
  }
  return { sourcePin, digests };
}

function validateRenderedManifest(cluster, manifest, release) {
  if (/^kind:\s*Secret\s*$/m.test(manifest)) {
    fail(`${cluster} manifest contains a Secret; secrets must stay out of Git`);
  }
  if (!manifest.includes(`FIDUCIA_CLUSTER: ${cluster}`)) {
    fail(`${cluster} manifest does not carry its cluster identity`);
  }

  for (const { repo, image } of components) {
    const expected = `${image}@${release.components[repo].digest}`;
    const imageLine = new RegExp(`^image:\\s*${escapeRegex(image)}(?=[:@])`);
    const imageLines = manifest
      .split(/\r?\n/)
      .filter((line) => imageLine.test(line.trim()));
    if (imageLines.length === 0) {
      fail(`${cluster} manifest does not contain ${image}`);
    }
    for (const line of imageLines) {
      if (line.trim() !== `image: ${expected}`) {
        fail(`${cluster} has a non-release image reference for ${image}: ${line.trim()}`);
      }
    }
  }
}

function promote(args) {
  const { sourcePin, digests } = parsePromoteArguments(args);
  const infraCommit = gitlink("fiducia-infra");
  requireCheckedOutGitlink("fiducia-infra", infraCommit);
  run("node", ["tools/render.mjs", "--check"], {
    cwd: path.join(root, "apps", "fiducia-infra"),
  });

  const release = {
    schema_version: 1,
    source_pin: sourcePin,
    infra_commit: infraCommit,
    clusters,
    components: {},
    manifests: {},
  };

  for (const { repo, image } of components) {
    const commit = gitlink(repo);
    requireCheckedOutGitlink(repo, commit);
    release.components[repo] = {
      commit,
      image,
      digest: digests.get(repo),
    };
  }

  for (const cluster of clusters) {
    let manifest = run("kubectl", [
      "kustomize",
      path.join(root, "apps", "fiducia-infra", "clusters", cluster),
    ]);
    manifest += "\n";

    for (const { repo, image } of components) {
      const matcher = new RegExp(
        `(image:\\s*)${escapeRegex(image)}(?::[^\\s#]+|@sha256:[0-9a-f]{64})`,
        "g",
      );
      const before = manifest;
      manifest = manifest.replace(
        matcher,
        `$1${image}@${release.components[repo].digest}`,
      );
      if (manifest === before) {
        fail(`${cluster} overlay does not reference ${image}`);
      }
    }

    const banner = [
      "# GENERATED by tools/gitops-release.mjs; do not edit.",
      `# reviewed-monorepo-pin: ${sourcePin}`,
      `# fiducia-infra: ${infraCommit}`,
      "",
    ].join("\n");
    manifest = banner + manifest;
    validateRenderedManifest(cluster, manifest, release);
    const manifestPath = path.join(dataPlanePath, cluster, "manifests.yaml");
    writeFileSync(manifestPath, manifest);
    release.manifests[cluster] = sha256(manifest);
  }

  writeFileSync(releasePath, `${JSON.stringify(release, null, 2)}\n`);
  check();
}

function check() {
  const release = JSON.parse(readFileSync(releasePath, "utf8"));
  assert.equal(release.schema_version, 1, "unsupported gitops release schema");
  assert.match(release.source_pin, shaPattern);
  assert.match(release.infra_commit, shaPattern);
  assert.deepEqual(release.clusters, clusters);
  assert.deepEqual(Object.keys(release.components), components.map(({ repo }) => repo));
  assert.deepEqual(Object.keys(release.manifests), clusters);

  const actualClusterDirectories = readdirSync(dataPlanePath, {
    withFileTypes: true,
  })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort();
  assert.deepEqual(actualClusterDirectories, clusters);

  for (const { repo, image } of components) {
    const component = release.components[repo];
    assert.equal(component.image, image);
    assert.match(component.commit, shaPattern);
    assert.match(component.digest, digestPattern);
  }

  for (const cluster of clusters) {
    const clusterPath = path.join(dataPlanePath, cluster);
    const manifest = readFileSync(path.join(clusterPath, "manifests.yaml"), "utf8");
    assert.equal(
      sha256(manifest),
      release.manifests[cluster],
      `${cluster} manifest differs from release.json`,
    );
    assert.match(manifest, new RegExp(`# reviewed-monorepo-pin: ${release.source_pin}`));
    assert.match(manifest, new RegExp(`# fiducia-infra: ${release.infra_commit}`));
    validateRenderedManifest(cluster, manifest, release);
    run("kubectl", ["kustomize", clusterPath]);
  }

  run("kubectl", ["kustomize", path.join(root, "gitops", "argocd")]);
}

function usage() {
  console.error(
    "usage: node tools/gitops-release.mjs check | promote --source <sha> --digest <repo=sha256:...> ...",
  );
  process.exitCode = 2;
}

const [command, ...args] = process.argv.slice(2);
try {
  if (command === "check") check();
  else if (command === "promote") promote(args);
  else usage();
} catch (error) {
  console.error(`gitops-release: ${error.message}`);
  process.exitCode = 1;
}
