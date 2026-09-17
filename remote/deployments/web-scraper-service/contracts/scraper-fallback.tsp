import "@typespec/json-schema";
using TypeSpec.JsonSchema;

namespace DdWebScraper;

@id("WaitUntil")
enum WaitUntil {
  load: "load",
  domcontentloaded: "domcontentloaded",
  networkidle: "networkidle",
}

@id("ScrapeFallbackRequest")
model ScrapeFallbackRequest {
  requestId?: string;
  url: string;
  strategy?: string;
  selector?: string;
  includeHtml?: boolean;
  includeText?: boolean;
  includeLinks?: boolean;
  includeContacts?: boolean;
  includePhones?: boolean;
  includeEmails?: boolean;
  contactRegion?: string;
  maxPhones?: integer;
  maxEmails?: integer;
  maxHtmlChars?: integer;
  maxTextChars?: integer;
  timeoutMs?: integer;
  waitUntil?: WaitUntil;
  respectRobots?: boolean;
}

@id("FallbackAttemptOutcome")
enum FallbackAttemptOutcome {
  success: "success",
  error: "error",
}

@id("FallbackAttemptErrorClass")
enum FallbackAttemptErrorClass {
  timeout: "timeout",
  rateLimit: "rate-limit",
  providerServerError: "provider-server-error",
  network: "network",
  emptyResult: "empty-result",
  invalidResponse: "invalid-response",
  responseTooLarge: "response-too-large",
  providerAuth: "provider-auth",
  invalidRequest: "invalid-request",
  unknown: "unknown",
}

@id("FallbackAttempt")
model FallbackAttempt {
  actorId: string;
  outcome: FallbackAttemptOutcome;
  durationMs: integer;
  errorClass?: FallbackAttemptErrorClass;
}

@id("FallbackProvenance")
model FallbackProvenance {
  provider: string;
  actorId: string;
  trigger: string;
  localStatusCode: integer;
  localStrategy?: string;
  durationMs: integer;
  localDurationMs?: integer;
  totalDurationMs?: integer;
  route?: string;
  attempts?: FallbackAttempt[];
  maxChainAttempts?: integer;
  maxChainChargeUsd?: float64;
}
