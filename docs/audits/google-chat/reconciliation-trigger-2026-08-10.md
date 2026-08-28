# Google Chat reconciliation trigger record — 2026-08-10

Purpose: export the fixed `alex-alex-me` Google Chat space through the encrypted one-time relay so the rolling 15-day Linear/GitHub reconciliation can be completed.

Requested window: the 15 days ending 2026-08-10.

Relay attempt 2 was requested after the first encrypted archive was validated but its local ephemeral decryption state was lost before analysis. No source credential or passphrase is stored here.

No credentials are stored in this branch or file. The one-time trigger record is
kept outside `.github/chat-relay-trigger/` so later PR synchronizations cannot
launch another relay that has no matching encrypted payload.
