import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import { test } from 'node:test';

import { chromium } from 'playwright';

const commandBase = process.env.SLACK_COMMAND_BASE_URL ?? 'http://127.0.0.1:8151';
const slackMockBase = process.env.SLACK_MOCK_BASE_URL ?? 'http://127.0.0.1:8170';
const signingSecret = process.env.SLACK_SIGNING_SECRET ?? 'integration-signing-secret';
const slackAppId = process.env.SLACK_EXPECTED_APP_ID ?? 'A0BMBAMM5NJ';
const slackTeamId = process.env.SLACK_EXPECTED_TEAM_ID ?? 'T01B3C83PMK';
const slackChannelId = 'C0BKP2N3LG7';
const slackUserId = 'U01AZNU2LJ2';

function encodedCommand({ channel = slackChannelId, text = '', trigger }) {
  return new URLSearchParams({
    api_app_id: slackAppId,
    command: '/ores-chatgpt',
    team_id: slackTeamId,
    channel_id: channel,
    user_id: slackUserId,
    text,
    trigger_id: trigger,
  }).toString();
}

function slackHeaders(body, timestamp = Math.floor(Date.now() / 1000)) {
  const signature = createHmac('sha256', signingSecret)
    .update(`v0:${timestamp}:${body}`)
    .digest('hex');
  return {
    'content-type': 'application/x-www-form-urlencoded',
    'x-slack-request-timestamp': String(timestamp),
    'x-slack-signature': `v0=${signature}`,
  };
}

async function resetMock(request) {
  const response = await request.post(`${slackMockBase}/admin/reset`);
  assert.equal(response.status(), 200, await response.text());
}

async function mockState(request) {
  const response = await request.get(`${slackMockBase}/admin/state`);
  assert.equal(response.status(), 200, await response.text());
  return response.json();
}

test(
  'Slack browser boundary rejects forged traffic without side effects and safely renders evidence',
  { timeout: 45_000 },
  async () => {
    const browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ serviceWorkers: 'block' });
    const page = await context.newPage();
    const request = context.request;

    page.setDefaultTimeout(10_000);
    page.setDefaultNavigationTimeout(15_000);

    await context.route('**/*', async (route) => {
      const hostname = new URL(route.request().url()).hostname;
      if (hostname === '127.0.0.1' || hostname === 'localhost') {
        await route.continue();
        return;
      }
      await route.abort('blockedbyclient');
    });

    try {
      await resetMock(request);
      await page.goto(`${slackMockBase}/`);
      await page
        .getByRole('heading', { name: 'ORESoftware Slack command integration dashboard' })
        .waitFor();

      const missingSignatureBody = encodedCommand({
        text: 'must not run without a Slack signature',
        trigger: 'security-missing-signature',
      });
      const missingSignature = await request.post(
        `${commandBase}/slack/commands/ores-chatgpt`,
        {
          headers: { 'content-type': 'application/x-www-form-urlencoded' },
          data: missingSignatureBody,
        },
      );
      assert.equal(missingSignature.status(), 401, await missingSignature.text());

      const forgedBody = encodedCommand({
        text: 'must not run with a forged Slack signature',
        trigger: 'security-forged-signature',
      });
      const forgedHeaders = slackHeaders(forgedBody);
      forgedHeaders['x-slack-signature'] = `v0=${'0'.repeat(64)}`;
      const forged = await request.post(`${commandBase}/slack/commands/ores-chatgpt`, {
        headers: forgedHeaders,
        data: forgedBody,
      });
      assert.equal(forged.status(), 401, await forged.text());

      const staleBody = encodedCommand({
        text: 'must not run after the replay window',
        trigger: 'security-stale-signature',
      });
      const staleTimestamp = Math.floor(Date.now() / 1000) - 600;
      const stale = await request.post(`${commandBase}/slack/commands/ores-chatgpt`, {
        headers: slackHeaders(staleBody, staleTimestamp),
        data: staleBody,
      });
      assert.equal(stale.status(), 401, await stale.text());

      const unauthorizedBody = encodedCommand({
        channel: 'C0000000000',
        text: 'must not run outside the bound pilot channel',
        trigger: 'security-unauthorized-channel',
      });
      const unauthorized = await request.post(
        `${commandBase}/slack/commands/ores-chatgpt`,
        {
          headers: slackHeaders(unauthorizedBody),
          data: unauthorizedBody,
        },
      );
      assert.equal(unauthorized.status(), 403, await unauthorized.text());

      let state = await mockState(request);
      assert.deepEqual(
        {
          historyCalls: state.historyCalls,
          messages: state.messages.length,
          views: state.views.length,
        },
        { historyCalls: 0, messages: 0, views: 0 },
        'rejected Slack requests must not read channel history or create visible side effects',
      );

      const hostileStatus =
        '<script>globalThis.__slackDashboardXss = true</script>' +
        '<img src=x onerror="globalThis.__slackDashboardXss = true">';
      const injected = await request.post(`${slackMockBase}/api/chat.postMessage`, {
        data: { text: hostileStatus },
      });
      assert.equal(injected.status(), 200, await injected.text());

      await page.reload();
      assert.equal(await page.getByTestId('message-count').innerText(), '1');
      assert.equal(await page.getByTestId('status-0').innerText(), hostileStatus);
      assert.equal(await page.getByTestId('statuses').locator('script').count(), 0);
      assert.equal(await page.getByTestId('statuses').locator('img').count(), 0);
      assert.equal(await page.evaluate(() => globalThis.__slackDashboardXss), undefined);

      await page.screenshot({
        path: `${process.env.PLAYWRIGHT_OUTPUT_DIR ?? 'test-results'}/slack-agent-command-security.png`,
        fullPage: true,
      });

      await resetMock(request);
      state = await mockState(request);
      assert.equal(state.messages.length, 0);
    } finally {
      await context.close();
      await browser.close();
    }
  },
);
