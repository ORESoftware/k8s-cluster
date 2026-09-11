namespace DdWebScraper;

enum WaitUntil {
  load: "load",
  domcontentloaded: "domcontentloaded",
  networkidle: "networkidle",
}

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

model FallbackProvenance {
  provider: string;
  actorId: string;
  trigger: string;
  localStatusCode: integer;
  localStrategy?: string;
  durationMs: integer;
  localDurationMs?: integer;
  totalDurationMs?: integer;
}
