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
    <button id="next" type="submit">Next</button>
  </form>
</body></html>`;

function page2(entity, state) {
  return `<!doctype html><html><head><title>Step 2 - Review</title></head><body>
  <h1>Review and file</h1>
  <p>Entity: <b>${entity || '(none)'}</b> in <b>${state || '(none)'}</b></p>
  <form id="file" method="GET" action="/done">
    <button id="submit-filing" type="submit">Submit filing</button>
  </form>
  <a id="captcha-link" href="/captcha">verify identity</a>
</body></html>`;
}

const CAPTCHA = `<!doctype html><html><head><title>Verify</title></head><body>
  <h1>Please complete the CAPTCHA to continue</h1>
  <div class="g-recaptcha" data-sitekey="fixture"></div>
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
