import {
  PROTOCOL_VERSION,
  type ChangesResponse,
  type MutationRequest,
  type MutationResponse,
  type OutboxMutation,
} from "./types";

export type AccessTokenProvider = () => Promise<string | null>;

export interface SyncTransportOptions {
  baseUrl?: string;
  fetch?: typeof fetch;
  getAccessToken?: AccessTokenProvider;
}

function mutationRequest(clientId: string, operations: readonly OutboxMutation[]): MutationRequest {
  return {
    protocolVersion: PROTOCOL_VERSION,
    clientId,
    operations: operations.map((operation) => ({
      mutationId: operation.mutationId,
      key: operation.key,
      action: operation.action,
      baseVersion: operation.baseVersion,
      schemaVersion: operation.schemaVersion,
      ...(operation.value === undefined ? {} : { value: operation.value }),
    })),
  };
}

export function encodedMutationRequestBytes(
  clientId: string,
  operations: readonly OutboxMutation[],
): number {
  return new TextEncoder().encode(JSON.stringify(mutationRequest(clientId, operations))).byteLength;
}

export class SyncHttpError extends Error {
  readonly status: number;
  readonly code: string | null;
  readonly retryAfterMs: number | null;

  constructor(
    message: string,
    status: number,
    code: string | null = null,
    retryAfterMs: number | null = null,
  ) {
    super(message);
    this.name = "SyncHttpError";
    this.status = status;
    this.code = code;
    this.retryAfterMs = retryAfterMs;
  }

  get retryable(): boolean {
    return this.status === 408 || this.status === 425 || this.status === 429 || this.status >= 500;
  }
}

function parseRetryAfter(value: string | null, now = Date.now()): number | null {
  if (value === null) {
    return null;
  }
  const seconds = Number(value);
  if (Number.isFinite(seconds) && seconds >= 0) {
    return Math.floor(seconds * 1_000);
  }
  const date = Date.parse(value);
  return Number.isNaN(date) ? null : Math.max(0, date - now);
}

function withoutTrailingSlash(value: string): string {
  return value.endsWith("/") ? value.slice(0, -1) : value;
}

export class SyncTransport {
  private readonly baseUrl: string;
  private readonly fetchImplementation: typeof fetch;
  private readonly getAccessToken: AccessTokenProvider | undefined;

  constructor(options: SyncTransportOptions = {}) {
    this.baseUrl = withoutTrailingSlash(options.baseUrl ?? "");
    this.fetchImplementation = options.fetch ?? globalThis.fetch.bind(globalThis);
    this.getAccessToken = options.getAccessToken;
  }

  async getChanges(
    cursor: string | null,
    limit: number,
    signal?: AbortSignal,
  ): Promise<ChangesResponse> {
    const query = new URLSearchParams({ limit: String(limit) });
    if (cursor !== null) {
      query.set("cursor", cursor);
    }
    const response = await this.request(`${this.baseUrl}/api/v1/sync/changes?${query.toString()}`, {
      method: "GET",
      ...(signal === undefined ? {} : { signal }),
    });
    const body: unknown = await response.json();
    if (typeof body !== "object" || body === null) {
      throw new TypeError("changes response must be an object");
    }
    const candidate = body as Partial<ChangesResponse>;
    if (!Array.isArray(candidate.changes) || typeof candidate.nextCursor !== "string") {
      throw new TypeError("changes response is missing changes or nextCursor");
    }
    if (typeof candidate.caughtUp !== "boolean") {
      throw new TypeError("changes response is missing caughtUp");
    }
    return candidate as ChangesResponse;
  }

  async pushMutations(
    clientId: string,
    operations: readonly OutboxMutation[],
    signal?: AbortSignal,
  ): Promise<MutationResponse> {
    const body = mutationRequest(clientId, operations);
    const response = await this.request(`${this.baseUrl}/api/v1/sync/mutations`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
      ...(signal === undefined ? {} : { signal }),
    });
    const responseBody: unknown = await response.json();
    if (typeof responseBody !== "object" || responseBody === null) {
      throw new TypeError("mutation response must be an object");
    }
    const candidate = responseBody as Partial<MutationResponse>;
    if (!Array.isArray(candidate.results)) {
      throw new TypeError("mutation response is missing results");
    }
    return candidate as MutationResponse;
  }

  private async request(input: string, init: RequestInit): Promise<Response> {
    const headers = new Headers(init.headers);
    headers.set("accept", "application/json");
    const csrfToken =
      typeof document === "undefined"
        ? null
        : document.querySelector<HTMLMetaElement>('meta[name="csrf-token"]')?.content;
    if (csrfToken !== undefined && csrfToken !== null && csrfToken.length > 0) {
      headers.set("x-csrf-token", csrfToken);
    }
    const token = await this.getAccessToken?.();
    if (token !== undefined && token !== null) {
      headers.set("authorization", `Bearer ${token}`);
    }
    const response = await this.fetchImplementation(input, {
      ...init,
      headers,
      credentials: "same-origin",
    });
    if (!response.ok) {
      let detail = `sync request failed with HTTP ${response.status}`;
      let code: string | null = null;
      try {
        const body: unknown = await response.clone().json();
        if (typeof body === "object" && body !== null && "error" in body) {
          const error = (body as { error?: unknown }).error;
          if (typeof error === "string") {
            detail = error;
          } else if (typeof error === "object" && error !== null) {
            const structured = error as { code?: unknown; message?: unknown };
            if (typeof structured.code === "string") {
              code = structured.code;
            }
            if (typeof structured.message === "string") {
              detail = structured.message;
            }
          }
        }
      } catch {
        // The status and generic detail are sufficient when the body is not JSON.
      }
      throw new SyncHttpError(
        detail,
        response.status,
        code,
        parseRetryAfter(response.headers.get("retry-after")),
      );
    }
    return response;
  }
}
