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

function element(extra = {}) {
  const handlers = {};
  return {
    value: '', checked: false, disabled: false, title: '', textContent: '', innerHTML: '', dataset: {}, children: [],
    classList: { toggle() {} },
    setAttribute() {},
    append(...nodes) { this.children.push(...nodes); },
    addEventListener(event, handler) { handlers[event] = handler; },
    async fire(event) {
      if (handlers[event]) handlers[event]({ preventDefault() {} });
      for (let i = 0; i < 5; i++) await new Promise((resolve) => setImmediate(resolve));
    },
    ...extra,
  };
}

function job(id, name) {
  return {
    id, display_name: name, profile_name: 'standard', state: { type: 'exited', code: 0 },
    started_at: '2026-10-30T10:00:00Z', finished_at: '2026-10-30T11:00:00Z', terminal: {},
    child_pid: 10, wrapper_pid: 9, output_bytes: 0, log_truncated: false, visibility: 'public',
  };
}

function dashboard({ search = '', page, responses = [] } = {}) {
  const initial = page || { jobs: [job('A', 'first')], total: 1, offset: 0, limit: 50, counts: { all: 1, active: 0, attention: 0, finished: 1 }, hidden_finished: 3 };
  const ids = ['job-list', 'job-list-meta', 'job-search', 'started-from', 'started-to', 'finished-from', 'finished-to',
    'show-old-jobs', 'clear-job-filters', 'page-size', 'page-prev', 'page-next', 'page-status', 'refresh-jobs',
    'jobs-refresh-status', 'summary-all', 'summary-active', 'summary-attention', 'summary-finished'];
  const nodes = Object.fromEntries(ids.map((id) => [id, element()]));
  nodes['job-list'].dataset = { page: JSON.stringify(initial), recentDays: '7', pageSize: '50' };
  const filters = ['all', 'active', 'attention', 'finished'].map((filter) => element({ dataset: { jobFilter: filter } }));
  const requests = [];
  const urls = [];
  const queue = [...responses];
  const context = {
    URLSearchParams,
    document: {
      hidden: false,
      getElementById: (id) => nodes[id] || null,
      querySelectorAll: (selector) => selector === '[data-job-filter]' ? filters : [],
      createElement: () => element(),
    },
    window: {
      location: { pathname: '/', search },
      history: { replaceState: (_state, _title, url) => urls.push(url) },
      setInterval() {},
      setTimeout: (fn) => { fn(); return 1; },
      clearTimeout() {},
    },
    fetch: async (url) => {
      requests.push(url);
      const body = url.startsWith('/api/v1/jobs/query') ? (queue.shift() || initial) : { authenticated: false };
      return { ok: true, status: 200, json: async () => body };
    },
  };
  vm.runInNewContext(source, context);
  const settle = async () => {
    for (let i = 0; i < 5; i++) await new Promise((resolve) => setImmediate(resolve));
  };
  const queries = () => requests.filter((url) => url.startsWith('/api/v1/jobs/query'))
    .map((url) => new URLSearchParams(url.split('?')[1]));
  return { nodes, filters, queries, urls, settle, context };
}

test('dashboard renders the embedded page without refetching', async () => {
  const page = dashboard();
  await page.settle();
  assert.equal(page.queries().length, 0);
  assert.match(page.nodes['job-list'].innerHTML, /first/);
  assert.match(page.nodes['job-list-meta'].textContent, /Showing 1–1 of 1 job · 3 finished more than 7 days ago hidden/);
  assert.equal(page.nodes['page-status'].textContent, 'Page 1 of 1');
  assert.equal(page.nodes['page-next'].disabled, true);
});

test('dashboard hides old finished jobs until asked', async () => {
  const page = dashboard();
  await page.nodes['refresh-jobs'].fire('click');
  assert.equal(page.queries()[0].get('hide_finished_older_than_days'), '7');
  page.nodes['show-old-jobs'].checked = true;
  await page.nodes['show-old-jobs'].fire('change');
  assert.equal(page.queries()[1].get('hide_finished_older_than_days'), null);
  assert.equal(page.urls.at(-1), '/?show_old=1');
});

test('date filters send local-day bounds and include older jobs', async () => {
  const page = dashboard();
  page.nodes['started-from'].value = '2026-10-01';
  await page.nodes['started-from'].fire('change');
  page.nodes['finished-to'].value = '2026-10-15';
  await page.nodes['finished-to'].fire('change');
  const query = page.queries().at(-1);
  assert.equal(query.get('started_after'), new Date(2026, 9, 1).toISOString());
  assert.equal(query.get('finished_before'), new Date(2026, 9, 16).toISOString());
  assert.equal(query.get('hide_finished_older_than_days'), null);
  assert.equal(page.nodes['show-old-jobs'].disabled, true);
  assert.equal(page.urls.at(-1), '/?started_from=2026-10-01&finished_to=2026-10-15');
});

test('pager moves between pages of the server result', async () => {
  const many = { jobs: [job('A', 'first')], total: 120, offset: 0, limit: 50, counts: { all: 120, active: 0, attention: 0, finished: 120 }, hidden_finished: 0 };
  const page = dashboard({ page: many, responses: [{ ...many, offset: 50 }] });
  assert.equal(page.nodes['page-status'].textContent, 'Page 1 of 3');
  assert.equal(page.nodes['page-prev'].disabled, true);
  await page.nodes['page-next'].fire('click');
  const query = page.queries()[0];
  assert.equal(query.get('offset'), '50');
  assert.equal(query.get('limit'), '50');
  assert.equal(page.nodes['page-status'].textContent, 'Page 2 of 3');
  assert.equal(page.nodes['page-prev'].disabled, false);
});

test('filters in the address bar are restored and fetched', async () => {
  const page = dashboard({ search: '?state=finished&q=build&per_page=25&offset=25&state=bogus' });
  await page.settle();
  const query = page.queries()[0];
  assert.equal(query.get('state'), 'finished');
  assert.equal(query.get('q'), 'build');
  assert.equal(query.get('limit'), '25');
  assert.equal(page.nodes['job-search'].value, 'build');
  assert.equal(page.nodes['page-size'].value, '25');
});

function prunePage({ confirmAnswer = true, jobs = [job('A', 'old-one'), job('B', 'old-two')] } = {}) {
  const nodes = Object.fromEntries(['prune-form', 'prune-days', 'prune-preview', 'prune-confirm', 'prune-result']
    .map((id) => [id, element()]));
  nodes['prune-days'].value = '30';
  nodes['prune-confirm'].hidden = true;
  const posts = [];
  const confirms = [];
  const context = {
    URLSearchParams,
    document: {
      hidden: false,
      getElementById: (id) => nodes[id] || null,
      querySelectorAll: () => [],
      createElement: () => element(),
    },
    window: {
      location: { pathname: '/', search: '' },
      history: { replaceState() {} },
      setInterval() {},
      setTimeout() {},
      clearTimeout() {},
      confirm: (message) => { confirms.push(message); return confirmAnswer; },
    },
    fetch: async (url, options = {}) => {
      if (options.method === 'POST') posts.push({ url, body: JSON.parse(options.body) });
      const body = url === '/api/v1/jobs/prune'
        ? { jobs, dry_run: JSON.parse(options.body).dry_run, freed_log_bytes: 2048 }
        : { authenticated: true };
      return { ok: true, status: 200, json: async () => body };
    },
  };
  vm.runInNewContext(source, context);
  return { nodes, posts, confirms };
}

test('prune previews before deleting and deletes the previewed age', async () => {
  const page = prunePage();
  await page.nodes['prune-form'].fire('submit');
  assert.deepEqual(page.posts[0], { url: '/api/v1/jobs/prune', body: { older_than_days: 30, dry_run: true } });
  assert.match(page.nodes['prune-result'].innerHTML, /2 finished jobs would be deleted/);
  assert.equal(page.nodes['prune-confirm'].hidden, false);
  assert.equal(page.nodes['prune-confirm'].textContent, 'Delete 2 jobs');
  await page.nodes['prune-confirm'].fire('click');
  assert.equal(page.confirms.length, 1);
  assert.deepEqual(page.posts[1].body, { older_than_days: 30, dry_run: false });
  assert.match(page.nodes['prune-result'].innerHTML, /Deleted 2 jobs and 2\.00 KiB of logs/);
  assert.equal(page.nodes['prune-confirm'].hidden, true);
});

test('prune does nothing when the confirmation is declined or the age changed', async () => {
  const declined = prunePage({ confirmAnswer: false });
  await declined.nodes['prune-form'].fire('submit');
  await declined.nodes['prune-confirm'].fire('click');
  assert.equal(declined.posts.length, 1);

  const changed = prunePage();
  await changed.nodes['prune-form'].fire('submit');
  changed.nodes['prune-days'].value = '1';
  await changed.nodes['prune-days'].fire('input');
  assert.equal(changed.nodes['prune-confirm'].hidden, true);
  await changed.nodes['prune-confirm'].fire('click');
  assert.equal(changed.posts.length, 1);
});

test('prune offers no deletion when nothing is old enough', async () => {
  const page = prunePage({ jobs: [] });
  await page.nodes['prune-form'].fire('submit');
  assert.match(page.nodes['prune-result'].innerHTML, /No finished jobs ended more than 30 days ago/);
  assert.equal(page.nodes['prune-confirm'].hidden, true);
});

test('Python environment filters round-trip through dashboard and API URLs', () => {
  const context = { document: { getElementById: () => null, querySelectorAll: () => [] }, window: { location: { pathname: '/' } }, fetch: async () => ({ ok: true, json: async () => ({}) }), URLSearchParams };
  vm.runInNewContext(source, context);
  vm.runInNewContext("restoreDashboardState('?environment=science'); globalThis.apiQuery = jobQueryParams(jobListState).toString(); globalThis.addressQuery = dashboardSearch(jobListState);", context);
  assert.match(context.apiQuery, /environment=science/);
  assert.match(context.addressQuery, /environment=science/);
});
