import process from 'node:process';
import { config } from './runtime.mjs';
import {
  conversationIdentity,
  isRateLimitErrorText,
  normalizeText,
} from './policy.mjs';

async function firstVisible(page, selectors) {
  for (const selector of selectors) {
    const locator = page.locator(selector);
    const count = Math.min(await locator.count(), 8);
    for (let index = 0; index < count; index += 1) {
      const candidate = locator.nth(index);
      if (await candidate.isVisible().catch(() => false)) return candidate;
    }
  }
  return null;
}

export async function visibleComposer(page) {
  return firstVisible(page, [
    'textarea#prompt-textarea',
    '#prompt-textarea[contenteditable="true"]',
    'textarea[data-id="root"]',
    '[contenteditable="true"][data-lexical-editor="true"]',
    'form textarea',
    'form [contenteditable="true"]',
  ]);
}

async function isGenerationInProgress(page) {
  return Boolean(
    await firstVisible(page, [
      'button[data-testid="stop-button"]',
      'button[aria-label*="Stop" i]',
      'button:has-text("Stop generating")',
    ]),
  );
}

async function detectAuthOrChallenge(page) {
  const current = new URL(page.url());
  if (current.hostname !== 'chatgpt.com' || /\/(?:auth|login)(?:\/|$)/i.test(current.pathname)) {
    return 'login_redirect';
  }

  if (
    await firstVisible(page, [
      'button:has-text("Log in")',
      'a:has-text("Log in")',
      'button:has-text("Sign up")',
      'a:has-text("Sign up")',
    ])
  ) {
    return 'login_required';
  }

  const challenge = await page
    .getByText(/verify you are human|checking your browser|just a moment/i)
    .first()
    .isVisible()
    .catch(() => false);
  if (challenge) return 'interactive_challenge';

  return null;
}

async function timeline(page) {
  return page
    .locator('[data-message-author-role], [role="alert"]')
    .evaluateAll((nodes) =>
      nodes
        .filter((node) => {
          const style = window.getComputedStyle(node);
          const rect = node.getBoundingClientRect();
          return style.visibility !== 'hidden' && style.display !== 'none' && rect.height > 0;
        })
        .map((node) => ({
          kind: node.hasAttribute('data-message-author-role') ? 'turn' : 'alert',
          role: node.getAttribute('data-message-author-role'),
          text: (node.innerText || node.textContent || '').slice(0, 4_000),
        }))
        .slice(-60),
    )
    .catch(() => []);
}

function rateLimitSignal(events) {
  let lastTurnIndex = -1;
  for (let index = events.length - 1; index >= 0; index -= 1) {
    if (events[index].kind === 'turn') {
      lastTurnIndex = index;
      break;
    }
  }

  const lastTurn = lastTurnIndex >= 0 ? events[lastTurnIndex] : null;
  const trailingEvents = events.slice(Math.max(0, lastTurnIndex));
  const trailingAlert = trailingEvents.find(
    (event) => event.kind === 'alert' && isRateLimitErrorText(event.text),
  );
  if (trailingAlert) return { blocked: true, reason: 'trailing_alert', lastTurn };

  if (
    lastTurn?.role === 'assistant' &&
    isRateLimitErrorText(lastTurn.text)
  ) {
    return { blocked: true, reason: 'assistant_error_turn', lastTurn };
  }

  return { blocked: false, reason: null, lastTurn };
}

export async function inspectConversation(page) {
  const authProblem = await detectAuthOrChallenge(page);
  if (authProblem) return { authProblem, blocked: false };

  const events = await timeline(page);
  const signal = rateLimitSignal(events);
  const composer = await visibleComposer(page);
  const inProgress = await isGenerationInProgress(page);
  const turnCount = events.filter((event) => event.kind === 'turn').length;
  const userTurnCount = events.filter(
    (event) => event.kind === 'turn' && event.role === 'user',
  ).length;
  const assistantTurnCount = events.filter(
    (event) => event.kind === 'turn' && event.role === 'assistant',
  ).length;

  return {
    authProblem: null,
    blocked: signal.blocked && Boolean(composer) && !inProgress,
    rateReason: signal.reason,
    lastRole: signal.lastTurn?.role ?? null,
    inProgress,
    hasComposer: Boolean(composer),
    turnCount,
    userTurnCount,
    assistantTurnCount,
    events,
  };
}

async function openSidebarIfNeeded(page) {
  const links = page.locator('a[href*="/c/"]');
  if ((await links.count()) > 0) return;

  const opener = await firstVisible(page, [
    'button[data-testid="open-sidebar-button"]',
    'button[aria-label*="Open sidebar" i]',
    'button[aria-label*="Sidebar" i]',
  ]);
  if (opener) {
    await opener.click();
    await page.waitForTimeout(1_000);
  }
}

async function conversationLinks(page) {
  const hrefs = await page
    .locator('a[href]')
    .evaluateAll((anchors) => anchors.map((anchor) => anchor.getAttribute('href')))
    .catch(() => []);

  const candidates = [];
  for (const href of hrefs) {
    const identity = conversationIdentity(href, config.baseUrl);
    if (identity) candidates.push(identity);
  }
  return candidates;
}

async function scrollConversationList(page) {
  const anchor = page.locator('a[href*="/c/"]').last();
  if ((await anchor.count()) === 0) return false;

  return anchor
    .evaluate((element) => {
      let current = element.parentElement;
      while (current && current !== document.body) {
        if (current.scrollHeight > current.clientHeight + 40) {
          const before = current.scrollTop;
          current.scrollTop = Math.min(current.scrollHeight, before + current.clientHeight * 0.9);
          current.dispatchEvent(new Event('scroll', { bubbles: true }));
          return current.scrollTop > before;
        }
        current = current.parentElement;
      }
      return false;
    })
    .catch(() => false);
}

export async function collectCandidates(page) {
  await page.goto(config.baseUrl, {
    waitUntil: 'domcontentloaded',
    timeout: config.pageTimeoutMs,
  });
  await page.waitForTimeout(3_000);

  const authProblem = await detectAuthOrChallenge(page);
  if (authProblem) {
    const error = new Error(`ChatGPT authentication is unavailable: ${authProblem}`);
    error.exitCode = 78;
    throw error;
  }

  await openSidebarIfNeeded(page);
  const byId = new Map();
  let unchangedPasses = 0;

  for (let pass = 0; pass < config.sidebarScrollPasses; pass += 1) {
    const before = byId.size;
    for (const candidate of await conversationLinks(page)) {
      if (!byId.has(candidate.id)) byId.set(candidate.id, candidate);
      if (byId.size >= config.maxChatScan) break;
    }
    if (byId.size >= config.maxChatScan) break;

    unchangedPasses = byId.size === before ? unchangedPasses + 1 : 0;
    if (unchangedPasses >= 2) break;
    if (!(await scrollConversationList(page))) break;
    await page.waitForTimeout(900);
  }

  if (byId.size === 0) {
    throw new Error('No ChatGPT conversation links were found; the sidebar DOM or auth state changed.');
  }

  return [...byId.values()].slice(0, config.maxChatScan);
}

export async function fillComposer(composer, value) {
  try {
    await composer.fill(value);
  } catch {
    await composer.click();
    await composer.press(process.platform === 'darwin' ? 'Meta+A' : 'Control+A');
    await composer.type(value, { delay: 1 });
  }
}

export async function clickSend(page, composer) {
  const sendButton = await firstVisible(page, [
    'button[data-testid="send-button"]',
    'button[aria-label*="Send" i]',
  ]);

  if (sendButton) {
    await sendButton.click();
    return 'button';
  }

  await composer.press('Enter');
  return 'enter';
}

export async function waitForSubmission(page, baselineUserTurns) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    const snapshot = await inspectConversation(page);
    if (snapshot.authProblem) throw new Error(`Authentication lost: ${snapshot.authProblem}`);
    if (snapshot.userTurnCount > baselineUserTurns) return snapshot;
    await page.waitForTimeout(500);
  }
  throw new Error('ChatGPT did not acknowledge the continuation prompt within 30 seconds.');
}

export async function waitForOutcome(page, baselineUserTurns) {
  const deadline = Date.now() + config.responseWaitMs;
  let sawNewUserTurn = false;

  while (Date.now() < deadline) {
    const snapshot = await inspectConversation(page);
    if (snapshot.authProblem) throw new Error(`Authentication lost: ${snapshot.authProblem}`);
    sawNewUserTurn ||= snapshot.userTurnCount > baselineUserTurns;

    if (sawNewUserTurn && snapshot.blocked) {
      return 'still_rate_limited';
    }

    const lastTurn = [...(snapshot.events ?? [])]
      .reverse()
      .find((event) => event.kind === 'turn');
    if (
      sawNewUserTurn &&
      !snapshot.inProgress &&
      lastTurn?.role === 'assistant' &&
      normalizeText(lastTurn.text) &&
      !isRateLimitErrorText(lastTurn.text)
    ) {
      return 'resumed';
    }

    await page.waitForTimeout(snapshot.inProgress ? 2_000 : 1_000);
  }

  const finalSnapshot = await inspectConversation(page);
  return finalSnapshot.inProgress ? 'submitted_in_progress' : 'response_timeout';
}
