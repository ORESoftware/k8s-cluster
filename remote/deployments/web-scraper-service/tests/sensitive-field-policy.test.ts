import assert from 'node:assert/strict';
import test from 'node:test';

import {
  SensitiveFieldPolicyError,
  assertSensitiveFieldWriteAllowed,
  classifySensitiveField,
  decideSensitiveFieldWrite,
} from '../src/sensitive-field-policy.ts';

test('government identifiers are always blocked', () => {
  for (const field of [
    { label: 'Social Security Number' },
    { name: 'ssn' },
    { id: 'taxpayer-identification-number' },
    { placeholder: 'EIN' },
    { ariaLabel: 'ITIN' },
    { name: 'profile', label: 'Tax ID (optional)' },
  ]) {
    assert.equal(classifySensitiveField(field), 'government_identifier');
    assert.equal(decideSensitiveFieldWrite(field, 'literal').allowed, false);
    assert.equal(decideSensitiveFieldWrite(field, 'secret_ref').allowed, false);
  }
});

test('bank and payment fields are always blocked', () => {
  for (const field of [
    { label: 'Routing number' },
    { label: 'Bank account number' },
    { name: 'bank_account_number' },
    { autocomplete: 'cc-number' },
    { autocomplete: 'cc-csc' },
    { name: 'cvv' },
    { placeholder: 'Expiration date' },
  ]) {
    const kind = classifySensitiveField(field);
    assert.ok(kind === 'banking' || kind === 'payment_card');
    assert.equal(decideSensitiveFieldWrite(field, 'literal').allowed, false);
    assert.equal(decideSensitiveFieldWrite(field, 'secret_ref').allowed, false);
  }
});

test('MFA, OTP, and PIN controls are always blocked', () => {
  for (const field of [
    { autocomplete: 'one-time-code' },
    { label: 'Authenticator code' },
    { name: 'otp' },
    { placeholder: 'Verification code' },
    { id: 'security-pin' },
  ]) {
    assert.equal(classifySensitiveField(field), 'mfa');
    assert.equal(decideSensitiveFieldWrite(field, 'literal').allowed, false);
    assert.equal(decideSensitiveFieldWrite(field, 'secret_ref').allowed, false);
  }
});

test('credentials reject literals and allow only secret references', () => {
  for (const field of [
    { type: 'password', label: 'Password' },
    { label: 'API key' },
    { placeholder: 'Private key' },
  ]) {
    assert.equal(classifySensitiveField(field), 'credential');
    assert.equal(decideSensitiveFieldWrite(field, 'literal').allowed, false);
    assert.equal(decideSensitiveFieldWrite(field, 'secret_ref').allowed, true);
  }
});

test('ordinary application fields remain writable', () => {
  for (const field of [
    { label: 'Email address', type: 'email' },
    { label: 'Full name' },
    { label: 'LinkedIn profile' },
    { label: 'Account manager title' },
    { label: 'Security engineer experience' },
    { placeholder: 'Portfolio URL' },
  ]) {
    assert.equal(classifySensitiveField(field), 'ordinary');
    assert.doesNotThrow(() => assertSensitiveFieldWriteAllowed(field, 'literal'));
  }
});

test('blocked errors are stable and never contain the attempted value', () => {
  const attemptedValue = '123-45-6789';
  assert.throws(
    () => assertSensitiveFieldWriteAllowed({ label: 'SSN' }, 'literal'),
    (error: unknown) => {
      assert.ok(error instanceof SensitiveFieldPolicyError);
      assert.equal(error.code, 'sensitive_field_blocked');
      assert.equal(error.kind, 'government_identifier');
      assert.equal(error.message.includes(attemptedValue), false);
      return true;
    },
  );
});
