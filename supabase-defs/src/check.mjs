import { access, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const catalogPath = resolve(root, 'catalog.json');
const catalog = JSON.parse(await readFile(catalogPath, 'utf8'));

const idPattern = /^[a-z]{20}$/;
const schemaPattern = /^[a-z_][a-z0-9_]*$/;
const seenOrganizations = new Set();
const seenProjects = new Set();

if (catalog.version !== 1 || !Array.isArray(catalog.organizations)) {
  throw new Error('catalog.json must declare version 1 and organizations[]');
}

for (const organization of catalog.organizations) {
  if (!idPattern.test(organization.id) || seenOrganizations.has(organization.id)) {
    throw new Error(`invalid or duplicate organization id: ${organization.id}`);
  }
  seenOrganizations.add(organization.id);
  if (typeof organization.name !== 'string' || organization.name.trim() === '') {
    throw new Error(`organization ${organization.id} must have a name`);
  }
  if (!Array.isArray(organization.projects)) {
    throw new Error(`organization ${organization.id} must have projects[]`);
  }

  for (const project of organization.projects) {
    if (!idPattern.test(project.ref) || seenProjects.has(project.ref)) {
      throw new Error(`invalid or duplicate project ref: ${project.ref}`);
    }
    seenProjects.add(project.ref);
    if (typeof project.name !== 'string' || project.name.trim() === '') {
      throw new Error(`project ${project.ref} must have a name`);
    }
    if (!Array.isArray(project.schemas)) {
      throw new Error(`project ${project.ref} must have schemas[]`);
    }

    for (const schema of project.schemas) {
      if (!schemaPattern.test(schema.name)) {
        throw new Error(`invalid schema name for ${project.ref}: ${schema.name}`);
      }
      const definition = resolve(root, schema.definition);
      if (!definition.startsWith(`${root}/`)) {
        throw new Error(`schema definition escapes supabase-defs: ${schema.definition}`);
      }
      await access(definition);
      const sql = await readFile(definition, 'utf8');
      if (!sql.includes(`Supabase project: ${project.ref}`)) {
        throw new Error(`${schema.definition} is not labelled for ${project.ref}`);
      }
    }
  }
}

console.log(
  `validated ${seenOrganizations.size} Supabase organizations and ${seenProjects.size} projects`,
);
