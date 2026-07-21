// Cloudflare Worker — edge session gate for OreSoftware apps.
//
// Sits in front of a protected origin and validates the caller's session at the
// edge, in tandem with both auth systems:
//
//   1. Prefer the OreSoftware session cookie/bearer → verify it against the
//      shared-auth server's JWKS (cached at the edge). Fast, and works even if
//      Supabase is down.
//   2. Fall back to a Supabase access token → exchange it at shared-auth for an
//      OreSoftware token, set the cookie, and continue. This keeps users signed
//      in even if shared-auth was briefly unavailable when they logged in.
//   3. No valid session → 302 redirect to the login page (or 401 for API/XHR).
//
// The Worker never holds signing keys or secrets: it verifies via public JWKS and
// delegates token minting to shared-auth. Config is env vars (wrangler.toml).

const COOKIE = "ore_session";

export default {
  async fetch(request, env, ctx) {
    const cfg = loadConfig(env);
    const url = new URL(request.url);

    // Never gate the login page or the auth server's own public endpoints.
    if (url.pathname.startsWith("/auth/") || url.pathname === cfg.loginPath) {
      return fetch(request);
    }

    const session = await resolveSession(request, cfg, ctx);
    if (session.ok) {
      // Forward identity to the origin as trusted headers.
      const headers = new Headers(request.headers);
      headers.set("x-auth-user-id", session.claims.sub);
      headers.set("x-auth-project", session.claims.project || "");
      if (session.claims.email) headers.set("x-auth-email", session.claims.email);
      const resp = await fetch(new Request(request, { headers }));
      // If we just minted a fresh cookie (fallback path), attach it.
      if (session.setCookie) {
        const out = new Response(resp.body, resp);
        out.headers.append("set-cookie", session.setCookie);
        return out;
      }
      return resp;
    }

    return unauthorized(request, url, cfg);
  },
};

function loadConfig(env) {
  return {
    // shared-auth-server base URL, e.g. https://gateway/shared-auth
    authBase: (env.SHARED_AUTH_BASE || "").replace(/\/$/, ""),
    jwksUrl:
      env.SHARED_AUTH_JWKS_URL ||
      `${(env.SHARED_AUTH_BASE || "").replace(/\/$/, "")}/.well-known/jwks.json`,
    issuer: env.AUTH_ISSUER || "https://auth.oresoftware.dev",
    audience: env.AUTH_AUDIENCE || "oresoftware",
    loginPath: env.LOGIN_PATH || "/auth/sign-in",
    // JWKS edge-cache TTL (seconds). Long enough to ride out a shared-auth blip.
    jwksTtl: Number(env.JWKS_TTL_SECONDS || 3600),
  };
}

async function resolveSession(request, cfg, ctx) {
  // 1. OreSoftware session cookie or bearer → verify locally against JWKS.
  const ore = bearer(request) || cookie(request, COOKIE);
  if (ore) {
    const claims = await verifyJwt(ore, cfg, ctx).catch(() => null);
    if (claims) return { ok: true, claims };
  }

  // 2. Supabase access token → exchange at shared-auth, set our cookie.
  const supa = request.headers.get("x-supabase-token") || cookie(request, "sb-access-token");
  if (supa && cfg.authBase) {
    const exchanged = await exchangeSupabase(supa, cfg).catch(() => null);
    if (exchanged) {
      const claims = await verifyJwt(exchanged.access_token, cfg, ctx).catch(() => null);
      if (claims) {
        return {
          ok: true,
          claims,
          setCookie: `${COOKIE}=${exchanged.access_token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=3600`,
        };
      }
    }
  }

  return { ok: false };
}

async function exchangeSupabase(supabaseToken, cfg) {
  const resp = await fetch(`${cfg.authBase}/auth/exchange`, {
    method: "POST",
    headers: { authorization: `Bearer ${supabaseToken}` },
  });
  if (!resp.ok) return null;
  return resp.json();
}

// --- Minimal ES256 JWT verification at the edge (WebCrypto), JWKS-cached ---

let jwksCache = { at: 0, keys: null };

async function getJwks(cfg, ctx) {
  const now = Date.now() / 1000;
  if (jwksCache.keys && now - jwksCache.at < cfg.jwksTtl) return jwksCache.keys;
  const resp = await fetch(cfg.jwksUrl, { cf: { cacheTtl: cfg.jwksTtl } });
  if (!resp.ok) {
    // shared-auth JWKS unavailable: keep serving the last-known keys (tandem
    // resilience — a stale key rides out a multi-minute shared-auth outage).
    if (jwksCache.keys) return jwksCache.keys;
    throw new Error("jwks unavailable");
  }
  const body = await resp.json();
  jwksCache = { at: now, keys: body.keys || [] };
  return jwksCache.keys;
}

async function verifyJwt(token, cfg, ctx) {
  const [h, p, s] = token.split(".");
  if (!h || !p || !s) throw new Error("malformed");
  const header = JSON.parse(b64urlToString(h));
  const claims = JSON.parse(b64urlToString(p));

  if (claims.iss !== cfg.issuer || claims.aud !== cfg.audience) throw new Error("iss/aud");
  if (claims.exp && claims.exp < Date.now() / 1000) throw new Error("expired");

  const jwk = (await getJwks(cfg, ctx)).find((k) => k.kid === header.kid) || (await getJwks(cfg, ctx))[0];
  if (!jwk) throw new Error("no key");
  const key = await crypto.subtle.importKey(
    "jwk",
    { kty: jwk.kty, crv: jwk.crv, x: jwk.x, y: jwk.y, ext: true },
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["verify"],
  );
  const ok = await crypto.subtle.verify(
    { name: "ECDSA", hash: "SHA-256" },
    key,
    b64urlToBytes(s),
    new TextEncoder().encode(`${h}.${p}`),
  );
  if (!ok) throw new Error("bad signature");
  return claims;
}

function unauthorized(request, url, cfg) {
  const accept = request.headers.get("accept") || "";
  const isBrowser = accept.includes("text/html");
  if (!isBrowser) return new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 });
  const to = new URL(cfg.loginPath, url.origin);
  to.searchParams.set("return", url.pathname + url.search);
  return Response.redirect(to.toString(), 302);
}

// --- helpers ---
function bearer(request) {
  const h = request.headers.get("authorization") || "";
  return h.startsWith("Bearer ") ? h.slice(7).trim() : null;
}
function cookie(request, name) {
  const raw = request.headers.get("cookie") || "";
  const m = raw.match(new RegExp(`(?:^|; )${name}=([^;]+)`));
  return m ? m[1] : null;
}
function b64urlToBytes(s) {
  const b = atob(s.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(s.length / 4) * 4, "="));
  const out = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) out[i] = b.charCodeAt(i);
  return out;
}
function b64urlToString(s) {
  return new TextDecoder().decode(b64urlToBytes(s));
}
