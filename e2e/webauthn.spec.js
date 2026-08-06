const { test, expect } = require('@playwright/test');

async function register(request, email) {
  const response = await request.post('/auth/register', {
    data: {
      email,
      password: 'correct horse battery staple',
      display_name: 'Playwright WebAuthn',
    },
  });
  expect(response.status(), await response.text()).toBe(201);
  return response.json();
}

async function startRegistration(request, accessToken) {
  const response = await request.post('/auth/passkeys/registration/options', {
    headers: { Authorization: `Bearer ${accessToken}` },
    data: { label: 'Chromium virtual authenticator' },
  });
  expect(response.status(), await response.text()).toBe(200);
  return response.json();
}

async function startAuthentication(request, accessToken) {
  const response = await request.post('/auth/passkeys/authentication/options', {
    headers: { Authorization: `Bearer ${accessToken}` },
    data: {},
  });
  expect(response.status(), await response.text()).toBe(200);
  return response.json();
}

async function createCredential(page, start) {
  return page.evaluate(async ({ options }) => {
    const decode = (value) => {
      const normalized = value.replace(/-/g, '+').replace(/_/g, '/');
      const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=');
      const binary = atob(padded);
      return Uint8Array.from(binary, (character) => character.charCodeAt(0));
    };
    const encode = (value) => {
      const bytes = new Uint8Array(value);
      let binary = '';
      for (const byte of bytes) binary += String.fromCharCode(byte);
      return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
    };

    const publicKey = structuredClone(options.publicKey);
    publicKey.challenge = decode(publicKey.challenge);
    publicKey.user.id = decode(publicKey.user.id);
    publicKey.timeout = Math.min(publicKey.timeout || 10_000, 10_000);
    if (publicKey.excludeCredentials) {
      publicKey.excludeCredentials = publicKey.excludeCredentials.map((descriptor) => ({
        ...descriptor,
        id: decode(descriptor.id),
      }));
    }
    const credential = await navigator.credentials.create({ publicKey });
    if (!(credential instanceof PublicKeyCredential)) {
      throw new Error('browser did not return a PublicKeyCredential');
    }
    return {
      id: credential.id,
      rawId: encode(credential.rawId),
      type: credential.type,
      response: {
        attestationObject: encode(credential.response.attestationObject),
        clientDataJSON: encode(credential.response.clientDataJSON),
        transports: credential.response.getTransports?.() || [],
      },
      extensions: credential.getClientExtensionResults(),
    };
  }, start);
}

async function getCredential(page, start) {
  return page.evaluate(async ({ options }) => {
    const decode = (value) => {
      const normalized = value.replace(/-/g, '+').replace(/_/g, '/');
      const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=');
      const binary = atob(padded);
      return Uint8Array.from(binary, (character) => character.charCodeAt(0));
    };
    const encode = (value) => {
      const bytes = new Uint8Array(value);
      let binary = '';
      for (const byte of bytes) binary += String.fromCharCode(byte);
      return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
    };

    const publicKey = structuredClone(options.publicKey);
    publicKey.challenge = decode(publicKey.challenge);
    publicKey.timeout = Math.min(publicKey.timeout || 10_000, 10_000);
    if (publicKey.allowCredentials) {
      publicKey.allowCredentials = publicKey.allowCredentials.map((descriptor) => ({
        ...descriptor,
        id: decode(descriptor.id),
      }));
    }
    const credential = await navigator.credentials.get({ publicKey });
    if (!(credential instanceof PublicKeyCredential)) {
      throw new Error('browser did not return a PublicKeyCredential');
    }
    const response = {
      authenticatorData: encode(credential.response.authenticatorData),
      clientDataJSON: encode(credential.response.clientDataJSON),
      signature: encode(credential.response.signature),
    };
    if (credential.response.userHandle) {
      response.userHandle = encode(credential.response.userHandle);
    }
    return {
      id: credential.id,
      rawId: encode(credential.rawId),
      type: credential.type,
      response,
      extensions: credential.getClientExtensionResults(),
    };
  }, start);
}

async function browserErrorName(page, operation, start) {
  return page.evaluate(async ({ operation, options }) => {
    const decode = (value) => {
      const normalized = value.replace(/-/g, '+').replace(/_/g, '/');
      const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=');
      const binary = atob(padded);
      return Uint8Array.from(binary, (character) => character.charCodeAt(0));
    };
    const publicKey = structuredClone(options.publicKey);
    publicKey.challenge = decode(publicKey.challenge);
    publicKey.timeout = 3_000;
    if (publicKey.user) publicKey.user.id = decode(publicKey.user.id);
    for (const field of ['allowCredentials', 'excludeCredentials']) {
      if (publicKey[field]) {
        publicKey[field] = publicKey[field].map((descriptor) => ({
          ...descriptor,
          id: decode(descriptor.id),
        }));
      }
    }
    try {
      if (operation === 'create') await navigator.credentials.create({ publicKey });
      else await navigator.credentials.get({ publicKey });
      return null;
    } catch (error) {
      return error.name;
    }
  }, { operation, ...start });
}

test('Chromium completes UV passkey registration and step-up, rejects replay and bad origin', async ({
  page,
  request,
}) => {
  await page.goto('/healthz');
  expect(await page.evaluate(() => window.isSecureContext)).toBe(true);

  const cdp = await page.context().newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2',
      transport: 'internal',
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });

  const suffix = `${Date.now()}-${Math.floor(Math.random() * 1_000_000)}`;
  const session = await register(request, `passkey-${suffix}@example.com`);
  const registration = await startRegistration(request, session.access_token);
  expect(registration.options.publicKey.authenticatorSelection.userVerification).toBe('required');
  const registeredCredential = await createCredential(page, registration);
  const registrationFinish = await request.post('/auth/passkeys/registration/verify', {
    headers: { Authorization: `Bearer ${session.access_token}` },
    data: {
      challenge_id: registration.challenge_id,
      credential: registeredCredential,
      label: 'Chromium virtual authenticator',
    },
  });
  expect(registrationFinish.status(), await registrationFinish.text()).toBe(200);

  const authentication = await startAuthentication(request, session.access_token);
  expect(authentication.options.publicKey.userVerification).toBe('required');
  const assertion = await getCredential(page, authentication);
  const finishBody = {
    challenge_id: authentication.challenge_id,
    credential: assertion,
  };
  const authenticationFinish = await request.post('/auth/passkeys/authentication/verify', {
    headers: { Authorization: `Bearer ${session.access_token}` },
    data: finishBody,
  });
  expect(authenticationFinish.status(), await authenticationFinish.text()).toBe(200);
  const stepUp = await authenticationFinish.json();
  expect(stepUp.amr).toContain('passkey');
  expect(stepUp.acr).toBe('urn:oresoftware:loa:2');

  const replay = await request.post('/auth/passkeys/authentication/verify', {
    headers: { Authorization: `Bearer ${session.access_token}` },
    data: finishBody,
  });
  expect(replay.status()).toBe(401);

  const noUvChallenge = await startAuthentication(request, session.access_token);
  await cdp.send('WebAuthn.setUserVerified', {
    authenticatorId,
    isUserVerified: false,
  });
  expect(await browserErrorName(page, 'get', noUvChallenge)).toBe('NotAllowedError');
  await cdp.send('WebAuthn.setUserVerified', {
    authenticatorId,
    isUserVerified: true,
  });

  const originSession = await register(request, `origin-${suffix}@example.com`);
  const wrongOriginRegistration = await startRegistration(request, originSession.access_token);
  await page.goto('http://127.0.0.1:8120/healthz');
  expect(await browserErrorName(page, 'create', wrongOriginRegistration)).toBe('SecurityError');

  await cdp.send('WebAuthn.removeVirtualAuthenticator', { authenticatorId });
});
