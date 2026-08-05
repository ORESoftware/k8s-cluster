import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { test } from 'node:test';

import { chromium } from 'playwright';

const commandBase = process.env.SLACK_COMMAND_BASE_URL ?? 'http://127.0.0.1:8151';
const slackMockBase = process.env.SLACK_MOCK_BASE_URL ?? 'http://127.0.0.1:8170';
const signingSecret = process.env.SLACK_SIGNING_SECRET ?? 'integration-signing-secret';
const slackAppId = process.env.SLACK_EXPECTED_APP_ID ?? 'A0BMBAMM5NJ';
const slackTeamId = process.env.SLACK_EXPECTED_TEAM_ID ?? 'T01B3C83PMK';
const slackChannelId = process.env.SLACK_EXPECTED_CHANNEL_ID ?? 'C0BKP2N3LG7';
const slackUserId = process.env.SLACK_EXPECTED_USER_ID ?? 'U01AZNU2LJ2';
const outputDir =
  process.env.PLAYWRIGHT_OUTPUT_DIR ?? path.resolve('test-results/slack-agent-command-security');

function encodedCommand({
  appId = slackAppId,
  team = slackTeamId,
  channel = slackChannelId,
  user = slackUserId,
  command = '/ores-chatgpt',
  text = '',
  trigger,
}) {
  return new URLSearchParams({
    api_app_id: appId,
    command,
    team_id: team,
    channel_id: channel,
    user_id: user,
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

async function postSigned(request, pathName, body, timestamp) {
  return request.post(`${commandBase}${pathName}`, {
    headers: slackHeaders(body, timestamp),
    data: body,
  });
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

async function assertRejectedWithoutPromptReflection(response, expectedStatus) {
  const body = await response.text();
  assert.equal(response.status(), expectedStatus, body);
  assert.equal(body.includes('DEN-1041'), false, 'denial reflected private prompt text');
  assert.equal(body.includes('<script>'), false, 'denial reflected hostile markup');
  return body;
}

test(
  'Slack signed ingress rejects forged or unauthorized requests without side effects',
  { timeout: 60_000 },
  async () => {
    await mkdir(outputDir, { recursive: true, mode: 0o700 });
    const browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ serviceWorkers: 'block' });
    const page = await context.newPage();
    const request = context.request;

    page.setDefaultTimeout(10_000);
    page.setDefaultNavigationTimeout(15_000);

    await context.route('**/*', async (route) => {
      const hostname = new URL(route.request().url()).hostname;
      if (['127.0.0.1', 'localhost', '::1', '[::1]'].includes(hostname)) {
        await route.continue();
        return;
      }
      await route.abort('blockedbyclient');
    });

    try {
      await resetMock(request);

      const readyResponse = await page.goto(`${commandBase}/readyz`);
      assert.ok(readyResponse);
      assert.equal(readyResponse.status(), 200);
      const ready = JSON.parse(await page.locator('body').innerText());
      assert.deepEqual(
        {
          ok: ready.ok,
          dry_run: ready.dry_run,
          installed_app_identity_enforced: ready.installed_app_identity_enforced,
        },
        {
          ok: true,
          dry_run: true,
          installed_app_identity_enforced: true,
        },
      );

      let blockedNavigation;
      try {
        await page.goto('https://example.com/');
      } catch (error) {
        blockedNavigation = error;
      }
      assert.match(String(blockedNavigation), /ERR_BLOCKED_BY_CLIENT/);

      const hostilePrompt =
        'Investigate DEN-1041 <script>globalThis.__slackIngressXss = true</script>';
      const missingSignatureBody = encodedCommand({
        text: hostilePrompt,
        trigger: 'security-missing-signature',
      });
      const missingSignature = await request.post(
        `${commandBase}/slack/commands/ores-chatgpt`,
        {
          headers: { 'content-type': 'application/x-www-form-urlencoded' },
          data: missingSignatureBody,
        },
      );
      await assertRejectedWithoutPromptReflection(missingSignature, 401);

      const forgedBody = encodedCommand({
        text: hostilePrompt,
        trigger: 'security-forged-signature',
      });
      const forgedHeaders = slackHeaders(forgedBody);
      forgedHeaders['x-slack-signature'] = `v0=${'0'.repeat(64)}`;
      const forged = await request.post(`${commandBase}/slack/commands/ores-chatgpt`, {
        headers: forgedHeaders,
        data: forgedBody,
      });
      await assertRejectedWithoutPromptReflection(forged, 401);

      const staleBody = encodedCommand({
        text: hostilePrompt,
        trigger: 'security-stale-signature',
      });
      const stale = await postSigned(
        request,
        '/slack/commands/ores-chatgpt',
        staleBody,
        Math.floor(Date.now() / 1000) - 301,
      );
      await assertRejectedWithoutPromptReflection(stale, 401);

      const wrongAppBody = encodedCommand({
        appId: 'A0WRONGAPP',
        text: hostilePrompt,
        trigger: 'security-wrong-app',
      });
      const wrongApp = await postSigned(
        request,
        '/slack/commands/ores-chatgpt',
        wrongAppBody,
      );
      await assertRejectedWithoutPromptReflection(wrongApp, 403);

      const wrongTeamBody = encodedCommand({
        team: 'T0WRONGTEAM',
        text: hostilePrompt,
        trigger: 'security-wrong-team',
      });
      const wrongTeam = await postSigned(
        request,
        '/slack/commands/ores-chatgpt',
        wrongTeamBody,
      );
      await assertRejectedWithoutPromptReflection(wrongTeam, 403);

      const unauthorizedBody = encodedCommand({
        channel: 'C0UNAUTHORIZED',
        text: hostilePrompt,
        trigger: 'security-unauthorized-channel',
      });
      const unauthorized = await postSigned(
        request,
        '/slack/commands/ores-chatgpt',
        unauthorizedBody,
      );
      await assertRejectedWithoutPromptReflection(unauthorized, 403);

      const wrongEndpointBody = encodedCommand({
        command: '/ores-claude',
        text: hostilePrompt,
        trigger: 'security-provider-confusion',
      });
      const wrongEndpoint = await postSigned(
        request,
        '/slack/commands/ores-chatgpt',
        wrongEndpointBody,
      );
      await assertRejectedWithoutPromptReflection(wrongEndpoint, 400);

      const duplicateNormalizedKeyBody = [
        `api_app_id=${encodeURIComponent(slackAppId)}`,
        `team_id=${encodeURIComponent(slackTeamId)}`,
        `team%5Fid=${encodeURIComponent(slackTeamId)}`,
        `channel_id=${encodeURIComponent(slackChannelId)}`,
        `user_id=${encodeURIComponent(slackUserId)}`,
        'command=%2Fores-chatgpt',
        `text=${encodeURIComponent(hostilePrompt)}`,
        'trigger_id=security-duplicate-normalized-key',
      ].join('&');
      const duplicateNormalizedKey = await postSigned(
        request,
        '/slack/commands/ores-chatgpt',
        duplicateNormalizedKeyBody,
      );
      await assertRejectedWithoutPromptReflection(duplicateNormalizedKey, 400);

      let state = await mockState(request);
      assert.deepEqual(
        {
          historyCalls: state.historyCalls,
          messages: state.messages.length,
          views: state.views.length,
        },
        { historyCalls: 0, messages: 0, views: 0 },
        'rejected Slack requests must not read history or create Slack side effects',
      );

      const hostileStatus =
        '<script>globalThis.__slackDashboardXss = true</script>' +
        '<img src=x onerror="globalThis.__slackDashboardXss = true">';
      const injected = await request.post(`${slackMockBase}/api/chat.postMessage`, {
        data: { text: hostileStatus },
      });
      assert.equal(injected.status(), 200, await injected.text());

      await page.goto(`${slackMockBase}/`);
      await page
        .getByRole('heading', { name: 'ORESoftware Slack command integration dashboard' })
        .waitFor();
      assert.equal(await page.getByTestId('message-count').innerText(), '1');
      assert.equal(await page.getByTestId('status-0').innerText(), hostileStatus);
      assert.equal(await page.getByTestId('statuses').locator('script').count(), 0);
      assert.equal(await page.getByTestId('statuses').locator('img').count(), 0);
      assert.equal(await page.evaluate(() => globalThis.__slackDashboardXss), undefined);

      await page.screenshot({
        path: path.join(outputDir, 'slack-agent-command-security.png'),
        fullPage: true,
      });

      await resetMock(request);
      state = await mockState(request);
      assert.deepEqual(state, { historyCalls: 0, views: [], messages: [] });
    } finally {
      await context.close();
      await browser.close();
    }
  },
);
