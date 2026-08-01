import assert from 'node:assert/strict';
import test from 'node:test';

import {
  classifySensitiveMessage,
  isHighConfidenceContactOnly,
  redactSensitiveContent,
  scanSensitiveContent,
} from '../sensitive-content.mjs';
import { sanitizeDocuments } from '../sanitize-export.mjs';

const secretFixtures = [
  ['aws_access_key_id', `AccessKeyId: ${'AKIA' + 'ABCDEFGHIJKLMNOP'}`],
  ['aws_secret_access_key', `"SecretAccessKey": "${'abcdefghijklmnop' + 'qrstuvwxyz0123456789ABCD'}"`],
  ['github_token', `${'github_' + 'pat_'}11AA22BB33CC44DD55EE66FF77GG88HH99`],
  ['linear_api_key', `LINEAR_API_KEY=${'lin_' + 'api_'}abcdefghijklmnopqrstuvwxyz012345`],
  ['google_chat_bridge_token', `CHAT_BRIDGE_TOKEN=${'abcdefghijklmnop' + 'qrstuvwxyz0123456789'}`],
  ['slack_token', `${'xoxb' + '-'}123456789012-abcdefghijklmnopqrstuvwxyz`],
  ['google_api_key', `${'AIza' + 'SyA'}12345678901234567890123456789012`],
  ['sendgrid_api_key', `${'S' + 'G.'}abcdefghijklmnopqrstuv.abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG`],
  ['supabase_key', `${'sb_' + 'publishable_'}abcdefghijklmnopqrstuvwxyz012345`],
  ['openai_api_key', `${'sk-' + 'proj-'}abcdefghijklmnopqrstuvwxyz012345`],
  ['anthropic_api_key', `${'sk-' + 'ant-'}abcdefghijklmnopqrstuvwxyz012345`],
  ['stripe_secret_key', `${'sk_' + 'live_'}abcdefghijklmnopqrstuvwxyz`],
  ['huggingface_token', `${'hf_' + 'abcdefghijklmnopqrstuvwx'}`],
  ['npm_token', `${'npm_' + 'abcdefghijklmnopqrstuvwx'}`],
  ['pypi_token', `${'pypi-' + 'abcdefghijklmnopqrstuvwx'}`],
  ['gitlab_token', `${'glpat-' + 'abcdefghijklmnopqrstuvwx'}`],
  ['digitalocean_token', `${'dop_' + 'v1_'}0123456789abcdef0123456789abcdef`],
  ['twilio_api_key', `${'S' + 'K'}0123456789abcdef0123456789abcdef`],
  ['jwt', `${'eyJ' + 'abcdefghijk'}.${'abcdefghijklmnop'}.${'qrstuvwxyz012345'}`],
  ['url_password', `postgresql://test:${'secret-' + 'password-0123456789'}@db.example.invalid/app`],
  ['bearer_token', `Authorization: Bearer ${'abcdefghijklmnop' + 'qrstuvwxyz0123456789'}`],
  ['assigned_secret', `CLIENT_SECRET=${'abcdefghijklmnop' + 'qrstuvwxyz0123456789'}`],
  [
    'private_key',
    [`-----BEGIN ${'PRIVATE' + ' KEY'}-----`, 'abcdef', `-----END ${'PRIVATE' + ' KEY'}-----`].join('\n'),
  ],
];

for (const [kind, fixture] of secretFixtures) {
  test(`detects and redacts ${kind}`, () => {
    const findings = scanSensitiveContent(fixture);
    assert.ok(findings.some((finding) => finding.kind === kind));
    const redacted = redactSensitiveContent(fixture, findings);
    assert.match(redacted, new RegExp(`\\[REDACTED:${kind}\\]`));
    assert.notEqual(redacted, fixture);
  });
}

test('does not flag architectural discussion without assigned values', () => {
  const text = 'Rotate the API key, bearer token, and client secret after the import.';
  assert.deepEqual(scanSensitiveContent(text), []);
});

test('classifies high-confidence contact-only messages without treating ordinary numbers as contacts', () => {
  assert.equal(isHighConfidenceContactOnly('+1 202 555 0147 test contact'), true);
  assert.equal(isHighConfidenceContactOnly('test contact 2025550147'), true);
  assert.equal(isHighConfidenceContactOnly('WhatsApp agregame +1 202 555 0147'), true);
  assert.equal(isHighConfidenceContactOnly('202 555 0147wpp'), true);
  assert.equal(isHighConfidenceContactOnly('100 variables, 150 constraints, objective 47824'), false);
  assert.equal(isHighConfidenceContactOnly('100.1.1.250'), false);
});

test('sanitizer preserves provenance but never emits secret or contact values', async () => {
  const awsKey = 'AKIA' + 'ABCDEFGHIJKLMNOP';
  const phone = '+1 202 555 0147 test contact';
  const document = {
    ok: true,
    data: {
      messages: [
        {
          sourceKey: 'google-chat:space:secret',
          name: 'spaces/test/messages/secret',
          createTime: '2026-06-09T22:14:40Z',
          text: `AccessKeyId: ${awsKey}`,
          attachments: [{ name: 'secret.txt' }],
        },
        {
          sourceKey: 'google-chat:space:contact',
          name: 'spaces/test/messages/contact',
          createTime: '2026-08-01T00:21:08Z',
          text: phone,
        },
        {
          sourceKey: 'google-chat:space:safe',
          name: 'spaces/test/messages/safe',
          createTime: '2026-08-01T15:00:00Z',
          text: 'Add regression tests for the importer.',
        },
      ],
    },
  };

  const { sanitized, report } = await sanitizeDocuments(
    [{ document, filePath: '/private/messages.json' }],
    { since: '2026-06-05T00:00:00Z' },
  );
  const output = JSON.stringify({ sanitized, report });

  assert.equal(report.messagesSeen, 3);
  assert.equal(report.sensitiveMessages, 1);
  assert.equal(report.privateContactMessages, 1);
  assert.equal(report.safeMessages, 1);
  assert.equal(report.quarantined.length, 2);
  assert.ok(report.quarantined.every((entry) => !('text' in entry)));
  assert.doesNotMatch(output, new RegExp(awsKey));
  assert.doesNotMatch(output, new RegExp(phone.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
  assert.equal(sanitized[0].document.data.messages[0].text, '');
  assert.deepEqual(sanitized[0].document.data.messages[0].attachments, []);
  assert.equal(sanitized[0].document.data.messages[2].text, 'Add regression tests for the importer.');
});
