'use strict';

const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { test } = require('node:test');
const vm = require('node:vm');

const source = readFileSync(`${__dirname}/app.js`, 'utf8');
const jobPath = '/jobs/01M4C70AH3D4R07FZJHRTYNPRA';

function browser({ returnTo, pathname = jobPath, loginSucceeds = true } = {}) {
  let submit;
  const form = {
    dataset: returnTo === undefined ? {} : { returnTo },
    addEventListener: (event, handler) => { if (event === 'submit') submit = handler; },
  };
  const error = { textContent: '' };
  const link = { href: '' };
  const auth = { innerHTML: '', querySelector: () => link };
  const location = { pathname, href: pathname };
  const requests = [];
  const context = {
    document: {
      getElementById: (id) => ({
        'login-form': form,
        'login-error': error,
        password: { value: 'test-password' },
      }[id] || null),
      querySelectorAll: (selector) => selector === '[data-auth-controls]' ? [auth] : [],
    },
    window: { location },
    fetch: async (url, options) => {
      requests.push({ url, options });
      const ok = url !== '/api/v1/login' || loginSucceeds;
      return {
        ok, status: ok ? 200 : 401,
        json: async () => url === '/api/v1/auth'
          ? { authenticated: false }
          : ok ? { ok: true } : { error: { message: 'incorrect password' } },
      };
    },
  };
  vm.runInNewContext(source, context);
  return { context, location, link, error, requests, submit: () => submit({ preventDefault() {} }) };
}

test('successful login returns to the requested private job', async () => {
  const page = browser({ returnTo: jobPath, pathname: '/login' });
  await page.submit();
  assert.equal(page.location.href, jobPath);
  const request = page.requests.find(({ url }) => url === '/api/v1/login');
  assert.equal(request.options.method, 'POST');
});

test('ordinary login returns to the dashboard', async () => {
  const page = browser({ pathname: '/login' });
  await page.submit();
  assert.equal(page.location.href, '/');
});

test('failed login stays on the login page with an error', async () => {
  const page = browser({ returnTo: jobPath, pathname: '/login', loginSucceeds: false });
  await page.submit();
  assert.equal(page.location.href, '/login');
  assert.equal(page.error.textContent, 'Incorrect password.');
});

test('authentication header preserves the job URL in its login link', async () => {
  const page = browser();
  await vm.runInNewContext('refreshAuthControls()', page.context);
  assert.equal(page.link.href, `/login?return_to=${encodeURIComponent(jobPath)}`);
});

test('login header retains the original destination rather than the login page', async () => {
  const page = browser({ returnTo: jobPath, pathname: '/login' });
  await vm.runInNewContext('refreshAuthControls()', page.context);
  assert.equal(page.link.href, `/login?return_to=${encodeURIComponent(jobPath)}`);
});
