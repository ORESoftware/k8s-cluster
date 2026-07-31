import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const catalog = JSON.parse(await readFile(resolve(root, 'catalog.json'), 'utf8'));

test('the Sonus Auris schema is owned by its observed organization and project', () => {
  const organization = catalog.organizations.find(
    (entry) => entry.id === 'ngixgitdtrbwnaimwsbb',
  );
  assert.equal(organization?.name, 'sonus-auris');

  const project = organization.projects.find(
    (entry) => entry.ref === 'mckxblyvfzyoxpwvrnjm',
  );
  assert.equal(project?.schemas[0]?.name, 'public');
  assert.match(project?.schemas[0]?.definition, /mckxblyvfzyoxpwvrnjm/);
});

test('project refs are globally unique', () => {
  const refs = catalog.organizations.flatMap((organization) =>
    organization.projects.map((project) => project.ref),
  );
  assert.equal(new Set(refs).size, refs.length);
});
