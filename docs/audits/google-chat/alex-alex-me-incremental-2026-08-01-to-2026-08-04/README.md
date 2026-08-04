# Incremental Google Chat reconciliation: `alex-alex-me`

This ledger accounts for every message added after the previous June 5 audit
without committing message bodies, credential values, contact values, or the raw
export. It is the incremental companion to the 955-record ledger in PR #492.

## Verified input

- Space: `alex-alex-me` (`spaces/AAQAoHKdzvI`)
- Google Apps Script bridge: version `1.0.1`
- Encrypted relay run: `30874714077-1`
- Fresh export: **5 pages / 1,186 messages**
- Complete requested window since `2026-06-05T00:00:00Z`: **1,003 messages**
- Previous audited cutoff: `2026-08-01T15:32:03.554959Z`
- Incremental window: after that cutoff through `2026-08-04T02:53:06.997967Z`
- Incremental messages: **48**
- Previously audited messages edited: **0**
- Previously audited messages deleted: **0**
- Exact duplicate occurrences after the first copy: **4**

The full June 5 window is therefore accounted for as **955 previously audited +
48 incremental = 1,003 messages**.

## Dispositions

| Disposition | Messages |
| --- | ---: |
| `mapped-existing-work` | 24 |
| `quarantined-private-contact` | 8 |
| `reference-only-supporting-work` | 7 |
| `quarantined-security-and-routed` | 4 |
| `mapped-created-work` | 3 |
| `reference-only-needs-source-review` | 2 |

`quarantined-security-and-routed` means the credential value was omitted and
routed to `DEN-1230`/`DEN-27`, while the surrounding engineering instruction was
also routed to its owning ticket. `mapped-created-work` identifies prompts for
which this reconciliation created focused Linear issues. Reference records are
kept content-free and either support an existing work item or remain on the
prompt-intake queue when the pasted output did not state an actionable target.

## New focused work created

- `DEN-1601` — meter browser-automation runs and send deduplicated plan-upgrade notifications.
- `DEN-1602` — bootstrap `ORESoftware/slack-ores-integrations` as a thin manifest/runbook repository without forking the tested Rust runtime; blocked by `DEN-319` repository-creation authorization.

## Routing categories

| Category | Messages | Canonical Linear routing |
| --- | ---: | --- |
| `ai_mcp_slack` | 10 | DEN-1298 (10), DEN-1041 (9), DEN-1042 (3), DEN-1602 (3), DEN-1230 (2), DEN-1231 (2), DEN-27 (2), DEN-1272 (1), DEN-1273 (1), DEN-1274 (1), DEN-1275 (1), DEN-1276 (1), DEN-1277 (1), DEN-1278 (1), DEN-1279 (1), DEN-1280 (1), DEN-1281 (1), DEN-1283 (1), DEN-1285 (1), DEN-1287 (1), DEN-319 (1) |
| `github_linear_k8s_ops` | 9 | DEN-598 (4), DEN-599 (3), DEN-629 (3), DEN-1168 (2), DEN-1408 (2), DEN-1409 (2), DEN-1410 (2), DEN-1586 (2), DEN-268 (2), DEN-822 (2), DEN-1321 (1), DEN-1580 (1), DEN-464 (1), DEN-630 (1), DEN-801 (1) |
| `zed_opto_sync` | 9 | DEN-1411 (3), DEN-1425 (3), DEN-1435 (3), DEN-1153 (2), DEN-1388 (2), DEN-1439 (2), DEN-309 (2), DEN-313 (2), DEN-100 (1), DEN-1230 (1), DEN-1420 (1), DEN-1505 (1), DEN-27 (1) |
| `private` | 8 | No engineering ticket; quarantined |
| `fiducia` | 3 | DEN-1390 (2), DEN-1391 (2), DEN-1392 (2), DEN-1393 (2), DEN-1394 (2), DEN-1154 (1), DEN-1230 (1), DEN-1549 (1), DEN-1550 (1), DEN-1590 (1), DEN-1600 (1), DEN-27 (1), DEN-80 (1) |
| `messaging_intel` | 2 | DEN-14 (2), DEN-1548 (2), DEN-411 (2), DEN-414 (2) |
| `sonus_auris` | 2 | DEN-1398 (2), DEN-1372 (1), DEN-1402 (1) |
| `streempilot` | 2 | DEN-876 (2), DEN-896 (2), DEN-902 (2), DEN-913 (2), DEN-918 (2) |
| `auth_3fa_shared` | 1 | DEN-1376 (1), DEN-1379 (1), DEN-253 (1), DEN-44 (1), DEN-793 (1) |
| `jobs_admin` | 1 | DEN-256 (1), DEN-395 (1), DEN-823 (1), DEN-826 (1) |
| `quaestor_billing` | 1 | DEN-1429 (1), DEN-1601 (1), DEN-793 (1) |

## Linear routing totals

Counts overlap when one message legitimately maps to multiple issues.

| Linear issue | Routed messages |
| --- | ---: |
| DEN-1298 | 10 |
| DEN-1041 | 9 |
| DEN-1230 | 4 |
| DEN-27 | 4 |
| DEN-598 | 4 |
| DEN-1042 | 3 |
| DEN-1411 | 3 |
| DEN-1425 | 3 |
| DEN-1435 | 3 |
| DEN-1602 | 3 |
| DEN-599 | 3 |
| DEN-629 | 3 |
| DEN-1153 | 2 |
| DEN-1168 | 2 |
| DEN-1231 | 2 |
| DEN-1388 | 2 |
| DEN-1390 | 2 |
| DEN-1391 | 2 |
| DEN-1392 | 2 |
| DEN-1393 | 2 |
| DEN-1394 | 2 |
| DEN-1398 | 2 |
| DEN-14 | 2 |
| DEN-1408 | 2 |
| DEN-1409 | 2 |
| DEN-1410 | 2 |
| DEN-1439 | 2 |
| DEN-1548 | 2 |
| DEN-1586 | 2 |
| DEN-268 | 2 |
| DEN-309 | 2 |
| DEN-313 | 2 |
| DEN-411 | 2 |
| DEN-414 | 2 |
| DEN-793 | 2 |
| DEN-822 | 2 |
| DEN-876 | 2 |
| DEN-896 | 2 |
| DEN-902 | 2 |
| DEN-913 | 2 |
| DEN-918 | 2 |
| DEN-100 | 1 |
| DEN-1154 | 1 |
| DEN-1272 | 1 |
| DEN-1273 | 1 |
| DEN-1274 | 1 |
| DEN-1275 | 1 |
| DEN-1276 | 1 |
| DEN-1277 | 1 |
| DEN-1278 | 1 |
| DEN-1279 | 1 |
| DEN-1280 | 1 |
| DEN-1281 | 1 |
| DEN-1283 | 1 |
| DEN-1285 | 1 |
| DEN-1287 | 1 |
| DEN-1321 | 1 |
| DEN-1372 | 1 |
| DEN-1376 | 1 |
| DEN-1379 | 1 |
| DEN-1402 | 1 |
| DEN-1420 | 1 |
| DEN-1429 | 1 |
| DEN-1505 | 1 |
| DEN-1549 | 1 |
| DEN-1550 | 1 |
| DEN-1580 | 1 |
| DEN-1590 | 1 |
| DEN-1600 | 1 |
| DEN-1601 | 1 |
| DEN-253 | 1 |
| DEN-256 | 1 |
| DEN-319 | 1 |
| DEN-395 | 1 |
| DEN-44 | 1 |
| DEN-464 | 1 |
| DEN-630 | 1 |
| DEN-80 | 1 |
| DEN-801 | 1 |
| DEN-823 | 1 |
| DEN-826 | 1 |

## Safety boundary

The delta contains **4 credential-bearing prompts** and **8 private contact
records**. Credential values and contact values are absent from this repository.
The four engineering prompts that also contained credentials were routed to both
the security incidents and their product/CI owners. The user-provided bridge
credential was transmitted to the one-time relay only as RSA-OAEP ciphertext,
and the exported archive remained encrypted at rest until local reconciliation.

The bridge status warns that GET transport can expose tokens in URL logs. The
bridge token and the separately pasted Linear API key must be rotated after this
audit; this ledger does not claim rotation has occurred.

## Duplicate handling

Four exact duplicates were found after the first occurrence. Each duplicate
record keeps its own source key and timestamp and carries a `dup` pointer to the
first identical message. No prompt is silently dropped.

## Machine-readable ledger

- Index: `index.json`
- Content-free ledger: `ledger.json.gz.base64.part-0001`
- Gzip SHA-256: `057e2e56ca46861176a7e7df2ca12056ae8211a0c37437f74ac9490845e81b59`
- Records: `48`
- Uncompressed JSON bytes: `8307`
- Compressed bytes: `2077`

After decompression, each record stores:

- `id`: the suffix appended to `sourceKeyPrefix`;
- `t`: Google Chat create time;
- `d`: disposition;
- `c`: routing category;
- optional `i`: canonical Linear identifiers;
- optional `dup`: first identical message ID;
- optional `n`: safe reconciliation note codes.

No raw Chat text is present.

```bash
cat ledger.json.gz.base64.part-* | base64 --decode > ledger.json.gz
sha256sum ledger.json.gz
gzip --decompress --stdout ledger.json.gz > ledger.json
```
