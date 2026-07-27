# DEN-267 implementation branch

This feature branch contains executable TypeScript policy code, unit tests, a browser fixture, and a wiring contract for worker-enforced sensitive-field write blocking.

The intended runtime integration is:

- classify each resolved locator before `fill`, `type`, or `fill_form` mutates the DOM;
- reject SSN/tax-ID, bank/payment-card, and MFA/OTP/PIN targets regardless of value source;
- reject literal credentials;
- allow credentials only through a domain-bound `secret_ref`;
- return `sensitive_field_blocked` without echoing attempted values.

The one-shot branch workflow applies the guarded calls to `browser-agent.ts`, runs typechecking and tests, removes its temporary patch scripts/workflow, and commits the resulting source change back to the branch.
