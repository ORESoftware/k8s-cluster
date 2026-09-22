// Build-time validation for the handful of links that come from deployment
// configuration (store listings, desktop installer hosts, the tester folder)
// rather than from page source.
//
// Those values arrive as repository/environment variables, so they are a real
// injection surface: a mistyped or hostile value such as `javascript:alert(1)`
// would otherwise be emitted verbatim into an `href` and become an active URL
// on a site that ships zero client JavaScript. Every configured URL must parse
// as an absolute `https:` URL before it is rendered; anything else is dropped
// and the component keeps its "coming soon" fallback state.

/** Returns `value` when it is an absolute https URL, otherwise `undefined`. */
export function httpsUrl(value: string | undefined | null): string | undefined {
  const trimmed = value?.trim();
  if (!trimmed) return undefined;

  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return undefined;
  }

  return parsed.protocol === "https:" ? parsed.href : undefined;
}

/**
 * Same as {@link httpsUrl}, but for values used as a prefix that other path
 * segments are appended to, so the trailing slash is normalised away.
 */
export function httpsBaseUrl(
  value: string | undefined | null,
): string | undefined {
  return httpsUrl(value)?.replace(/\/+$/, "");
}
