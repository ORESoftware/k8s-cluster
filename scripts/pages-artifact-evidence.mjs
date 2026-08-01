import { createHash } from 'node:crypto';
import {
  lstat,
  mkdir,
  readFile,
  readdir,
  rename,
  rm,
  writeFile,
} from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const SHA256_HEX = /^[0-9a-f]{64}$/;
const GIT_SHA_HEX = /^[0-9a-f]{40}$/i;
const SAFE_REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
export const REQUIRED_PAGES_ARTIFACTS = Object.freeze([
  '.well-known/security.txt',
  'deployment.json',
  'index.html',
  'robots.txt',
  'sitemap.xml',
]);

function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}

function compareUtf8(left, right) {
  return Buffer.compare(Buffer.from(left, 'utf8'), Buffer.from(right, 'utf8'));
}

function assertSafeRelativePath(relativePath) {
  if (
    relativePath.length === 0 ||
    relativePath.startsWith('/') ||
    relativePath.includes('\\') ||
    /[\0\r\n]/.test(relativePath)
  ) {
    throw new Error(`unsafe Pages artifact path: ${JSON.stringify(relativePath)}`);
  }
  for (const segment of relativePath.split('/')) {
    if (segment === '' || segment === '.' || segment === '..') {
      throw new Error(`unsafe Pages artifact path segment: ${JSON.stringify(relativePath)}`);
    }
  }
}

function relativeArtifactPath(root, candidate) {
  const relative = path.relative(root, candidate).split(path.sep).join('/');
  if (relative.startsWith('../') || path.isAbsolute(relative)) {
    throw new Error(`artifact entry escaped root: ${candidate}`);
  }
  assertSafeRelativePath(relative);
  return relative;
}

async function walk(root, directory, entries) {
  const names = await readdir(directory);
  names.sort(compareUtf8);

  for (const name of names) {
    const candidate = path.join(directory, name);
    const stat = await lstat(candidate);
    const relativePath = relativeArtifactPath(root, candidate);

    if (stat.isSymbolicLink()) {
      throw new Error(`Pages artifact may not contain symlinks: ${relativePath}`);
    }
    if (stat.isDirectory()) {
      await walk(root, candidate, entries);
      continue;
    }
    if (!stat.isFile()) {
      throw new Error(`Pages artifact may contain only files and directories: ${relativePath}`);
    }

    const content = await readFile(candidate);
    entries.push({
      path: relativePath,
      bytes: content.byteLength,
      sha256: sha256(content),
    });
  }
}

export async function inventoryPagesTree(rootDirectory) {
  const root = path.resolve(rootDirectory);
  const rootStat = await lstat(root);
  if (rootStat.isSymbolicLink() || !rootStat.isDirectory()) {
    throw new Error('Pages artifact root must be a real directory');
  }

  const entries = [];
  await walk(root, root, entries);
  entries.sort((left, right) => compareUtf8(left.path, right.path));

  if (entries.length === 0) {
    throw new Error('Pages artifact tree is empty');
  }
  for (const required of REQUIRED_PAGES_ARTIFACTS) {
    if (!entries.some((entry) => entry.path === required)) {
      throw new Error(`Pages artifact is missing required file: ${required}`);
    }
  }

  const sha256Manifest = entries
    .map((entry) => `${entry.sha256}  ${entry.path}\n`)
    .join('');
  const treeSha256 = sha256(sha256Manifest);
  if (!SHA256_HEX.test(treeSha256)) {
    throw new Error('internal error: invalid tree digest');
  }

  return {
    entries,
    fileCount: entries.length,
    totalBytes: entries.reduce((sum, entry) => sum + entry.bytes, 0),
    sha256Manifest,
    treeSha256,
  };
}

function deploymentMetadata() {
  const repository = (process.env.GITHUB_REPOSITORY || 'sonus-auris/sonus-auris-site.web').trim();
  if (!SAFE_REPOSITORY.test(repository)) {
    throw new Error('GITHUB_REPOSITORY must be an owner/name identifier');
  }

  const rawCommitSha = (process.env.GITHUB_SHA || '').trim();
  if (rawCommitSha !== '' && !GIT_SHA_HEX.test(rawCommitSha)) {
    throw new Error('GITHUB_SHA must be empty or a 40-character hexadecimal commit SHA');
  }

  return {
    repository,
    commitSha: rawCommitSha === '' ? null : rawCommitSha.toLowerCase(),
    refName: (process.env.GITHUB_REF_NAME || '').trim() || null,
  };
}

async function atomicWrite(filePath, content) {
  const temporary = `${filePath}.tmp-${process.pid}`;
  await writeFile(temporary, content, { encoding: 'utf8', mode: 0o600 });
  await rename(temporary, filePath);
}

export async function writePagesEvidence({ rootDirectory, outputDirectory }) {
  const root = path.resolve(rootDirectory);
  const output = path.resolve(outputDirectory);
  if (output === root || output.startsWith(`${root}${path.sep}`)) {
    throw new Error('Pages evidence must be written outside the deployed tree');
  }

  const inventory = await inventoryPagesTree(root);
  const metadata = deploymentMetadata();
  const evidence = {
    schemaVersion: 1,
    repository: metadata.repository,
    commitSha: metadata.commitSha,
    refName: metadata.refName,
    fileCount: inventory.fileCount,
    totalBytes: inventory.totalBytes,
    treeSha256: inventory.treeSha256,
  };

  await mkdir(output, { recursive: true });
  const manifestPath = path.join(output, 'pages-tree.sha256');
  const evidencePath = path.join(output, 'pages-tree.json');
  await Promise.all([
    atomicWrite(manifestPath, inventory.sha256Manifest),
    atomicWrite(evidencePath, `${JSON.stringify(evidence, null, 2)}\n`),
  ]);

  return { evidence, evidencePath, manifestPath };
}

function parseArguments(argv) {
  const parsed = {
    rootDirectory: 'dist',
    outputDirectory: 'artifacts/pages-evidence',
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '--root' || argument === '--out') {
      const value = argv[index + 1];
      if (!value || value.startsWith('--')) {
        throw new Error(`${argument} requires a path`);
      }
      if (argument === '--root') parsed.rootDirectory = value;
      else parsed.outputDirectory = value;
      index += 1;
      continue;
    }
    throw new Error(`unknown argument: ${argument}`);
  }
  return parsed;
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  await rm(options.outputDirectory, { recursive: true, force: true });
  const { evidence } = await writePagesEvidence(options);
  console.log(
    `Sealed ${evidence.fileCount} Pages files (${evidence.totalBytes} bytes) as ${evidence.treeSha256}.`,
  );
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
