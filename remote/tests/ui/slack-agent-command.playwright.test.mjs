import assert from 'node:assert/strict';
import { createHmac } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import { test } from 'node:test';
import path from 'node:path';

import { chromium } from 'playwright';

const commandBase = process.env.SLACK_COMMAND_BASE_URL ?? 'http://127.0.0.1:8151';
const slackMockBase = process.env.SLACK_MOCK_BASE_URL ?? 'http://127.0.0.1:8170';
const bridgeBase = process.env.BRIDGE_BASE_URL ?? 'http://127.0.0.1:8142';
const coordinatorBase = process.env.COORDINATOR_BASE_URL ?? 'http://127.0.0.1:8160';
const signingSecret = process.env.SLACK_SIGNING_SECRET ?? 'integration-signing-secret';
const bridgeBearer = process.env.SLACK_BRIDGE_BEARER ?? 'bridge-test-token';
const coordinatorBearer = process.env.SLACK_COORDINATOR_BEARER ?? 'coordinator-test-token';
const slackAppId = process.env.SLACK_EXPECTED_APP_ID ?? 'A0BMBAMM5NJ';
const slackTeamId = process.env.SLACK_EXPECTED_TEAM_ID ?? 'T01B3C83PMK';
const slackChannelId = 'C0BKP2N3LG7';
const slackUserId = 'U01AZNU2LJ2';
const outputDir = process.env.PLAYWRIGHT_OUTPUT_DIR ?? path.resolve('test-results/slack-agent-command');

function encodedCommand({
  command,
  appId = slackAppId,
  team = slackTeamId,
  channel = slackChannelId,
  user = slackUserId,
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

function encodedInteraction(payload) {
  return new URLSearchParams({ payload: JSON.stringify(payload) }).toString();
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

async function getJson(request, url, options = {}) {
  const response = await request.get(url, options);
  assert.equal(response.ok(), true, `${url} returned ${response.status()}: ${await response.text()}`);
  return response.json();
}

async function mockState(request) {
  return getJson(request, `${slackMockBase}/admin/state`);
}

async function waitFor(request, predicate, description) {
  const deadline = Date.now() + 15_000;
  let last;
  while (Date.now() < deadline) {
    last = await mockState(request);
    if (predicate(last)) return last;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.fail(`${description}; last state=${JSON.stringify(last)}`);
}

function idsFromStatus(text) {
  const job = /Coordinator job: `([^`]+)`/.exec(text)?.[1];
  const workflow = /Bridge workflow: `([^`]+)`/.exec(text)?.[1];
  const run = /Run: `([^`]+)`/.exec(text)?.[1];
  assert.ok(job, `missing coordinator job id in ${text}`);
  assert.ok(workflow, `missing bridge workflow id in ${text}`);
  assert.ok(run, `missing run id in ${text}`);
  return { job, workflow, run };
}

async function verifyDispatch(
  request,
  statusText,
  {
    expectedProvider,
    expectedAgent,
    expectedRepository,
    expectedIssue,
    expectedAction,
  },
) {
  const ids = idsFromStatus(statusText);
  const coordinator = await getJson(request, `${coordinatorBase}/v1/jobs/${ids.job}`, {
    headers: { authorization: `Bearer ${coordinatorBearer}` },
  });
  const bridge = await getJson(request, `${bridgeBase}/workflows/${ids.workflow}`, {
    headers: { authorization: `Bearer ${bridgeBearer}` },
  });

  const job = coordinator.job;
  const payload = job.payload;
  const normalizedRepository = expectedRepository.toLowerCase();
  assert.equal(job.task_type, 'slack_agent_run');
  assert.equal(payload.run_id, ids.run);
  assert.equal(payload.provider, expectedProvider);
  assert.equal(payload.action, expectedAction);
  assert.equal(payload.bridge_workflow_id, ids.workflow);
  assert.equal(payload.origin.workspace_id, slackTeamId);
  assert.equal(payload.origin.channel_id, slackChannelId);
  assert.equal(payload.origin.requester_user_id, slackUserId);
  assert.equal(payload.routing.repository.toLowerCase(), normalizedRepository);
  assert.equal(`${job.org}/${job.repo}`.toLowerCase(), normalizedRepository);
  assert.equal(payload.routing.linear_issue, expectedIssue);
  assert.equal(
    payload.routing.linear_run_project_id,
    '72e891e2-603d-4903-8d08-bd06d204520f',
  );
  assert.equal(payload.context.trust, 'untrusted_channel_context');
  assert.deepEqual(
    payload.context.messages.map((message) => message.text),
    ['message-2', 'message-3', 'message-4', 'message-5', 'message-6'],
  );
  assert.equal(payload.context.messages.some((message) => message.text === 'ignore-bot'), false);
  assert.deepEqual(
    [...payload.broadcast_targets].sort(),
    [
      'ai_agent_bridge_workflow',
      'ai_agent_coordinator_job',
      'github_branch_pr_checks',
      'linear_run_queue',
      'slack_run_thread',
    ].sort(),
  );

  assert.equal(bridge.workflow.plan.assignments.length, 1);
  assert.equal(bridge.workflow.plan.assignments[0].agent_key, expectedAgent);
  assert.equal(bridge.workflow.plan.meta.repository.toLowerCase(), normalizedRepository);
  assert.equal(
    Object.hasOwn(bridge.workflow.plan, 'file_lease'),
    false,
    'Slack repository metadata must not opt the workflow into file leases',
  );

  const duplicate = await request.post(`${coordinatorBase}/v1/jobs`, {
    headers: {
      authorization: `Bearer ${coordinatorBearer}`,
      'idempotency-key': `slack-command:${ids.run}`,
    },
    data: {
      org: job.org,
      repo: job.repo,
      task_type: job.task_type,
      payload: job.payload,
      priority: job.priority,
      max_attempts: job.max_attempts,
      budget_usd: job.budget_usd,
    },
  });
  assert.equal(duplicate.status(), 202, await duplicate.text());
  assert.equal((await duplicate.json()).job.id, ids.job, 'coordinator idempotency created a second job');
  return ids;
}

test('Slack slash commands traverse browser, modal, bridge, coordinator, PostgreSQL, and Slack contracts', async () => {
  await mkdir(outputDir, { recursive: true });
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext();
  const page = await context.newPage();
  const request = context.request;

  try {
    await page.goto(`${commandBase}/readyz`);
    const ready = JSON.parse(await page.locator('body').innerText());
    assert.deepEqual(
      {
        ok: ready.ok,
        dry_run: ready.dry_run,
        default_context_messages: ready.default_context_messages,
        installed_app_identity_enforced: ready.installed_app_identity_enforced,
      },
      {
        ok: true,
        dry_run: false,
        default_context_messages: 5,
        installed_app_identity_enforced: true,
      },
    );

    await page.goto(`${slackMockBase}/`);
    await page.getByRole('heading', { name: 'ORESoftware Slack command integration dashboard' }).waitFor();
    assert.equal(await page.getByTestId('modal-count').innerText(), '0');
    assert.equal(await page.getByTestId('message-count').innerText(), '0');

    const wrongEndpointBody = encodedCommand({
      command: '/ores-chatgpt',
      text: 'Implement DEN-1041',
      trigger: 'trigger-wrong-endpoint',
    });
    const wrongEndpoint = await postSigned(
      request,
      '/slack/commands/ores-claude',
      wrongEndpointBody,
    );
    assert.equal(wrongEndpoint.status(), 400, await wrongEndpoint.text());

    const wrongAppBody = encodedCommand({
      command: '/ores-chatgpt',
      appId: 'A0WRONGAPP',
      text: 'Implement DEN-1041',
      trigger: 'trigger-wrong-app',
    });
    const wrongApp = await postSigned(request, '/slack/commands/ores-chatgpt', wrongAppBody);
    assert.equal(wrongApp.status(), 403, await wrongApp.text());

    const wrongTeamBody = encodedCommand({
      command: '/ores-chatgpt',
      team: 'T0WRONGTEAM',
      text: 'Implement DEN-1041',
      trigger: 'trigger-wrong-team',
    });
    const wrongTeam = await postSigned(request, '/slack/commands/ores-chatgpt', wrongTeamBody);
    assert.equal(wrongTeam.status(), 403, await wrongTeam.text());

    let state = await mockState(request);
    assert.equal(state.views.length, 0);
    assert.equal(state.messages.length, 0);
    assert.equal(state.historyCalls, 0);

    const modalBody = encodedCommand({ command: '/ores-claude', trigger: 'trigger-modal' });
    const modalResponse = await postSigned(request, '/slack/commands/ores-claude', modalBody);
    assert.equal(modalResponse.status(), 200, await modalResponse.text());
    state = await waitFor(request, (candidate) => candidate.views.length === 1, 'modal was not opened');

    await page.reload();
    assert.equal(await page.getByTestId('modal-count').innerText(), '1');
    assert.equal(await page.getByTestId('modal-callback').innerText(), 'ores-agent-run-v1');
    assert.equal(
      await page.getByTestId('modal-blocks').innerText(),
      'task,action,repository,issue,write_scope,context_messages',
    );
    assert.equal(await page.getByTestId('context-default').innerText(), '5');
    assert.equal(await page.getByTestId('write-default').innerText(), 'draft_pull_request');

    const privateMetadata = state.views[0].view.private_metadata;
    assert.equal(typeof privateMetadata, 'string');

    const wrongInteractionBody = encodedInteraction({
      api_app_id: 'A0WRONGAPP',
      type: 'view_submission',
      team: { id: slackTeamId },
      user: { id: slackUserId },
      view: {
        id: 'V-modal-submit-wrong-app',
        callback_id: 'ores-agent-run-v1',
        private_metadata: privateMetadata,
        state: { values: {} },
      },
    });
    const wrongInteraction = await postSigned(
      request,
      '/slack/interactions',
      wrongInteractionBody,
    );
    assert.equal(wrongInteraction.status(), 403, await wrongInteraction.text());

    const interactionBody = encodedInteraction({
      api_app_id: slackAppId,
      type: 'view_submission',
      team: { id: slackTeamId },
      user: { id: slackUserId },
      view: {
        id: 'V-modal-submit-1',
        callback_id: 'ores-agent-run-v1',
        private_metadata: privateMetadata,
        state: {
          values: {
            task: { task: { value: 'Investigate DEN-1231 with tests' } },
            action: { action: { selected_option: { value: 'investigate' } } },
            repository: {
              repository: {
                selected_option: { value: 'ORESoftware/ai-agent-coordinator.rs' },
              },
            },
            issue: { issue: { value: 'DEN-1231' } },
            write_scope: {
              write_scope: { selected_option: { value: 'draft_pull_request' } },
            },
            context_messages: {
              context_messages: { selected_option: { value: '5' } },
            },
          },
        },
      },
    });
    const interactionResponse = await postSigned(request, '/slack/interactions', interactionBody);
    assert.equal(interactionResponse.status(), 200, await interactionResponse.text());
    state = await waitFor(
      request,
      (candidate) => candidate.messages.length === 1,
      'Claude modal dispatch status was not posted',
    );
    await verifyDispatch(request, state.messages[0].text, {
      expectedProvider: 'claude',
      expectedAgent: 'claude-fable-5',
      expectedRepository: 'ORESoftware/ai-agent-coordinator.rs',
      expectedIssue: 'DEN-1231',
      expectedAction: 'investigate',
    });

    const chatgptBody = encodedCommand({
      command: '/ores-chatgpt',
      text: 'Implement DEN-1041 with tests',
      trigger: 'trigger-chatgpt',
    });
    const chatgptResponse = await postSigned(request, '/slack/commands/ores-chatgpt', chatgptBody);
    assert.equal(chatgptResponse.status(), 200, await chatgptResponse.text());
    assert.match((await chatgptResponse.json()).text, /Accepted ChatGPT run/);
    state = await waitFor(
      request,
      (candidate) => candidate.messages.length === 2,
      'ChatGPT dispatch status was not posted',
    );
    await verifyDispatch(request, state.messages[1].text, {
      expectedProvider: 'chatgpt',
      expectedAgent: 'gpt-5.6-sol',
      expectedRepository: 'ORESoftware/ai-agent-bridge.rs',
      expectedIssue: 'DEN-1041',
      expectedAction: 'implement',
    });

    const claudeBody = encodedCommand({
      command: '/ores-claude',
      text: 'Investigate DEN-1231 with tests',
      trigger: 'trigger-claude-direct',
    });
    const claudeResponse = await postSigned(request, '/slack/commands/ores-claude', claudeBody);
    assert.equal(claudeResponse.status(), 200, await claudeResponse.text());
    assert.match((await claudeResponse.json()).text, /Accepted Claude run/);
    state = await waitFor(
      request,
      (candidate) => candidate.messages.length === 3,
      'Direct Claude dispatch status was not posted',
    );
    await verifyDispatch(request, state.messages[2].text, {
      expectedProvider: 'claude',
      expectedAgent: 'claude-fable-5',
      expectedRepository: 'ORESoftware/ai-agent-bridge.rs',
      expectedIssue: 'DEN-1231',
      expectedAction: 'investigate',
    });

    const duplicateResponse = await postSigned(request, '/slack/commands/ores-chatgpt', chatgptBody);
    assert.equal(duplicateResponse.status(), 200, await duplicateResponse.text());
    assert.match((await duplicateResponse.json()).text, /already accepted/);
    await new Promise((resolve) => setTimeout(resolve, 250));
    state = await mockState(request);
    assert.equal(state.messages.length, 3, 'duplicate Slack request posted a second status');

    const unauthorizedBody = encodedCommand({
      command: '/ores-chatgpt',
      channel: 'C0UNAUTHORIZED',
      text: 'Implement DEN-1041',
      trigger: 'trigger-unauthorized',
    });
    const unauthorized = await postSigned(request, '/slack/commands/ores-chatgpt', unauthorizedBody);
    assert.equal(unauthorized.status(), 403, await unauthorized.text());

    const staleBody = encodedCommand({
      command: '/ores-chatgpt',
      text: 'Implement DEN-1041',
      trigger: 'trigger-stale',
    });
    const stale = await postSigned(
      request,
      '/slack/commands/ores-chatgpt',
      staleBody,
      Math.floor(Date.now() / 1000) - 600,
    );
    assert.equal(stale.status(), 401, await stale.text());

    state = await mockState(request);
    assert.equal(state.messages.length, 3);
    assert.equal(state.views.length, 1);
    assert.equal(state.historyCalls, 3, 'one bounded history read is performed per live dispatch');

    await page.goto(`${slackMockBase}/`);
    assert.equal(await page.getByTestId('message-count').innerText(), '3');
    await page.screenshot({
      path: path.join(outputDir, 'slack-agent-command-dashboard.png'),
      fullPage: true,
    });
  } catch (error) {
    await page
      .screenshot({
        path: path.join(outputDir, 'slack-agent-command-failure.png'),
        fullPage: true,
      })
      .catch(() => {});
    throw error;
  } finally {
    await context.close();
    await browser.close();
  }
});