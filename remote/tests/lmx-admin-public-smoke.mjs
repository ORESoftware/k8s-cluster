#!/usr/bin/env node
// lmx-admin-public-smoke.mjs
//
// End-to-end probe of the live-mutex broker admin surface through the
// public gateway. Designed to be safe to re-run after every sync of
// `dd-secrets` (External Secrets) and `dd-next-runtime` (gateway
// ConfigMap + lmx deployments). Exits 0 when every broker passes the
// authenticated `/admin/otel` matrix; exits non-zero otherwise.
//
// What it covers (per broker, lmx-rs and lmx-node):
//   * Unauthenticated GET  /admin/otel              → expect 401
//   * Authenticated   GET  /admin/otel              → expect 200 + boolean
//   * Authenticated   POST /admin/otel {enabled:T}  → expect 200 + previous=F
//   * Authenticated   GET  /admin/otel              → expect enabled=true
//   * Authenticated   POST /admin/otel {enabled:F}  → expect 200 + previous=T
//   * Wrong token     GET  /admin/otel              → expect 401
//   * Status HTML     GET  /                        → expect 200 + title text
//
// Required env:
//   * LMX_BASE_URL          default: https://54.91.17.58
//   * DD_GATEWAY_AUTH       value of the gateway `Auth:` header (the
//                           `${DD_REMOTE_GATEWAY_AUTH_VALUE}` from the
//                           `dd-remote-gateway-config` ConfigMap)
//   * LMX_ADMIN_TOKEN       per-broker `/admin/*` shared secret (the
//                           `LMX_ADMIN_TOKEN` value plumbed in via the
//                           `dd-lmx-admin-token` Kubernetes Secret)
//
// Optional env:
//   * LMX_BROKERS                comma list, default "lmx-rs,lmx-node"
//   * LMX_VERBOSE                "1" for raw response bodies
//   * NODE_TLS_REJECT_UNAUTHORIZED=0  ignore TLS errors (the live
//                                gateway uses a self-signed cert during
//                                bootstrap; set this when targeting it)
//
// Pre-conditions before this can pass:
//   1. AWS Secrets Manager has `dd/remote-dev/lmx-admin-token` with key
//      `LMX_ADMIN_TOKEN`.
//   2. ArgoCD has synced `dd-secrets` so the Kubernetes Secret
//      `dd-lmx-admin-token` exists.
//   3. ArgoCD has synced `dd-next-runtime` so the gateway ConfigMap has
//      `/lmx-rs/*` and `/lmx-node/*` location blocks AND the
//      `dd-live-mutex` deployment has the `LMX_HTTP_PORT=6971` env var
//      AND `dd-live-mutex` Service exposes port 6971.
//   4. The two broker Deployments have been rolled (env var changes
//      only apply on pod restart).
//
// Until (1)–(4) land, every probe returns 401 with the gateway's
// `{"error":"unauthorized","errMessage":"missing required dd header"}`
// body — that's the catch-all `location /` returning the JSON 401.

// Use Node 22's built-in `fetch` (undici-backed) to avoid pulling a
// new dep into the test workspace. Self-signed cert handling on the
// bootstrap gateway is left to NODE_TLS_REJECT_UNAUTHORIZED=0 — that
// environment variable is read by Node at startup, before this script
// runs, so we cannot toggle it from inside.

const baseUrl = (process.env.LMX_BASE_URL ?? "https://54.91.17.58").replace(/\/+$/, "");
const ddAuth = process.env.DD_GATEWAY_AUTH;
const adminToken = process.env.LMX_ADMIN_TOKEN;
const brokers = (process.env.LMX_BROKERS ?? "lmx-rs,lmx-node")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
const verbose = process.env.LMX_VERBOSE === "1";

if (!ddAuth) {
    console.error("[lmx-smoke] DD_GATEWAY_AUTH is required (gateway Auth header value)");
    process.exit(2);
}
if (!adminToken) {
    console.error("[lmx-smoke] LMX_ADMIN_TOKEN is required (broker x-admin-token value)");
    process.exit(2);
}

let totalChecks = 0;
let totalFailed = 0;

function record(label, ok, detail) {
    totalChecks += 1;
    if (!ok) totalFailed += 1;
    const mark = ok ? "PASS" : "FAIL";
    console.log(`  [${mark}] ${label}${detail ? `  ${detail}` : ""}`);
}

async function call(method, path, { authToken, bearer = false, body } = {}) {
    const url = `${baseUrl}${path}`;
    const headers = {
        Auth: ddAuth,
    };
    if (authToken !== undefined && authToken !== null) {
        if (bearer) {
            headers.Authorization = `Bearer ${authToken}`;
        } else {
            headers["x-admin-token"] = authToken;
        }
    }
    if (body !== undefined) {
        headers["content-type"] = "application/json";
    }
    const res = await fetch(url, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: "manual",
    });
    const text = await res.text();
    let json = null;
    try {
        json = text ? JSON.parse(text) : null;
    } catch {
        // Non-JSON (HTML status page, gateway 401 body, etc.) — leave json null.
    }
    if (verbose) {
        console.log(`    > ${method} ${url} → HTTP ${res.status}`);
        console.log(`    < ${text.slice(0, 240)}${text.length > 240 ? "..." : ""}`);
    }
    return { status: res.status, text, json };
}

async function smokeBroker(broker) {
    console.log(`\n[lmx-smoke] broker=${broker}  base=${baseUrl}/${broker}`);

    const adminPath = `/${broker}/admin/otel`;

    // [1] Unauthenticated GET — broker should reject.
    {
        const r = await call("GET", adminPath, { authToken: null });
        record(
            "GET /admin/otel without x-admin-token returns 401",
            r.status === 401,
            `got HTTP ${r.status}`,
        );
    }

    // [2] Authenticated GET — broker returns boolean.
    let initial = null;
    {
        const r = await call("GET", adminPath, { authToken: adminToken });
        const okStatus = r.status === 200;
        const okShape = r.json && typeof r.json.enabled === "boolean";
        record(
            "GET /admin/otel with x-admin-token returns 200 {enabled:bool}",
            okStatus && okShape,
            `HTTP ${r.status} body=${r.text.slice(0, 80)}`,
        );
        if (okShape) initial = r.json.enabled;
    }

    // [3] POST flip to !initial — must report previous=initial.
    {
        const target = !initial;
        const r = await call("POST", adminPath, {
            authToken: adminToken,
            body: { enabled: target },
        });
        const okStatus = r.status === 200;
        const okPrev = r.json && r.json.previous === initial;
        const okNow = r.json && r.json.enabled === target;
        record(
            `POST /admin/otel {enabled:${target}} reports previous=${initial}, enabled=${target}`,
            okStatus && okPrev && okNow,
            `HTTP ${r.status} body=${r.text.slice(0, 80)}`,
        );
    }

    // [4] GET reflects the flip.
    {
        const r = await call("GET", adminPath, { authToken: adminToken, bearer: true });
        const okStatus = r.status === 200;
        const okEnabled = r.json && r.json.enabled === !initial;
        record(
            "GET via Authorization: Bearer reflects the new state",
            okStatus && okEnabled,
            `HTTP ${r.status} body=${r.text.slice(0, 80)}`,
        );
    }

    // [5] POST flip back to initial.
    {
        const r = await call("POST", adminPath, {
            authToken: adminToken,
            body: { enabled: initial },
        });
        const okStatus = r.status === 200;
        const okPrev = r.json && r.json.previous === !initial;
        const okNow = r.json && r.json.enabled === initial;
        record(
            `POST /admin/otel {enabled:${initial}} restores previous state`,
            okStatus && okPrev && okNow,
            `HTTP ${r.status} body=${r.text.slice(0, 80)}`,
        );
    }

    // [6] Wrong token — broker should reject (proves the override took
    //     effect; with the literal default still in place a wrong token
    //     would still 401, so step [2] succeeding with the configured
    //     LMX_ADMIN_TOKEN is what proves the rotation).
    {
        const r = await call("GET", adminPath, { authToken: "obviously-not-the-token" });
        record(
            "wrong x-admin-token rejected with 401",
            r.status === 401,
            `got HTTP ${r.status}`,
        );
    }

    // [7] Status HTML page renders (read-only today; Slice B/C will add
    //     the form controls so this string changes).
    {
        const r = await call("GET", `/${broker}/`, { authToken: null });
        const okStatus = r.status === 200;
        // lmx-rs renders `<title>dd-rust-network-mutex — broker status</title>`
        // lmx-node renders `<title>live-mutex broker status</title>`
        const titleOk = /broker status<\/title>/.test(r.text);
        record(
            "GET / returns broker status HTML",
            okStatus && titleOk,
            `HTTP ${r.status} bytes=${r.text.length}`,
        );
    }
}

(async () => {
    console.log(`[lmx-smoke] base=${baseUrl} brokers=${brokers.join(",")}`);
    for (const broker of brokers) {
        try {
            await smokeBroker(broker);
        } catch (error) {
            console.error(`[lmx-smoke] broker=${broker} threw:`, error?.stack || error);
            totalFailed += 1;
            totalChecks += 1;
        }
    }
    console.log(
        `\n[lmx-smoke] ${totalFailed === 0 ? "ALL GREEN" : "FAILURES"}: ${totalChecks - totalFailed}/${totalChecks} checks passed`,
    );
    process.exit(totalFailed === 0 ? 0 : 1);
})();
