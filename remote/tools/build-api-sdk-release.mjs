#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { chmod, copyFile, lstat, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import {
  canonicalJson,
  readRepoFile,
  repoRoot,
  sha256,
  stableValue,
} from './api-sdk-common.mjs';

const SDK_ROOT_REPO_PATH = 'remote/api-sdks';
const SDK_ROOT = resolve(repoRoot, SDK_ROOT_REPO_PATH);
const LOCK_REPO_PATH = `${SDK_ROOT_REPO_PATH}/sdk-lock.json`;
const SUPPORTED_LANGUAGES = ['dart', 'gleam', 'rust', 'typescript'];
const SUPPORTED_SCOPES = ['public', 'internal'];
const ARCHIVE_PREFIX = 'oresoftware-k8s-api-sdks';
const SEMVER_PATTERN =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

function stablePretty(value) {
  return `${JSON.stringify(stableValue(value), null, 2)}\n`;
}

function usage() {
  return `Usage: node remote/tools/build-api-sdk-release.mjs --output-dir <path> [options]

Options:
  --output-dir <path>       Required output directory; it is replaced before each build.
  --source-revision <sha>   Forty-character Git commit SHA (defaults to HEAD).
  --release-version <semver> Require all generated packages to have this version.
  --help                    Show this message.
`;
}

function parseArgs(argv) {
  const result = {
    outputDir: null,
    sourceRevision: null,
    releaseVersion: null,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '--help') {
      process.stdout.write(usage());
      process.exit(0);
    }
    if (!['--output-dir', '--source-revision', '--release-version'].includes(argument)) {
      throw new Error(`unknown argument: ${argument}\n${usage()}`);
    }
    const value = argv[index + 1];
    if (!value || value.startsWith('--')) {
      throw new Error(`${argument} requires a value`);
    }
    index += 1;
    if (argument === '--output-dir') result.outputDir = value;
    if (argument === '--source-revision') result.sourceRevision = value;
    if (argument === '--release-version') result.releaseVersion = value;
  }
  if (!result.outputDir) {
    throw new Error(`--output-dir is required\n${usage()}`);
  }
  return result;
}

function git(args, options = {}) {
  return execFileSync('git', args, {
    cwd: repoRoot,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    ...options,
  });
}

function assertSafeOutputDirectory(rawOutputDirectory) {
  const outputDirectory = resolve(rawOutputDirectory);
  const repoPrefix = `${repoRoot}${sep}`;
  const sdkPrefix = `${SDK_ROOT}${sep}`;
  if (
    outputDirectory === repoRoot ||
    repoRoot.startsWith(`${outputDirectory}${sep}`) ||
    outputDirectory === SDK_ROOT ||
    outputDirectory.startsWith(sdkPrefix) ||
    outputDirectory.startsWith(repoPrefix)
  ) {
    throw new Error(`release output must be outside the repository: ${outputDirectory}`);
  }
  return outputDirectory;
}

function assertSourceRevision(rawRevision) {
  const revision = rawRevision ?? git(['rev-parse', 'HEAD']).trim();
  if (!/^[0-9a-f]{40}$/.test(revision)) {
    throw new Error(`source revision must be a forty-character lowercase Git SHA: ${revision}`);
  }
  return revision;
}

function assertCleanSdkTree() {
  const status = git([
    'status',
    '--porcelain=v1',
    '--untracked-files=all',
    '--',
    SDK_ROOT_REPO_PATH,
  ]).trim();
  if (status) {
    throw new Error(`generated SDK tree is not clean:\n${status}`);
  }
}

function trackedSdkFiles() {
  const raw = execFileSync(
    'git',
    ['ls-files', '-z', '--', SDK_ROOT_REPO_PATH],
    { cwd: repoRoot, encoding: 'buffer', stdio: ['ignore', 'pipe', 'pipe'] },
  );
  const files = raw
    .toString('utf8')
    .split('\0')
    .filter(Boolean)
    .sort();
  if (files.length === 0) {
    throw new Error('no tracked fleet SDK files were found');
  }
  return files;
}

function assertRepoPath(path, expectedPrefix = null) {
  if (
    typeof path !== 'string' ||
    path.length === 0 ||
    path.includes('\0') ||
    path.startsWith('/') ||
    path.split('/').some((segment) => segment === '' || segment === '.' || segment === '..')
  ) {
    throw new Error(`unsafe repository path: ${String(path)}`);
  }
  if (expectedPrefix && path !== expectedPrefix && !path.startsWith(`${expectedPrefix}/`)) {
    throw new Error(`repository path ${path} is outside ${expectedPrefix}`);
  }
  const absolute = resolve(repoRoot, path);
  if (absolute !== repoRoot && !absolute.startsWith(`${repoRoot}${sep}`)) {
    throw new Error(`repository path escapes the checkout: ${path}`);
  }
  return absolute;
}

async function readRepoBytes(path) {
  return readFile(assertRepoPath(path));
}

async function fileDigest(path) {
  return sha256(await readRepoBytes(path));
}

async function readJson(path) {
  const raw = await readRepoBytes(path);
  return { raw, value: JSON.parse(raw.toString('utf8')) };
}

function assertEqual(actual, expected, message) {
  if (actual !== expected) {
    throw new Error(
      `${message}: expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`,
    );
  }
}

function assertCanonicalEqual(actual, expected, message) {
  assertEqual(canonicalJson(actual), canonicalJson(expected), message);
}

function packagePairKey(language, scope) {
  return `${language}:${scope}`;
}

function expectedPackagePairs() {
  return new Set(
    SUPPORTED_LANGUAGES.flatMap((language) =>
      SUPPORTED_SCOPES.map((scope) => packagePairKey(language, scope)),
    ),
  );
}

async function readPackageVersion(packageEntry) {
  const root = packageEntry.path;
  if (packageEntry.language === 'typescript') {
    const { value } = await readJson(`${root}/package.json`);
    if (typeof value.version !== 'string') {
      throw new Error(`${root}/package.json has no version`);
    }
    return value.version;
  }

  const fileName =
    packageEntry.language === 'dart'
      ? 'pubspec.yaml'
      : packageEntry.language === 'rust'
        ? 'Cargo.toml'
        : 'gleam.toml';
  const source = await readRepoFile(`${root}/${fileName}`);
  const pattern =
    packageEntry.language === 'dart'
      ? /^version:\s*([^\s#]+)\s*$/m
      : /^version\s*=\s*"([^"]+)"\s*$/m;
  const match = source.match(pattern);
  if (!match?.[1]) {
    throw new Error(`${root}/${fileName} has no parseable version`);
  }
  return match[1];
}

async function assertRegularTrackedFiles(files) {
  for (const path of files) {
    const metadata = await lstat(assertRepoPath(path, SDK_ROOT_REPO_PATH));
    if (!metadata.isFile()) {
      throw new Error(`fleet SDK release refuses non-regular tracked entry: ${path}`);
    }
  }
}

async function verifySdkLock(trackedFiles) {
  const tracked = new Set(trackedFiles);
  if (!tracked.has(LOCK_REPO_PATH)) {
    throw new Error(`${LOCK_REPO_PATH} is not tracked`);
  }

  const { raw: lockRaw, value: lock } = await readJson(LOCK_REPO_PATH);
  assertEqual(lock.schemaVersion, 1, 'SDK lock schemaVersion drifted');
  assertEqual(
    lock.generatedBy,
    'remote/tools/generate-api-sdks.mjs',
    'SDK lock generator path drifted',
  );
  assertEqual(
    await fileDigest(lock.generatedBy),
    lock.generatorSha256,
    'SDK generator digest drifted',
  );
  assertEqual(
    await fileDigest(lock.indexPath),
    lock.indexSha256,
    'API-docs index digest drifted',
  );

  for (const scope of SUPPORTED_SCOPES) {
    const scopeLock = lock.scopes?.[scope];
    if (!scopeLock || typeof scopeLock !== 'object') {
      throw new Error(`SDK lock is missing ${scope} scope`);
    }
    assertRepoPath(scopeLock.catalogPath, SDK_ROOT_REPO_PATH);
    const { raw: catalogRaw, value: catalog } = await readJson(scopeLock.catalogPath);
    assertEqual(
      sha256(catalogRaw),
      scopeLock.catalogFileSha256,
      `${scope} catalog file digest drifted`,
    );
    assertEqual(
      catalog.catalogSha256,
      scopeLock.catalogSha256,
      `${scope} catalog identity drifted`,
    );
    assertEqual(catalog.scope, scope, `${scope} catalog scope drifted`);
    assertEqual(
      catalog.serviceCount,
      scopeLock.serviceCount,
      `${scope} service count drifted`,
    );
    assertEqual(
      catalog.operationCount,
      scopeLock.operationCount,
      `${scope} operation count drifted`,
    );
    assertCanonicalEqual(
      catalog.skippedServices ?? [],
      scopeLock.skippedServices ?? [],
      `${scope} skipped-service inventory drifted`,
    );
  }

  if (!Array.isArray(lock.packages)) {
    throw new Error('SDK lock packages must be an array');
  }
  const requiredPairs = expectedPackagePairs();
  const seenPairs = new Set();
  const versions = new Set();
  const packageFilesByScope = new Map(SUPPORTED_SCOPES.map((scope) => [scope, new Set()]));

  for (const packageEntry of lock.packages) {
    if (!SUPPORTED_LANGUAGES.includes(packageEntry.language)) {
      throw new Error(`unsupported SDK language in lock: ${packageEntry.language}`);
    }
    if (!SUPPORTED_SCOPES.includes(packageEntry.scope)) {
      throw new Error(`unsupported SDK scope in lock: ${packageEntry.scope}`);
    }
    const pair = packagePairKey(packageEntry.language, packageEntry.scope);
    if (!requiredPairs.has(pair) || seenPairs.has(pair)) {
      throw new Error(`duplicate or unexpected SDK package pair: ${pair}`);
    }
    seenPairs.add(pair);

    const expectedRoot = `${SDK_ROOT_REPO_PATH}/${packageEntry.language}/${packageEntry.scope}`;
    assertEqual(packageEntry.path, expectedRoot, `${pair} package root drifted`);
    assertEqual(
      packageEntry.manifestPath,
      `${expectedRoot}/sdk-manifest.json`,
      `${pair} manifest path drifted`,
    );
    const { raw: manifestRaw, value: manifest } = await readJson(packageEntry.manifestPath);
    assertEqual(
      sha256(manifestRaw),
      packageEntry.manifestSha256,
      `${pair} manifest digest drifted`,
    );
    assertEqual(manifest.schemaVersion, 1, `${pair} manifest schemaVersion drifted`);
    assertEqual(manifest.language, packageEntry.language, `${pair} manifest language drifted`);
    assertEqual(manifest.scope, packageEntry.scope, `${pair} manifest scope drifted`);
    assertEqual(manifest.packageName, packageEntry.packageName, `${pair} package name drifted`);
    assertEqual(manifest.generatedBy, lock.generatedBy, `${pair} manifest generator drifted`);
    assertEqual(
      manifest.catalogPath,
      lock.scopes[packageEntry.scope].catalogPath,
      `${pair} manifest catalog path drifted`,
    );
    assertEqual(
      manifest.catalogFileSha256,
      lock.scopes[packageEntry.scope].catalogFileSha256,
      `${pair} manifest catalog file digest drifted`,
    );
    assertEqual(
      manifest.serviceCount,
      lock.scopes[packageEntry.scope].serviceCount,
      `${pair} service count drifted`,
    );
    assertCanonicalEqual(
      manifest.skippedServices ?? [],
      lock.scopes[packageEntry.scope].skippedServices ?? [],
      `${pair} skipped-service inventory drifted`,
    );
    assertEqual(
      manifest.catalogSha256,
      packageEntry.catalogSha256,
      `${pair} catalog digest drifted`,
    );
    assertEqual(
      manifest.operationCount,
      packageEntry.operationCount,
      `${pair} operation count drifted`,
    );

    const expectedFiles = new Set([packageEntry.manifestPath]);
    if (!Array.isArray(manifest.generatedFiles) || manifest.generatedFiles.length === 0) {
      throw new Error(`${pair} manifest has no generated files`);
    }
    for (const generatedFile of manifest.generatedFiles) {
      const relativePath = generatedFile.path;
      assertRepoPath(relativePath);
      const repoPath = `${expectedRoot}/${relativePath}`;
      assertRepoPath(repoPath, expectedRoot);
      if (expectedFiles.has(repoPath)) {
        throw new Error(`${pair} manifest repeats generated file ${relativePath}`);
      }
      expectedFiles.add(repoPath);
      assertEqual(
        await fileDigest(repoPath),
        generatedFile.sha256,
        `${pair} generated file digest drifted for ${relativePath}`,
      );
    }

    const trackedPackageFiles = trackedFiles.filter(
      (path) => path === expectedRoot || path.startsWith(`${expectedRoot}/`),
    );
    assertCanonicalEqual(
      trackedPackageFiles,
      [...expectedFiles].sort(),
      `${pair} tracked package files differ from its manifest`,
    );
    for (const path of expectedFiles) {
      packageFilesByScope.get(packageEntry.scope).add(path);
    }

    const version = await readPackageVersion(packageEntry);
    versions.add(version);
    packageEntry.version = version;
  }

  assertCanonicalEqual(
    [...seenPairs].sort(),
    [...requiredPairs].sort(),
    'SDK package matrix is incomplete',
  );
  if (versions.size !== 1) {
    throw new Error(
      `generated SDK package versions diverge: ${[...versions].sort().join(', ')}`,
    );
  }
  const version = [...versions][0];
  if (!SEMVER_PATTERN.test(version)) {
    throw new Error(`generated SDK package version is not semantic: ${version}`);
  }

  const accountedFiles = new Set([LOCK_REPO_PATH, `${SDK_ROOT_REPO_PATH}/README.md`]);
  for (const scope of SUPPORTED_SCOPES) {
    accountedFiles.add(lock.scopes[scope].catalogPath);
    for (const path of packageFilesByScope.get(scope)) accountedFiles.add(path);
  }
  assertCanonicalEqual(
    trackedFiles,
    [...accountedFiles].sort(),
    'tracked SDK tree contains files outside the lock graph',
  );

  return {
    lock,
    lockRaw,
    version,
    packageFilesByScope,
  };
}

async function fileRecord(repoPath) {
  const bytes = await readRepoBytes(repoPath);
  return {
    path: relative(SDK_ROOT, assertRepoPath(repoPath)).split(sep).join('/'),
    size: bytes.length,
    sha256: sha256(bytes),
  };
}

async function copyScopeFiles(repoPaths, stagingDirectory) {
  for (const repoPath of repoPaths) {
    const source = assertRepoPath(repoPath, SDK_ROOT_REPO_PATH);
    const relativePath = relative(SDK_ROOT, source);
    if (relativePath.startsWith('..') || resolve(SDK_ROOT, relativePath) !== source) {
      throw new Error(`unable to derive SDK-relative release path for ${repoPath}`);
    }
    const target = resolve(stagingDirectory, relativePath);
    if (!target.startsWith(`${stagingDirectory}${sep}`)) {
      throw new Error(`release staging path escapes its root: ${repoPath}`);
    }
    await mkdir(dirname(target), { recursive: true, mode: 0o755 });
    await copyFile(source, target);
    await chmod(target, 0o644);
  }
}

function assertGnuTar() {
  const version = execFileSync('tar', ['--version'], { encoding: 'utf8' });
  if (!version.startsWith('tar (GNU tar)')) {
    throw new Error('deterministic SDK release archives require GNU tar');
  }
}

function buildTar(stagingDirectory, archivePath) {
  execFileSync(
    'tar',
    [
      '--sort=name',
      '--mtime=@0',
      '--owner=0',
      '--group=0',
      '--numeric-owner',
      '--mode=u+rwX,go+rX,go-w',
      '--format=ustar',
      '-cf',
      archivePath,
      '-C',
      stagingDirectory,
      '.',
    ],
    { stdio: ['ignore', 'pipe', 'pipe'] },
  );
}

async function buildScopeBundle({
  scope,
  outputDirectory,
  sourceRevision,
  version,
  lock,
  lockRaw,
  packageFiles,
}) {
  const scopeLock = lock.scopes[scope];
  const packages = lock.packages
    .filter((entry) => entry.scope === scope)
    .sort((left, right) => left.language.localeCompare(right.language));
  const repoPaths = [scopeLock.catalogPath, ...packageFiles].sort();
  const files = [];
  for (const path of repoPaths) files.push(await fileRecord(path));

  const core = {
    schemaVersion: 1,
    mediaType: 'application/vnd.oresoftware.k8s-api-sdk-release.v1+json',
    scope,
    version,
    sourceRepository: 'https://github.com/ORESoftware/k8s-cluster',
    sourceRevision,
    sourceTree: SDK_ROOT_REPO_PATH,
    sdkLock: {
      path: LOCK_REPO_PATH,
      sha256: sha256(lockRaw),
    },
    generator: {
      path: lock.generatedBy,
      sha256: lock.generatorSha256,
    },
    contractIndex: {
      path: lock.indexPath,
      sha256: lock.indexSha256,
    },
    catalog: {
      path: scopeLock.catalogPath,
      catalogSha256: scopeLock.catalogSha256,
      fileSha256: scopeLock.catalogFileSha256,
      serviceCount: scopeLock.serviceCount,
      operationCount: scopeLock.operationCount,
      skippedServices: scopeLock.skippedServices,
    },
    packages: packages.map((entry) => ({
      language: entry.language,
      scope: entry.scope,
      packageName: entry.packageName,
      version: entry.version,
      path: entry.path,
      manifestPath: entry.manifestPath,
      manifestSha256: entry.manifestSha256,
      catalogSha256: entry.catalogSha256,
      operationCount: entry.operationCount,
    })),
    files,
  };
  const manifest = {
    ...core,
    releaseIdentitySha256: sha256(canonicalJson(core)),
  };
  const manifestContent = stablePretty(manifest);
  const manifestName = `${scope}-release-manifest.json`;
  const stagingDirectory = resolve(outputDirectory, `.staging-${scope}`);
  const archiveName = `${ARCHIVE_PREFIX}-${scope}.tar`;
  const archivePath = resolve(outputDirectory, archiveName);

  await rm(stagingDirectory, { recursive: true, force: true });
  await mkdir(stagingDirectory, { recursive: true, mode: 0o755 });
  await copyScopeFiles(repoPaths, stagingDirectory);
  await writeFile(resolve(stagingDirectory, 'release-manifest.json'), manifestContent, {
    mode: 0o644,
  });
  await writeFile(resolve(outputDirectory, manifestName), manifestContent, { mode: 0o644 });
  buildTar(stagingDirectory, archivePath);
  await rm(stagingDirectory, { recursive: true, force: true });

  const archiveBytes = await readFile(archivePath);
  return {
    scope,
    archive: archiveName,
    archiveSha256: sha256(archiveBytes),
    archiveSize: archiveBytes.length,
    manifest: manifestName,
    manifestSha256: sha256(manifestContent),
    releaseIdentitySha256: manifest.releaseIdentitySha256,
  };
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const outputDirectory = assertSafeOutputDirectory(options.outputDir);
  const sourceRevision = assertSourceRevision(options.sourceRevision);
  assertCleanSdkTree();
  const trackedFiles = trackedSdkFiles();
  await assertRegularTrackedFiles(trackedFiles);
  const verified = await verifySdkLock(trackedFiles);
  if (options.releaseVersion && options.releaseVersion !== verified.version) {
    throw new Error(
      `requested release version ${options.releaseVersion} does not match generated package version ${verified.version}`,
    );
  }

  assertGnuTar();
  await rm(outputDirectory, { recursive: true, force: true });
  await mkdir(outputDirectory, { recursive: true, mode: 0o755 });

  const bundles = [];
  for (const scope of SUPPORTED_SCOPES) {
    bundles.push(
      await buildScopeBundle({
        scope,
        outputDirectory,
        sourceRevision,
        version: verified.version,
        lock: verified.lock,
        lockRaw: verified.lockRaw,
        packageFiles: [...verified.packageFilesByScope.get(scope)].sort(),
      }),
    );
  }

  const indexCore = {
    schemaVersion: 1,
    mediaType: 'application/vnd.oresoftware.k8s-api-sdk-release-index.v1+json',
    version: verified.version,
    sourceRepository: 'https://github.com/ORESoftware/k8s-cluster',
    sourceRevision,
    sdkLock: {
      path: LOCK_REPO_PATH,
      sha256: sha256(verified.lockRaw),
    },
    bundles,
  };
  const releaseIndex = {
    ...indexCore,
    releaseIdentitySha256: sha256(canonicalJson(indexCore)),
  };
  const releaseIndexContent = stablePretty(releaseIndex);
  await writeFile(resolve(outputDirectory, 'release-index.json'), releaseIndexContent, {
    mode: 0o644,
  });

  const checksumTargets = [
    ...bundles.map((bundle) => bundle.archive),
    ...bundles.map((bundle) => bundle.manifest),
    'release-index.json',
  ].sort();
  const checksumLines = [];
  for (const name of checksumTargets) {
    checksumLines.push(`${sha256(await readFile(resolve(outputDirectory, name)))}  ${name}`);
  }
  await writeFile(
    resolve(outputDirectory, 'SHA256SUMS'),
    `${checksumLines.join('\n')}\n`,
    { mode: 0o644 },
  );

  process.stdout.write(
    stablePretty({
      outputDirectory,
      sourceRevision,
      version: verified.version,
      releaseIdentitySha256: releaseIndex.releaseIdentitySha256,
      bundles: bundles.map((bundle) => ({
        scope: bundle.scope,
        archive: bundle.archive,
        sha256: bundle.archiveSha256,
        size: bundle.archiveSize,
      })),
    }),
  );
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
