/**
 * CAPTCHA solving is an operator-owned capability gate. A request may opt out,
 * but it can never elevate itself above the deployment's explicit policy.
 */
export function captchaAutoSolveAllowed(
  operatorEnabled: boolean,
  requestPreference: boolean | undefined,
): boolean {
  return operatorEnabled && requestPreference !== false;
}
