import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import YAML from 'yaml';

const root = join(import.meta.dirname, '../../..');
const manifestPath = join(
  root,
  'remote/argocd/dd-next-runtime/dd-build-server-gha-continuity.patch.yaml',
);
const manifest = YAML.parse(readFileSync(manifestPath, 'utf8'));
const env = Object.fromEntries(
  manifest.spec.template.spec.containers
    .find((container: { name: string }) => container.name === 'build-server')
    .env.map((entry: { name: string; value: string }) => [entry.name, entry.value]),
);

test('DES browser repositories receive exact least-privilege profiles', () => {
  const rules = JSON.parse(env.BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON);
  const byRepository = new Map(
    rules.map((rule: { repository: string; profiles: string[] }) => [
      rule.repository,
      rule.profiles,
    ]),
  );

  assert.deepEqual(
    byRepository.get(
      'https://github.com/discrete-event-systems-test/des-web-playwright-e2e.git',
    ),
    ['playwright'],
  );
  assert.deepEqual(
    byRepository.get(
      'https://github.com/discrete-event-systems-test/des-web-puppeteer-e2e.git',
    ),
    ['puppeteer'],
  );
  assert.equal(
    rules.some((rule: { repository: string }) =>
      rule.repository.endsWith('/discrete-event-systems-test/'),
    ),
    false,
  );
});

test('GHA planning remains bounded and live execution stays disabled', () => {
  assert.equal(env.BUILD_SERVER_GHA_WORKFLOW_EXECUTION_ENABLED, 'false');
  assert.equal(env.BUILD_SERVER_GHA_MAX_YAML_BYTES, '262144');
  assert.equal(env.BUILD_SERVER_GHA_MAX_JOBS, '64');
  assert.equal(env.BUILD_SERVER_GHA_MAX_STEPS_PER_JOB, '128');
  assert.match(env.BUILD_SERVER_ALLOWED_PROFILES, /(?:^|,)playwright(?:,|$)/);
  assert.match(env.BUILD_SERVER_ALLOWED_PROFILES, /(?:^|,)puppeteer(?:,|$)/);
});
