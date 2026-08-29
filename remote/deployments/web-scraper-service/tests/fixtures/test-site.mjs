// Minimal multi-page fixture site for browser-agent integration tests.
// Exercises: required-field validation, a select + checkbox, navigation between
// steps, a consequential "Submit filing" button, and a simulated CAPTCHA page.
import { createServer } from 'node:http';

const PAGE_1 = `<!doctype html><html><head><title>Step 1 - Entity</title></head><body>
  <h1>Register a business entity</h1>
  <form id="reg" method="GET" action="/step2">
    <label for="entity">Entity name</label>
    <input id="entity" name="entity" type="text" required placeholder="Legal name" />
    <label for="state">State</label>
    <select id="state" name="state">
      <option value="">Choose...</option>
      <option value="CO">Colorado</option>
      <option value="NY">New York</option>
    </select>
    <label><input id="agree" name="agree" type="checkbox" required /> I agree to the terms</label>
    <label for="attachment">Attachment</label>
    <input id="attachment" name="attachment" type="file" />
    <output id="upload-status">No attachment selected</output>
    <button id="next" type="submit">Next</button>
  </form>
  <script>
    document.querySelector('#attachment').addEventListener('change', (event) => {
      const file = event.target.files[0];
      document.querySelector('#upload-status').textContent =
        file ? 'Selected ' + file.name + ' (' + file.size + ' bytes)' : 'No attachment selected';
    });
  </script>
</body></html>`;

function page2(entity, state) {
  return `<!doctype html><html><head><title>Step 2 - Review</title></head><body>
  <h1>Review and file</h1>
  <p>Entity: <b>${entity || '(none)'}</b> in <b>${state || '(none)'}</b></p>
  <form id="file" method="GET" action="/done">
    <button id="submit-filing" type="submit">Submit filing</button>
  </form>
  <a id="captcha-link" href="/captcha">verify identity</a>
  <a id="external-link" href="https://example.com/">external site</a>
</body></html>`;
}

const CAPTCHA = `<!doctype html><html><head><title>Verify</title></head><body>
  <h1>Please complete the CAPTCHA to continue</h1>
  <div class="g-recaptcha" data-sitekey="fixture"></div>
</body></html>`;

const STARTUP_FORM = `<!doctype html><html><head><title>Startup application</title></head><body>
  <h1>Startup application</h1>
  <p>Our product is designed for startups and supports multi-factor authentication for customer accounts.</p>
  <p>No payment method is required to submit this application.</p>
  <form>
    <label for="first-name">First name</label><input id="first-name" name="first_name" />
    <label for="last-name">Last name</label><input id="last-name" name="last_name" />
    <label for="email">Work email</label><input id="email" name="email" />
    <label for="company">Company</label><input id="company" name="company" />
    <label for="domain">Company domain</label><input id="domain" name="domain" />
    <label for="funding">Funding</label><select id="funding" name="funding"><option>Bootstrapped</option></select>
    <label for="founded">Founded</label><select id="founded" name="founded"><option>Last 12 months</option></select>
    <label for="size">Company size</label><select id="size" name="size"><option>1-25</option></select>
  </form>
</body></html>`;

const MFA = `<!doctype html><html><head><title>Verify your sign-in</title></head><body>
  <h1>Enter the verification code</h1>
  <form><label for="code">One-time code</label><input id="code" name="verification_code" /></form>
</body></html>`;

const PAYMENT = `<!doctype html><html><head><title>Payment method</title></head><body>
  <h1>Add a payment method</h1>
  <form>
    <label for="card">Card number</label><input id="card" name="cardnumber" autocomplete="cc-number" />
    <label for="expiry">Expiration date</label><input id="expiry" autocomplete="cc-exp" />
    <label for="cvc">CVC</label><input id="cvc" name="cvc" autocomplete="cc-csc" />
  </form>
</body></html>`;

const DONE = `<!doctype html><html><head><title>Filing complete</title></head><body>
  <h1>Filing complete</h1><p id="ok">Your filing was submitted.</p>
</body></html>`;

export async function startFixture() {
  const server = createServer((req, res) => {
    const url = new URL(req.url, 'http://localhost');
    res.setHeader('content-type', 'text/html; charset=utf-8');
    if (url.pathname === '/' || url.pathname === '/step1') return res.end(PAGE_1);
    if (url.pathname === '/step2') return res.end(page2(url.searchParams.get('entity'), url.searchParams.get('state')));
    if (url.pathname === '/captcha') return res.end(CAPTCHA);
    if (url.pathname === '/startup') return res.end(STARTUP_FORM);
    if (url.pathname === '/mfa') return res.end(MFA);
    if (url.pathname === '/payment') return res.end(PAYMENT);
    if (url.pathname === '/done') return res.end(DONE);
    res.statusCode = 404;
    res.end('<h1>not found</h1>');
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address();
  return {
    url: `http://127.0.0.1:${port}`,
    async close() {
      await new Promise((resolve) => server.close(resolve));
    },
  };
}
