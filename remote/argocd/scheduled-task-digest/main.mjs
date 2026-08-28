#!/usr/bin/env node

import { randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';

import {
  CONFIG,
  boundedError,
  fetchJson,
  redact,
} from './shared.mjs';
import {
  collectDigest,
  logicalDateAt,
  renderDigest,
} from './digest.mjs';
import {
  kubernetesRequest,
  serviceAccountToken,
} from './kubernetes.mjs';

function leaseAnnotations(logicalDate, state, attemptId, now, extra = {}) {
  return {
    'dd.dev/logical-date': logicalDate,
    'dd.dev/state': state,
    'dd.dev/attempt-id': attemptId,
    'dd.dev/updated-at': now.toISOString(),
    ...extra,
  };
}

export function deliveryDecision(lease, logicalDate, force = false) {
  if (force) return { action: 'send', reason: 'manual canary bypass' };
  const annotations = lease?.metadata?.annotations || {};
  if (annotations['dd.dev/logical-date'] === logicalDate && ['claimed', 'sent'].includes(annotations['dd.dev/state'])) {
    return { action: 'suppress', reason: annotations['dd.dev/state'] };
  }
  return { action: 'claim', reason: annotations['dd.dev/state'] || 'unclaimed' };
}

async function getLease({ fetchImpl = globalThis.fetch, token = null } = {}) {
  const namespace = encodeURIComponent(CONFIG.leaseNamespace);
  const name = encodeURIComponent(CONFIG.leaseName);
  return kubernetesRequest(`/apis/coordination.k8s.io/v1/namespaces/${namespace}/leases/${name}`, {
    expectedStatuses: [200, 404],
    fetchImpl,
    token,
  });
}

export async function claimDelivery(logicalDate, now, { fetchImpl = globalThis.fetch } = {}) {
  const token = await serviceAccountToken();
  const attemptId = randomUUID();
  const namespace = encodeURIComponent(CONFIG.leaseNamespace);
  const name = encodeURIComponent(CONFIG.leaseName);
  for (let attempt = 1; attempt <= 4; attempt += 1) {
    const current = await getLease({ fetchImpl, token });
    const decision = deliveryDecision(current.status === 200 ? current.json : null, logicalDate, false);
    if (decision.action === 'suppress') return { status: 'duplicate_suppressed', reason: decision.reason, attemptId: null };
    const annotations = leaseAnnotations(logicalDate, 'claimed', attemptId, now, {
      'dd.dev/holder': redact(process.env.POD_NAME || 'scheduled-task-digest', 180),
    });
    try {
      if (current.status === 404) {
        await kubernetesRequest(`/apis/coordination.k8s.io/v1/namespaces/${namespace}/leases`, {
          method: 'POST',
          body: {
            apiVersion: 'coordination.k8s.io/v1',
            kind: 'Lease',
            metadata: { name: CONFIG.leaseName, namespace: CONFIG.leaseNamespace, annotations },
            spec: { holderIdentity: process.env.POD_NAME || 'scheduled-task-digest', leaseDurationSeconds: 172800, renewTime: now.toISOString() },
          },
          expectedStatuses: [201],
          fetchImpl,
          token,
        });
      } else {
        await kubernetesRequest(`/apis/coordination.k8s.io/v1/namespaces/${namespace}/leases/${name}`, {
          method: 'PATCH',
          body: {
            metadata: { resourceVersion: current.json.metadata?.resourceVersion, annotations },
            spec: { holderIdentity: process.env.POD_NAME || 'scheduled-task-digest', leaseDurationSeconds: 172800, renewTime: now.toISOString() },
          },
          expectedStatuses: [200],
          fetchImpl,
          token,
        });
      }
      return { status: 'claimed', reason: decision.reason, attemptId };
    } catch (error) {
      if (attempt === 4 || !String(error.message).includes('HTTP 409')) throw error;
    }
  }
  throw new Error('Delivery claim did not converge.');
}

export async function markDelivery(logicalDate, attemptId, state, now, { fetchImpl = globalThis.fetch, subject = null } = {}) {
  if (!attemptId) return;
  const token = await serviceAccountToken();
  const current = await getLease({ fetchImpl, token });
  if (current.status !== 200) throw new Error('Delivery Lease disappeared before finalization.');
  const annotations = current.json.metadata?.annotations || {};
  if (annotations['dd.dev/logical-date'] !== logicalDate || annotations['dd.dev/attempt-id'] !== attemptId) {
    throw new Error('Delivery Lease ownership changed before finalization.');
  }
  const namespace = encodeURIComponent(CONFIG.leaseNamespace);
  const name = encodeURIComponent(CONFIG.leaseName);
  await kubernetesRequest(`/apis/coordination.k8s.io/v1/namespaces/${namespace}/leases/${name}`, {
    method: 'PATCH',
    body: {
      metadata: {
        resourceVersion: current.json.metadata?.resourceVersion,
        annotations: leaseAnnotations(logicalDate, state, attemptId, now, subject ? { 'dd.dev/subject': redact(subject, 220) } : {}),
      },
      spec: { holderIdentity: process.env.POD_NAME || 'scheduled-task-digest', leaseDurationSeconds: 172800, renewTime: now.toISOString() },
    },
    expectedStatuses: [200],
    fetchImpl,
    token,
  });
}

export async function verifyEmailService({ fetchImpl = globalThis.fetch } = {}) {
  const base = new URL(CONFIG.emailServiceUrl);
  const response = await fetchJson(new URL('/readyz', base).toString(), {
    allowedOrigins: [base.origin],
    fetchImpl,
  });
  if (response.json?.email?.sendgrid_configured !== true) {
    throw new Error('Internal email service is reachable, but SendGrid mail delivery is not configured.');
  }
  return { ok: true, from: redact(response.json.email.from || '', 320) };
}

export async function sendDigestEmail(rendered, logicalDate, { fetchImpl = globalThis.fetch } = {}) {
  const auth = String(process.env.SERVER_AUTH_SECRET || '').trim();
  if (!auth) throw new Error('SERVER_AUTH_SECRET is required for the internal email service.');
  const base = new URL(CONFIG.emailServiceUrl);
  const response = await fetchJson(new URL('/send/email', base).toString(), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'x-server-auth': auth },
    body: {
      to: CONFIG.recipient,
      subject: rendered.subject,
      html: rendered.html,
      text: rendered.text,
      idempotency_key: `scheduled-task-digest-${logicalDate.replaceAll('-', '')}`,
    },
    expectedStatuses: [200],
    allowedOrigins: [base.origin],
    fetchImpl,
  });
  if (response.json?.ok !== true) throw new Error('Internal email service did not confirm delivery.');
  return { ok: true, transport: redact(response.json.transport || 'unknown', 80), upstreamStatus: response.json.upstreamStatus || null };
}

export async function runDigest({ now = new Date(), fetchImpl = globalThis.fetch } = {}) {
  if (!(now instanceof Date) || Number.isNaN(now.getTime())) throw new Error('A valid execution instant is required.');
  const dryRun = String(process.env.DRY_RUN || '').toLowerCase() === '1' || String(process.env.DRY_RUN || '').toLowerCase() === 'true';
  const force = String(process.env.FORCE_SEND || '').toLowerCase() === '1' || String(process.env.FORCE_SEND || '').toLowerCase() === 'true';
  const mode = force ? 'manual-canary' : (process.env.RUN_MODE || 'scheduled');
  const logicalDate = logicalDateAt(now);
  let claim = { status: 'dry_run', attemptId: null };
  let sendStarted = false;

  if (!dryRun && !force) {
    claim = await claimDelivery(logicalDate, now, { fetchImpl });
    if (claim.status === 'duplicate_suppressed') {
      return { status: 'duplicate_suppressed', logicalDate, reason: claim.reason };
    }
  }

  try {
    const emailService = await verifyEmailService({ fetchImpl });
    const report = await collectDigest(now, { fetchImpl });
    const rendered = renderDigest(report, mode);
    if (dryRun) {
      return { status: 'dry_run_ok', logicalDate, subject: rendered.subject, summary: rendered.summary, emailService };
    }
    sendStarted = true;
    const delivery = await sendDigestEmail(rendered, logicalDate, { fetchImpl });
    if (!force) await markDelivery(logicalDate, claim.attemptId, 'sent', new Date(), { fetchImpl, subject: rendered.subject });
    return { status: force ? 'manual_canary_sent' : 'sent', logicalDate, subject: rendered.subject, summary: rendered.summary, delivery };
  } catch (error) {
    if (!dryRun && !force && claim.attemptId && !sendStarted) {
      try {
        await markDelivery(logicalDate, claim.attemptId, 'failed', new Date(), { fetchImpl });
      } catch (markError) {
        console.error(JSON.stringify({ level: 'error', event: 'delivery_mark_failed_error', error: boundedError(markError) }));
      }
    }
    throw error;
  }
}

export async function main() {
  try {
    const result = await runDigest();
    console.log(JSON.stringify({ level: 'info', event: 'scheduled_task_digest_complete', ...result }));
  } catch (error) {
    console.error(JSON.stringify({ level: 'error', event: 'scheduled_task_digest_failed', error: boundedError(error) }));
    process.exitCode = 1;
  }
}

const invokedPath = process.argv[1] ? pathToFileURL(process.argv[1]).href : null;
if (invokedPath === import.meta.url) await main();
