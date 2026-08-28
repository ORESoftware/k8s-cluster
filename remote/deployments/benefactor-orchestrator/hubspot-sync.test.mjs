import assert from 'node:assert/strict';
import test from 'node:test';

import {
  assertLiveSyncConfig,
  buildCompanyProperties,
  buildContactProperties,
  buildExactSearchBody,
  buildSyncReport,
  domainFromLead,
  isRoleEmail,
} from './hubspot-sync-lib.mjs';

test('maps bounded standard HubSpot properties without inventing consent', () => {
  const lead = {
    email: 'INFO@Roofing.Example',
    business_name: 'Example Roofing',
    website_url: 'https://www.roofing.example/contact',
    city: 'Austin',
    state: 'TX',
    meta_data: { phones: ['+15125550123'] },
  };
  assert.deepEqual(buildCompanyProperties(lead), {
    name: 'Example Roofing',
    domain: 'roofing.example',
    website: 'https://www.roofing.example',
    city: 'Austin',
    state: 'TX',
  });
  assert.deepEqual(buildContactProperties(lead), {
    email: 'info@roofing.example',
    phone: '+15125550123',
    company: 'Example Roofing',
    website: 'https://www.roofing.example',
    city: 'Austin',
    state: 'TX',
  });
});

test('generic mailbox domains never become company domains', () => {
  assert.equal(domainFromLead({ email: 'hello@gmail.com' }), '');
  assert.equal(domainFromLead({ email: 'hello@gmail.com', source_url: 'https://contractor.example/about' }), 'contractor.example');
});

test('role email classification is conservative', () => {
  assert.equal(isRoleEmail('info@example.com'), true);
  assert.equal(isRoleEmail('sales2@example.com'), true);
  assert.equal(isRoleEmail('alex.mills@example.com'), false);
});

test('search requests use exact property matching', () => {
  assert.deepEqual(buildExactSearchBody('email', 'info@example.com'), {
    filterGroups: [{ filters: [{ propertyName: 'email', operator: 'EQ', value: 'info@example.com' }] }],
    limit: 1,
  });
});

test('live writes fail closed without token, batch, and exact confirmation', () => {
  assert.doesNotThrow(() => assertLiveSyncConfig({ dryRun: true }));
  assert.throws(
    () => assertLiveSyncConfig({ dryRun: false, accessToken: '', batchId: '', writeConfirmation: '' }),
    /HUBSPOT_ACCESS_TOKEN/,
  );
  assert.doesNotThrow(() =>
    assertLiveSyncConfig({
      dryRun: false,
      accessToken: 'configured-at-runtime',
      batchId: 'benefactor-20260805T201500Z-01020304',
      writeConfirmation: 'sync-benefactor-contact-batch',
    }),
  );
});

test('sync reports explicitly state that consent and outreach are untouched', () => {
  const report = buildSyncReport({
    batchId: 'benefactor-20260805T201500Z-01020304',
    dryRun: false,
    candidates: 250,
    synced: 245,
    skipped: 4,
    failed: 1,
    companies: 200,
    contacts: 245,
  });
  assert.equal(report.marketingConsentMutated, false);
  assert.equal(report.outreachDispatched, false);
  assert.equal(report.batchDigest.length, 64);
});
