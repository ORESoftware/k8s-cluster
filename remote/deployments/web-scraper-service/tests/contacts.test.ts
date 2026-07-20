import assert from 'node:assert/strict';
import { test } from 'node:test';

import { extractContacts, normalizeEmail, normalizePhoneNumber } from '../src/contacts.js';

const both = {
  includePhones: true,
  includeEmails: true,
  maxPhones: 50,
  maxEmails: 50,
  defaultRegion: 'US',
};

test('tel: and mailto: hrefs are extracted and normalized to E.164', () => {
  const html = `
    <a href="tel:+1 (415) 555-0134">Call us</a>
    <a href="mailto:Sales@Acme-Corp.com">Email sales</a>
  `;
  const { phones, emails } = extractContacts({ html, text: '', ...both });

  assert.equal(phones.length, 1);
  assert.equal(phones[0]?.e164, '+14155550134');
  assert.equal(phones[0]?.national, '(415) 555-0134');
  assert.deepEqual(phones[0]?.sources, ['tel-href']);
  assert.equal(phones[0]?.confidence, 1);

  assert.equal(emails.length, 1);
  assert.equal(emails[0]?.address, 'sales@acme-corp.com');
});

test('local numbers in visible text normalize using the default region', () => {
  const text = 'Main office: (212) 555-0147. Support: 415.555.0188';
  const { phones } = extractContacts({ html: '', text, ...both });

  const e164 = phones.map((p) => p.e164).sort();
  assert.deepEqual(e164, ['+12125550147', '+14155550188']);
  assert.equal(phones.every((p) => p.sources.includes('text')), true);
});

test('a number found in both a tel: href and text dedupes to one entry at high confidence', () => {
  const html = '<a href="tel:14155550134">(415) 555-0134</a>';
  const { phones } = extractContacts({
    html,
    text: 'Call (415) 555-0134 today',
    ...both,
  });

  assert.equal(phones.length, 1);
  assert.equal(phones[0]?.e164, '+14155550134');
  assert.deepEqual(phones[0]?.sources, ['tel-href', 'text']);
  assert.equal(phones[0]?.confidence, 1);
});

test('schema.org JSON-LD telephone and email are extracted', () => {
  const html = `
    <script type="application/ld+json">
    {
      "@context": "https://schema.org",
      "@type": "LocalBusiness",
      "name": "Acme",
      "email": "hello@acme.test",
      "contactPoint": { "@type": "ContactPoint", "telephone": "+44 20 7946 0958" }
    }
    </script>
  `;
  const { phones, emails } = extractContacts({ html, text: '', ...both });

  assert.equal(phones[0]?.e164, '+442079460958');
  assert.deepEqual(phones[0]?.sources, ['structured-data']);
  assert.equal(emails[0]?.address, 'hello@acme.test');
});

test('extensions are captured from text and from embedded suffixes', () => {
  const fromText = extractContacts({
    html: '',
    text: 'Reception (212) 555-0147 ext. 4021',
    ...both,
  });
  assert.equal(fromText.phones[0]?.extension, '4021');

  const fromHref = extractContacts({
    html: '<a href="tel:+12125550147,,4021">call</a>',
    text: '',
    ...both,
  });
  assert.equal(fromHref.phones[0]?.e164, '+12125550147');
  assert.equal(fromHref.phones[0]?.extension, '4021');
});

test('non-phone digit runs are rejected', () => {
  const text = [
    'Order #100000123456789 placed 2024-01-15 for $1,299.00',
    'SKU 12-3456-78 and ZIP 94107',
    'Placeholder 555-555-5555 and 123-456-7890',
    'Copyright 2019 - 2024',
  ].join(' ');
  const { phones } = extractContacts({ html: '', text, ...both });

  assert.deepEqual(phones, []);
});

test('asset and placeholder addresses are not treated as business emails', () => {
  const text = 'logo@2x.png hero@3x.jpg someone@example.com support@acme.test';
  const { emails } = extractContacts({ html: '', text, ...both });

  assert.deepEqual(emails.map((e) => e.address), ['support@acme.test']);
});

test('obfuscated HTML-entity emails are decoded', () => {
  const html = '<a href="mailto:info&#64;acme.test">write us</a>';
  const { emails } = extractContacts({ html, text: '', ...both });

  assert.equal(emails[0]?.address, 'info@acme.test');
});

test('entity-obfuscated contacts in visible text are decoded before scanning', () => {
  const { emails } = extractContacts({
    html: '',
    text: 'Reach purchasing&#64;acme.test for quotes',
    ...both,
  });

  assert.deepEqual(emails.map((e) => e.address), ['purchasing@acme.test']);
});

test('inline script and style bodies are not mined for contacts', () => {
  const html = `
    <script>
      var cfg = { supportHref: "tel:+15005550001", owner: "ops@internal-vendor.test" };
    </script>
    <style>/* mailto:designer@theme-shop.test */</style>
    <a href="tel:+14155550134">Real line</a>
  `;
  const { phones, emails } = extractContacts({ html, text: '', ...both });

  assert.deepEqual(phones.map((p) => p.e164), ['+14155550134']);
  assert.deepEqual(emails, []);
});

test('flags gate collection so callers only take the PII they asked for', () => {
  const html = '<a href="tel:+14155550134">c</a><a href="mailto:a@acme.test">e</a>';

  const phonesOnly = extractContacts({
    html,
    text: '',
    ...both,
    includeEmails: false,
  });
  assert.equal(phonesOnly.phones.length, 1);
  assert.deepEqual(phonesOnly.emails, []);

  const neither = extractContacts({
    html,
    text: '',
    ...both,
    includePhones: false,
    includeEmails: false,
  });
  assert.deepEqual(neither.phones, []);
  assert.deepEqual(neither.emails, []);
});

test('results are capped by maxPhones/maxEmails', () => {
  const text = 'Call (212) 555-0147 or (415) 555-0188 or (312) 555-0199';
  const { phones } = extractContacts({ html: '', text, ...both, maxPhones: 2 });
  assert.equal(phones.length, 2);
});

test('normalizePhoneNumber keeps national digits when the region is unknown', () => {
  const result = normalizePhoneNumber('020 7946 0958', undefined);
  assert.equal(result?.e164, undefined);
  assert.equal(result?.digits, '02079460958');
});

test('normalizePhoneNumber handles the 00 international prefix', () => {
  assert.equal(normalizePhoneNumber('0044 20 7946 0958', 'US')?.e164, '+442079460958');
});

test('normalizeEmail rejects malformed addresses', () => {
  assert.equal(normalizeEmail('not-an-email'), undefined);
  assert.equal(normalizeEmail('a@b'), undefined);
  assert.equal(normalizeEmail('user@acme.test.'), 'user@acme.test');
});

test('structurally impossible NANP numbers are rejected, not stored', () => {
  // Area code and exchange must start 2-9. These would otherwise become
  // "+11115550100" / "+14150550100" and pollute the CRM.
  assert.equal(normalizePhoneNumber('(111) 555-0100', 'US'), undefined);
  assert.equal(normalizePhoneNumber('(415) 055-0100', 'US'), undefined);
  assert.equal(normalizePhoneNumber('+1 111 555 0100', 'US'), undefined);
  // A well-formed NANP number still passes.
  assert.equal(normalizePhoneNumber('(415) 555-0100', 'US')?.e164, '+14155550100');
});

test('E.164 country codes cannot start with zero', () => {
  assert.equal(normalizePhoneNumber('+0 20 7946 0958', 'US'), undefined);
  assert.equal(normalizePhoneNumber('+44 20 7946 0958', 'US')?.e164, '+442079460958');
});

test('invalid NANP candidates in text are dropped end-to-end', () => {
  const { phones } = extractContacts({
    html: '',
    text: 'Call (111) 555-0100 or our real line (628) 555-0100',
    ...both,
  });
  assert.deepEqual(phones.map((p) => p.e164), ['+16285550100']);
});

test('scanning pathological input stays fast (no ReDoS)', () => {
  // A long run of email/phone-shaped characters with no delimiter is the classic
  // catastrophic-backtracking trigger. Bounded quantifiers keep this linear.
  const html = `${'a'.repeat(400_000)} ${'1'.repeat(400_000)}`;
  const started = process.hrtime.bigint();
  const { phones, emails } = extractContacts({ html, text: html, ...both });
  const elapsedMs = Number(process.hrtime.bigint() - started) / 1e6;

  assert.deepEqual(phones, []);
  assert.deepEqual(emails, []);
  assert.ok(elapsedMs < 1_000, `contact scan took ${elapsedMs.toFixed(0)}ms — expected < 1000ms`);
});
