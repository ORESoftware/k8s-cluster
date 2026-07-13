import { isIP } from 'node:net';

export type ProxyRotation = 'round-robin' | 'random' | 'sticky';

export const PROXY_ROTATIONS: readonly ProxyRotation[] = [
  'round-robin',
  'random',
  'sticky',
] as const;

const SUPPORTED_PROXY_PROTOCOLS = new Set(['http:', 'https:', 'socks4:', 'socks5:', 'socks:']);

export type ProxyEntry = {
  /** Full proxy URL including credentials, used to configure the agent. */
  readonly url: string;
  /** `protocol//host:port` with credentials stripped; safe for logs and metrics. */
  readonly label: string;
  readonly protocol: string;
  readonly hostname: string;
  readonly port: string;
  readonly username: string;
  readonly password: string;
  /** True when the proxy speaks SOCKS rather than HTTP CONNECT. */
  readonly isSocks: boolean;
};

export function parseProxyEntry(raw: string): ProxyEntry {
  const trimmed = raw.trim();
  if (!trimmed) {
    throw new Error('empty proxy entry');
  }
  // Allow bare `host:port` and `user:pass@host:port` by assuming http.
  const candidate = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed) ? trimmed : `http://${trimmed}`;
  let url: URL;
  try {
    url = new URL(candidate);
  } catch {
    throw new Error(`invalid proxy URL: ${trimmed}`);
  }
  if (!SUPPORTED_PROXY_PROTOCOLS.has(url.protocol)) {
    throw new Error(`unsupported proxy protocol ${url.protocol} in ${trimmed}`);
  }
  if (!url.hostname) {
    throw new Error(`proxy URL is missing a host: ${trimmed}`);
  }
  return {
    url: url.toString(),
    label: `${url.protocol}//${url.host}`,
    protocol: url.protocol,
    hostname: stripIpv6Brackets(url.hostname.toLowerCase()),
    port: url.port,
    username: decodeURIComponent(url.username),
    password: decodeURIComponent(url.password),
    isSocks: url.protocol.startsWith('socks'),
  };
}

export function parseProxyList(raw: string | null | undefined): ProxyEntry[] {
  if (!raw) {
    return [];
  }
  const seen = new Set<string>();
  const entries: ProxyEntry[] = [];
  for (const token of raw.split(/[\s,]+/)) {
    if (!token) {
      continue;
    }
    const entry = parseProxyEntry(token);
    if (seen.has(entry.url)) {
      continue;
    }
    seen.add(entry.url);
    entries.push(entry);
  }
  return entries;
}

type PoolMember = {
  readonly entry: ProxyEntry;
  cooldownUntil: number;
  failures: number;
  successes: number;
};

export type ProxyPoolStats = {
  size: number;
  available: number;
  cooling: number;
  rotation: ProxyRotation;
  selections: number;
  failures: number;
};

/**
 * Rotates outbound requests across a fixed set of proxies. Failures put a proxy
 * on a short cooldown so a dead upstream stops being handed out; `sticky` keeps
 * the same proxy per key (typically the target host) to preserve sessions.
 */
export class ProxyPool {
  private readonly members: PoolMember[];
  private cursor = 0;
  private readonly stickyByKey = new Map<string, string>();
  private selections = 0;
  private failureReports = 0;

  constructor(
    proxies: ProxyEntry[],
    private readonly rotation: ProxyRotation,
    private readonly cooldownMs: number,
  ) {
    this.members = proxies.map((entry) => ({
      entry,
      cooldownUntil: 0,
      failures: 0,
      successes: 0,
    }));
  }

  get size(): number {
    return this.members.length;
  }

  get isEmpty(): boolean {
    return this.members.length === 0;
  }

  /**
   * Pick a proxy. `key` is only consulted for the `sticky` rotation; pass the
   * target hostname so one host keeps the same egress IP across requests.
   */
  select(key?: string): ProxyEntry | null {
    if (this.members.length === 0) {
      return null;
    }
    const now = Date.now();
    const member = this.pickMember(now, key);
    if (!member) {
      return null;
    }
    this.selections += 1;
    return member.entry;
  }

  reportSuccess(entry: ProxyEntry): void {
    const member = this.find(entry);
    if (member) {
      member.successes += 1;
      member.cooldownUntil = 0;
    }
  }

  reportFailure(entry: ProxyEntry): void {
    const member = this.find(entry);
    if (!member) {
      return;
    }
    this.failureReports += 1;
    member.failures += 1;
    member.cooldownUntil = Date.now() + this.cooldownMs;
    // Drop sticky bindings that point at a now-cooling proxy.
    for (const [key, label] of this.stickyByKey) {
      if (label === entry.label) {
        this.stickyByKey.delete(key);
      }
    }
  }

  stats(): ProxyPoolStats {
    const now = Date.now();
    const cooling = this.members.filter((member) => member.cooldownUntil > now).length;
    return {
      size: this.members.length,
      available: this.members.length - cooling,
      cooling,
      rotation: this.rotation,
      selections: this.selections,
      failures: this.failureReports,
    };
  }

  private pickMember(now: number, key?: string): PoolMember | null {
    if (this.rotation === 'sticky' && key) {
      const boundLabel = this.stickyByKey.get(key);
      const bound = boundLabel ? this.members.find((m) => m.entry.label === boundLabel) : undefined;
      if (bound && bound.cooldownUntil <= now) {
        return bound;
      }
      const fresh = this.pickFresh(now);
      if (fresh) {
        this.stickyByKey.set(key, fresh.entry.label);
      }
      return fresh;
    }
    return this.pickFresh(now);
  }

  private pickFresh(now: number): PoolMember | null {
    const available = this.members.filter((member) => member.cooldownUntil <= now);
    // If every proxy is cooling down, fall back to the whole set rather than
    // failing the request outright — a degraded proxy beats a dropped scrape.
    const pool = available.length > 0 ? available : this.members;
    if (this.rotation === 'random') {
      return pool[Math.floor(Math.random() * pool.length)] ?? null;
    }
    // round-robin (and sticky's initial pick) walk the cursor over all members.
    for (let i = 0; i < this.members.length; i += 1) {
      const member = this.members[(this.cursor + i) % this.members.length];
      if (member && member.cooldownUntil <= now) {
        this.cursor = (this.cursor + i + 1) % this.members.length;
        return member;
      }
    }
    const member = this.members[this.cursor % this.members.length] ?? null;
    this.cursor = (this.cursor + 1) % this.members.length;
    return member;
  }

  private find(entry: ProxyEntry): PoolMember | undefined {
    return this.members.find((member) => member.entry.label === entry.label);
  }
}

function stripIpv6Brackets(hostname: string): string {
  return hostname.replace(/^\[/, '').replace(/\]$/, '');
}

/** Re-exported so the server can reuse the same private-IP guard for proxies. */
export function proxyTargetsIpLiteral(entry: ProxyEntry): boolean {
  return isIP(entry.hostname) !== 0;
}
