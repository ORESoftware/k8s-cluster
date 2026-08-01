# Google Chat reconciliation: `alex-alex-me` since 2026-06-05

This ledger accounts for every Google Chat message in the requested window
without committing message bodies, credential values, contact values, or the raw
export. The machine-readable ledger is gzip-compressed and stored as eight deterministic
base64 parts. It contains only source-key message IDs, timestamps, dispositions,
routing categories, canonical Linear issue identifiers, and exact duplicate
provenance.

## Verified input

- Space: `alex-alex-me` (`spaces/AAQAoHKdzvI`)
- Bridge: Google Apps Script HTTP bridge version `1.0.1`
- Export: 5 pages / 1,138 messages
- Requested window: `2026-06-05T00:00:00Z` through `2026-08-01T15:32:03.554959Z`
- Window messages: **955**
- Exact duplicate occurrences after the first copy: **16**

## Dispositions

| Disposition | Messages |
| --- | --- |
| mapped-existing-work | 861 |
| quarantined-private-contact | 42 |
| reference-only-needs-source-review | 24 |
| quarantined-security | 15 |
| excluded-empty | 9 |
| excluded-private-personal | 2 |
| excluded-private-operational | 1 |
| reference-only-commit | 1 |

`mapped-existing-work` means the source key is routed to one or more existing
canonical Linear issues instead of creating a duplicate issue. Reference-only
messages remain on the prompt-intake queue because their linked content was not
embedded in the Chat export. Private/contact/operational records and empty
messages are explicitly accounted for but are not copied into engineering
tickets.

## Routing categories

| Category | Messages | Canonical Linear routing |
| --- | --- | --- |
| akrion_soccer | 478 | DEN-863 (253), DEN-104 (197), DEN-103 (164), DEN-862 (164), DEN-936 (89), DEN-624 (12), DEN-669 (12), DEN-873 (11), DEN-1228 (5) |
| github_linear_k8s_ops | 98 | DEN-161 (85), DEN-822 (13), DEN-834 (13), DEN-319 (8) |
| general_intake | 86 | DEN-822 (86) |
| ai_mcp_slack | 69 | DEN-1066 (34), DEN-171 (21), DEN-174 (21), DEN-864 (10), DEN-822 (2), DEN-834 (2), DEN-766 (2), DEN-1041 (2), DEN-1042 (2) |
| fiducia | 47 | DEN-608 (28), DEN-1154 (5), DEN-566 (5), DEN-871 (4), DEN-500 (4), DEN-945 (4), DEN-82 (2) |
| private | 45 | No engineering ticket |
| sonus_auris | 28 | DEN-293 (14), DEN-844 (14), DEN-609 (11), DEN-1024 (3) |
| reference | 25 | DEN-822 (25) |
| zed_pkg_clients | 19 | DEN-637 (14), DEN-1107 (11), DEN-1123 (5) |
| security | 15 | DEN-1230 (15) |
| opto_sync_cliptown | 11 | DEN-817 (6), DEN-825 (6), DEN-829 (3), DEN-821 (3), DEN-818 (1), DEN-831 (1), DEN-40 (1) |
| nonactionable | 9 | No engineering ticket |
| benefactor | 7 | DEN-833 (4), DEN-622 (3) |
| hypesiege_streempilot | 6 | DEN-877 (5), DEN-881 (4), DEN-875 (1) |
| auth_3fa_shared | 5 | DEN-44 (2), DEN-663 (2), DEN-664 (2), DEN-665 (2), DEN-981 (1) |
| jobs_admin | 2 | DEN-822 (2) |
| dancing_dragons | 2 | DEN-795 (2) |
| quaestor_ledger | 2 | DEN-1138 (2) |
| usa_acc | 1 | DEN-936 (1) |

## Linear issue routing totals

Counts overlap when one message legitimately maps to multiple issues.

| Linear issue | Mapped messages |
| --- | --- |
| DEN-863 | 253 |
| DEN-104 | 197 |
| DEN-103 | 164 |
| DEN-862 | 164 |
| DEN-822 | 128 |
| DEN-936 | 90 |
| DEN-161 | 85 |
| DEN-1066 | 34 |
| DEN-608 | 28 |
| DEN-171 | 21 |
| DEN-174 | 21 |
| DEN-1230 | 15 |
| DEN-834 | 15 |
| DEN-293 | 14 |
| DEN-844 | 14 |
| DEN-637 | 14 |
| DEN-624 | 12 |
| DEN-669 | 12 |
| DEN-609 | 11 |
| DEN-873 | 11 |
| DEN-1107 | 11 |
| DEN-864 | 10 |
| DEN-319 | 8 |
| DEN-817 | 6 |
| DEN-825 | 6 |
| DEN-877 | 5 |
| DEN-1228 | 5 |
| DEN-1154 | 5 |
| DEN-566 | 5 |
| DEN-1123 | 5 |
| DEN-871 | 4 |
| DEN-833 | 4 |
| DEN-500 | 4 |
| DEN-945 | 4 |
| DEN-881 | 4 |
| DEN-1024 | 3 |
| DEN-622 | 3 |
| DEN-829 | 3 |
| DEN-821 | 3 |
| DEN-82 | 2 |
| DEN-44 | 2 |
| DEN-795 | 2 |
| DEN-1138 | 2 |
| DEN-663 | 2 |
| DEN-664 | 2 |
| DEN-665 | 2 |
| DEN-766 | 2 |
| DEN-1041 | 2 |
| DEN-1042 | 2 |
| DEN-981 | 1 |
| DEN-818 | 1 |
| DEN-831 | 1 |
| DEN-40 | 1 |
| DEN-875 | 1 |

## Safety boundary

The safety pass quarantined 15 credential-bearing messages and 42 private
contact messages. The source-key ledger routes the credential records to
`DEN-1230` without including values. Phone, WhatsApp, and other contact values
are not present in this repository. Two additional private/personal messages and
one private operational-address message are excluded from engineering intake.

## Reproduction notes

The raw export is an ephemeral encrypted operational artifact and must not be
committed. Run the DEN-1230 sanitizer before the DEN-266 planner. The JSON ledger
is intentionally content-free so it can be reviewed, diffed, and attached to
Linear safely.

## Machine-readable ledger

- Index: `index.json`
- Complete content-free ledger: concatenate `ledger.json.gz.base64.part-0001` through `part-0008`, base64-decode the result, then gunzip it.
- The index records the gzip SHA-256, record count, compressed size, base64 character count, part count, and uncompressed JSON size.
- After decompression, each record stores `id` (appended to `sourceKeyPrefix`), `t` (create time), `d` (disposition), `c` (category), optional `i` (Linear issue identifiers), and optional `dup` (the first identical message ID).

The compressed ledger contains all 955 records in chronological order. No raw Chat text is present.

```bash
cat ledger.json.gz.base64.part-* | base64 --decode > ledger.json.gz
sha256sum ledger.json.gz
gzip --decompress --stdout ledger.json.gz > ledger.json
```
