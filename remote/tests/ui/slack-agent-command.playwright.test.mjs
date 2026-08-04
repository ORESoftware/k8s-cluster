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
const outputDir =
  process.env.PLAYWRIGHT_OUTPUT_DIR ?? path.resolve('test-results/slack-agent-command');

const aliasMatrix = Object.freeze([
  {
    command: '/ores-claude',
    endpoint: '/slack/commands/ores-claude',
    provider: 'claude',
    providerLabel: 'Claude',
    observableProvider: 'anthropic',
    agent: 'claude-fable-5',
  },
  {
    command: '/x-claude',
    endpoint: '/slack/commands/ores-claude',
    provider: 'claude',
    providerLabel: 'Claude',
    observableProvider: 'anthropic',
    agent: 'claude-fable-5',
  },
  {
    command: '/my-claude',
    endpoint: '/slack/commands/ores-claude',
    provider: 'claude',
    providerLabel: 'Claude',
    observableProvider: 'anthropic',
    agent: 'claude-fable-5',
  },
  {
    command: '/ores-chatgpt',
    endpoint: '/slack/commands/ores-chatgpt',
    provider: 'chatgpt',
    providerLabel: 'ChatGPT',
    observableProvider: 'openai',
    agent: 'gpt-5.6-sol',
  },
  {
    command: '/x-chatgpt',
    endpoint: '/slack/commands/ores-chatgpt',
    provider: 'chatgpt',
    providerLabel: 'ChatGPT',
    observableProvider: 'openai',
    agent: 'gpt-5.6-sol',
  },
  {
    command: '/my-chatgpt',
    endpoint: '/slack/commands/ores-chatgpt',
    provider: 'chatgpt',
    providerLabel: 'ChatGPT',
    observableProvider: 'openai',
    agent: 'gpt-5.6-sol',
  },
]);

function alias(command) {
  const entry = aliasMatrix.find((candidate) => candidate.command === command);
  assert.ok(entry, `unknown test alias ${command}`);
  return entry;
}

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
  const deadline = Date.now() + 30_000;
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
    expectedObservableProvider,
    expectedAgent,
    expectedRepository,
    expectedIssue,
    expectedAction,
    expectedPrompt,
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
  const observableEvent = payload.observable_event;
  const normalizedRepository = expectedRepository.toLowerCase();

  assert.equal(job.task_type, 'slack_agent_run');
  assert.equal(payload.run_id, ids.run);
  assert.equal(payload.provider, expectedProvider);
  assert.equal(payload.action, expectedAction);
  assert.equal(payload.prompt, expectedPrompt);
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

  assert.equal(observableEvent.schema_version, '1.0');
  assert.equal(observableEvent.kind, 'task_created');
  assert.equal(observableEvent.payload_classification, 'internal');
  assert.equal(observableEvent.redaction_state, 'sanitized');
  assert.equal(observableEvent.idempotency_key, `slack-task-created:${ids.run}`);
  assert.equal(observableEvent.correlation.run_id, ids.run);
  assert.equal(observableEvent.correlation.session_id, ids.run);
  assert.equal(observableEvent.correlation.task_id, ids.run);
  assert.equal(observableEvent.source.agent_id, expectedAgent);
  assert.equal(observableEvent.source.model, expectedAgent);
  assert.equal(observableEvent.source.provider, expectedObservableProvider);
  assert.equal(observableEvent.payload.repository.toLowerCase(), normalizedRepository);
  assert.equal(observableEvent.payload.bridge_workflow_id, ids.workflow);
  assert.equal(observableEvent.payload.linear_issue, expectedIssue);
  assert.equal(observableEvent.delivery.ack_requested, true);
  assert.equal(observableEvent.delivery.attempt, 1);

  const encodedObservableEvent = JSON.stringify(observableEvent);
  for (const forbidden of [
    expectedPrompt,
    slackTeamId,
    slackChannelId,
    slackUserId,
    'message-2',
    'message-6',
  ]) {
    assert.equal(
      encodedObservableEvent.includes(forbidden),
      false,
      `observable event leaked private Slack input: ${forbidden}`,
    );
  }

  assert.equal(bridge.workflow.plan.assignments.length, 1);
  assert.equal(bridge.workflow.plan.assignments[0].agent_key, expectedAgent);
  assert.equal(bridge.workflow.plan.meta.repository.toLowerCase(), normalizedRepository);
  assert.equal(
    Object.hasOwn(bridge.workflow.plan, 'file_lease'),
    false,
    'Slack repository metadata must not opt the workflow into file leases',
  );

  const duplicateRequest = {
    org: job.org,
    repo: job.repo,
    task_type: job.task_type,
    payload: job.payload,
    priority: job.priority,
    max_attempts: job.max_attempts,
    budget_usd: job.budget_usd,
  };
  const prefixed = await request.post(`${coordinatorBase}/v1/jobs`, {
    headers: {
      authorization: `Bearer ${coordinatorBearer}`,
      'idempotency-key': `slack-command:${ids.run}`,
    },
    data: duplicateRequest,
  });
  assert.equal(
    prefixed.status(),
    400,
    `prefixed Slack idempotency key was not rejected: ${await prefixed.text()}`,
  );

  const duplicate = await request.post(`${coordinatorBase}/v1/jobs`, {
    headers: {
      authorization: `Bearer ${coordinatorBearer}`,
      'idempotency-key': ids.run,
    },
    data: duplicateRequest,
  });
  assert.equal(duplicate.status(), 202, await duplicate.text());
  assert.equal((await duplicate.json()).job.id, ids.job, 'coordinator idempotency created a second job');
  return ids;
}

test('all six Slack slash-command aliases traverse browser, modal, bridge, coordinator, PostgreSQL, and Slack contracts', async () => {
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
    await page
      .getByRole('heading', { name: 'ORESoftware Slack command integration dashboard' })
      .waitFor();
    assert.equal(await page.getByTestId('modal-count').innerText(), '0');
    assert.equal(await page.getByTestId('message-count').innerText(), '0');

    const wrongEndpointBody = encodedCommand({
      command: '/x-chatgpt',
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
      command: '/my-chatgpt',
      appId: 'A0WRONGAPP',
      text: 'Implement DEN-1041',
      trigger: 'trigger-wrong-app',
    });
    const wrongApp = await postSigned(
      request,
      '/slack/commands/ores-chatgpt',
      wrongAppBody,
    );
    assert.equal(wrongApp.status(), 403, await wrongApp.text());

    const wrongTeamBody = encodedCommand({
      command: '/x-claude',
      team: 'T0WRONGTEAM',
      text: 'Implement DEN-1041',
      trigger: 'trigger-wrong-team',
    });
    const wrongTeam = await postSigned(
      request,
      '/slack/commands/ores-claude',
      wrongTeamBody,
    );
    assert.equal(wrongTeam.status(), 403, await wrongTeam.text());

    let state = await mockState(request);
    assert.equal(state.views.length, 0);
    assert.equal(state.messages.length, 0);
    assert.equal(state.historyCalls, 0);

    const modalAlias = alias('/my-claude');
    const modalBody = encodedCommand({
      command: modalAlias.command,
      trigger: 'trigger-my-claude-modal',
    });
    const modalResponse = await postSigned(request, modalAlias.endpoint, modalBody);
    assert.equal(modalResponse.status(), 200, await modalResponse.text());
    state = await waitFor(
      request,
      (candidate) => candidate.views.length === 1,
      `${modalAlias.command} modal was not opened`,
    );

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
    assert.deepEqual(JSON.parse(privateMetadata), {
      provider: 'claude',
      team_id: slackTeamId,
      channel_id: slackChannelId,
      user_id: slackUserId,
    });

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

    const modalPrompt = 'Investigate DEN-1231 with tests through /my-claude';
    const interactionBody = encodedInteraction({
      api_app_id: slackAppId,
      type: 'view_submission',
      team: { id: slackTeamId },
      user: { id: slackUserId },
      view: {
        id: 'V-my-claude-modal-submit-1',
        callback_id: 'ores-agent-run-v1',
        private_metadata: privateMetadata,
        state: {
          values: {
            task: { task: { value: modalPrompt } },
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
      `${modalAlias.command} modal dispatch status was not posted`,
    );

    const dispatches = new Map();
    dispatches.set(
      modalAlias.command,
      await verifyDispatch(request, state.messages[0].text, {
        expectedProvider: modalAlias.provider,
        expectedObservableProvider: modalAlias.observableProvider,
        expectedAgent: modalAlias.agent,
        expectedRepository: 'ORESoftware/ai-agent-coordinator.rs',
        expectedIssue: 'DEN-1231',
        expectedAction: 'investigate',
        expectedPrompt: modalPrompt,
      }),
    );

    const directAliases = aliasMatrix.filter((entry) => entry.command !== modalAlias.command);
    let replay;
    for (const [index, entry] of directAliases.entries()) {
      const issue = 'DEN-1041';
      const prompt = `Implement ${issue} with tests through ${entry.command}`;
      const body = encodedCommand({
        command: entry.command,
        text: prompt,
        trigger: `trigger-alias-${index}-${entry.command.slice(1)}`,
      });
      const response = await postSigned(request, entry.endpoint, body);
      assert.equal(response.status(), 200, `${entry.command}: ${await response.text()}`);
      assert.match((await response.json()).text, new RegExp(`Accepted ${entry.providerLabel} run`));

      const expectedMessageCount = index + 2;
      state = await waitFor(
        request,
        (candidate) => candidate.messages.length === expectedMessageCount,
        `${entry.command} dispatch status was not posted`,
      );
      dispatches.set(
        entry.command,
        await verifyDispatch(request, state.messages[expectedMessageCount - 1].text, {
          expectedProvider: entry.provider,
          expectedObservableProvider: entry.observableProvider,
          expectedAgent: entry.agent,
          expectedRepository: 'ORESoftware/ai-agent-bridge.rs',
          expectedIssue: issue,
          expectedAction: 'implement',
          expectedPrompt: prompt,
        }),
      );
      if (entry.command === '/my-chatgpt') replay = { entry, body };
    }

    assert.deepEqual([...dispatches.keys()].sort(), aliasMatrix.map((entry) => entry.command).sort());
    assert.equal(
      new Set([...dispatches.values()].map((ids) => ids.run)).size,
      aliasMatrix.length,
      'each alias must create a distinct deterministic run for its unique Slack trigger',
    );
    assert.ok(replay, 'the /my-chatgpt replay fixture was not captured');

    const duplicateResponse = await postSigned(request, replay.entry.endpoint, replay.body);
    assert.equal(duplicateResponse.status(), 200, await duplicateResponse.text());
    assert.match((await duplicateResponse.json()).text, /already accepted/);
    await new Promise((resolve) => setTimeout(resolve, 250));
    state = await mockState(request);
    assert.equal(
      state.messages.length,
      aliasMatrix.length,
      'duplicate Slack request posted a second status',
    );

    const unauthorizedBody = encodedCommand({
      command: '/x-chatgpt',
      channel: 'C0UNAUTHORIZED',
      text: 'Implement DEN-1041',
      trigger: 'trigger-unauthorized',
    });
    const unauthorized = await postSigned(
      request,
      '/slack/commands/ores-chatgpt',
      unauthorizedBody,
    );
    assert.equal(unauthorized.status(), 403, await unauthorized.text());

    const staleBody = encodedCommand({
      command: '/ores-claude',
      text: 'Implement DEN-1041',
      trigger: 'trigger-stale',
    });
    const stale = await postSigned(
      request,
      '/slack/commands/ores-claude',
      staleBody,
      Math.floor(Date.now() / 1000) - 600,
    );
    assert.equal(stale.status(), 401, await stale.text());

    state = await mockState(request);
    assert.equal(state.messages.length, aliasMatrix.length);
    assert.equal(state.views.length, 1);
    assert.equal(
      state.historyCalls,
      aliasMatrix.length,
      'one bounded history read is performed per live alias dispatch',
    );

    await page.goto(`${slackMockBase}/`);
    assert.equal(await page.getByTestId('message-count').innerText(), String(aliasMatrix.length));
    assert.equal(await page.getByTestId('modal-count').innerText(), '1');
    await page.screenshot({
      path: path.join(outputDir, 'slack-agent-command-six-alias-dashboard.png'),
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