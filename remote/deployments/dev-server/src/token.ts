// HMAC verification for direct frontend → docker SSE streaming.
//
// Mirror of src/lib/server/remote-dev/token.ts on the Vercel side. Both
// sides MUST share the same REMOTE_DEV_TOKEN_SECRET.
//
// Token format (compact, JWT-ish):
//   base64url( JSON(payload) ) "." base64url( HMAC-SHA256( base64url(payload) ) )

import { createHmac, timingSafeEqual } from "node:crypto";

export interface DirectStreamTokenPayload {
  taskId: string;
  userId: string;
  /** Unix seconds. */
  exp: number;
  jti: string;
}

function getSecret(): string | null {
  const secret = process.env.REMOTE_DEV_TOKEN_SECRET;
  if (!secret || secret.length < 16) return null;
  return secret;
}

export function verifyDirectStreamToken(
  token: string,
): DirectStreamTokenPayload | null {
  const secret = getSecret();
  if (!secret) return null;
  if (!token || typeof token !== "string") return null;

  const dot = token.indexOf(".");
  if (dot < 1 || dot >= token.length - 1) return null;

  const b64Payload = token.slice(0, dot);
  const sigGiven = token.slice(dot + 1);

  const sigExpected = createHmac("sha256", secret)
    .update(b64Payload)
    .digest("base64url");

  const givenBuf = Buffer.from(sigGiven, "utf8");
  const expectedBuf = Buffer.from(sigExpected, "utf8");
  if (givenBuf.length !== expectedBuf.length) return null;
  if (!timingSafeEqual(givenBuf, expectedBuf)) return null;

  let parsed: unknown;
  try {
    parsed = JSON.parse(Buffer.from(b64Payload, "base64url").toString("utf8"));
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const p = parsed as Record<string, unknown>;
  if (
    typeof p.taskId !== "string" ||
    typeof p.userId !== "string" ||
    typeof p.exp !== "number" ||
    typeof p.jti !== "string"
  ) {
    return null;
  }
  if (p.exp <= Math.floor(Date.now() / 1000)) return null;

  return {
    taskId: p.taskId,
    userId: p.userId,
    exp: p.exp,
    jti: p.jti,
  };
}
