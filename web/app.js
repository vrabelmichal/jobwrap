// jobwrap browser client (plain JS, no build step).
'use strict';

function b64decode(b64) {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

function appendBytes(node, bytes) {
  const text = new TextDecoder('utf-8', { fatal: false }).decode(bytes);
  const maxTerminalChars = 2 * 1024 * 1024;
  const combined = node.textContent + text;
  node.textContent = combined.length > maxTerminalChars
    ? combined.slice(combined.length - maxTerminalChars)
    : combined;
  node.scrollTop = node.scrollHeight;
}

async function postJson(url, body) {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({}));
    throw new Error((err.error && err.error.message) || res.statusText);
  }
  if (res.status === 204) return null;
  return res.json();
}

function formatState(state) {
  if (!state || typeof state === 'string') return state || 'unknown';
  if (state.type === 'exited') return `exited (${state.code})`;
  if (state.type === 'signaled') return `signaled (${state.signal})`;
  return state.type || 'unknown';
}

function stateType(state) {
  if (!state) return 'unknown';
  return typeof state === 'string' ? state : (state.type || 'unknown');
}

function stateTone(state) {
  switch (stateType(state)) {
    case 'running': return 'success';
    case 'stopped':
    case 'disconnected': return 'warning';
    case 'lost':
    case 'signaled': return 'danger';
    default: return 'neutral';
  }
}

function stateGroup(state) {
  switch (stateType(state)) {
    case 'registering':
    case 'running':
    case 'stopped': return 'active';
    case 'disconnected':
    case 'lost': return 'attention';
    case 'exited':
    case 'signaled': return 'finished';
    default: return 'attention';
  }
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

function setText(id, value) {
  const node = document.getElementById(id);
  if (!node) return;
  node.textContent = value === null || value === undefined || value === '' ? '—' : String(value);
}

function formatDate(value) {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

function formatDuration(startedAt, finishedAt) {
  if (!startedAt) return '—';
  const start = new Date(startedAt).getTime();
  const end = finishedAt ? new Date(finishedAt).getTime() : Date.now();
  if (!Number.isFinite(start) || !Number.isFinite(end) || end < start) return '—';
  let seconds = Math.floor((end - start) / 1000);
  const days = Math.floor(seconds / 86400);
  seconds %= 86400;
  const hours = Math.floor(seconds / 3600);
  seconds %= 3600;
  const minutes = Math.floor(seconds / 60);
  seconds %= 60;
  const parts = [];
  if (days) parts.push(`${days}d`);
  if (hours || days) parts.push(`${hours}h`);
  if (minutes || hours || days) parts.push(`${minutes}m`);
  parts.push(`${seconds}s`);
  return parts.join(' ');
}

function formatBytes(value) {
  if (value === null || value === undefined) return '—';
  const number = Number(value);
  if (!Number.isFinite(number) || number < 0) return '—';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let scaled = number;
  let unit = 0;
  while (scaled >= 1024 && unit < units.length - 1) {
    scaled /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2;
  return `${scaled.toFixed(digits)} ${units[unit]}`;
}

function shortJobId(id) {
  const value = String(id || '');
  return value.length > 12 ? `${value.slice(0, 8)}…${value.slice(-4)}` : value;
}

const jobListState = {
  jobs: [],
  filter: 'all',
  query: '',
};

function jobMatchesQuery(job, query) {
  if (!query) return true;
  const haystack = [
    job.display_name,
    job.profile_name,
    job.id,
    job.child_pid,
    job.wrapper_pid,
    job.visibility,
    formatState(job.state),
  ].filter((value) => value !== null && value !== undefined).join(' ').toLowerCase();
  return haystack.includes(query);
}

function updateJobSummary(jobs) {
  const counts = { all: jobs.length, active: 0, attention: 0, finished: 0 };
  for (const job of jobs) counts[stateGroup(job.state)] += 1;
  setText('summary-all', counts.all);
  setText('summary-active', counts.active);
  setText('summary-attention', counts.attention);
  setText('summary-finished', counts.finished);
}

function renderJobList() {
  const container = document.getElementById('job-list');
  if (!container) return;

  const jobs = [...jobListState.jobs].sort((a, b) => {
    const aTime = new Date(a.started_at || 0).getTime();
    const bTime = new Date(b.started_at || 0).getTime();
    return bTime - aTime;
  });
  updateJobSummary(jobs);

  const visible = jobs.filter((job) => {
    const filterOk = jobListState.filter === 'all' || stateGroup(job.state) === jobListState.filter;
    return filterOk && jobMatchesQuery(job, jobListState.query);
  });

  const meta = document.getElementById('job-list-meta');
  if (meta) {
    meta.textContent = visible.length === jobs.length
      ? `${jobs.length} job${jobs.length === 1 ? '' : 's'}`
      : `Showing ${visible.length} of ${jobs.length} jobs`;
  }

  if (!jobs.length) {
    container.innerHTML = '<div class="empty-state"><strong>No jobs yet</strong><p>Wrap a command from the terminal or create a job from the web interface.</p><a href="/jobs/new" class="primary-link inline-action">+ New job</a></div>';
    return;
  }
  if (!visible.length) {
    container.innerHTML = '<div class="empty-state"><strong>No matching jobs</strong><p>Try another search or state filter.</p></div>';
    return;
  }

  let html = '<table class="jobs"><thead><tr>' +
    '<th>Job</th><th>State</th><th>Process</th><th>Started</th><th>Runtime</th><th>Output</th></tr></thead><tbody>';
  for (const job of visible) {
    const terminal = job.terminal || {};
    const pid = job.child_pid === null || job.child_pid === undefined ? '—' : job.child_pid;
    const output = formatBytes(job.output_bytes);
    const truncated = job.log_truncated ? '<span class="row-warning">truncated</span>' : '';
    const visibility = job.visibility ? escapeHtml(job.visibility) : '—';
    html += `<tr>` +
      `<td data-label="Job"><a class="job-name" href="/jobs/${encodeURIComponent(job.id)}">${escapeHtml(job.display_name)}</a>` +
        `<span class="row-subtitle">${escapeHtml(job.profile_name)} · ${escapeHtml(shortJobId(job.id))}</span></td>` +
      `<td data-label="State"><span class="status-pill ${stateTone(job.state)}">${escapeHtml(formatState(job.state))}</span>` +
        `<span class="row-subtitle">${visibility}</span></td>` +
      `<td data-label="Process"><span class="row-value">PID ${escapeHtml(pid)}</span>` +
        `<span class="row-subtitle">${terminal.attached ? 'terminal attached' : 'no terminal at launch'}</span></td>` +
      `<td data-label="Started"><span class="row-value">${escapeHtml(formatDate(job.started_at))}</span></td>` +
      `<td data-label="Runtime"><span class="row-value tabular">${escapeHtml(formatDuration(job.started_at, job.finished_at))}</span></td>` +
      `<td data-label="Output"><span class="row-value tabular">${escapeHtml(output)}</span>${truncated}</td>` +
      `</tr>`;
  }
  html += '</tbody></table>';
  container.innerHTML = html;
}

function setJobFilter(filter) {
  jobListState.filter = filter;
  document.querySelectorAll('[data-job-filter]').forEach((button) => {
    const selected = button.dataset.jobFilter === filter;
    button.classList.toggle('is-selected', selected);
    button.setAttribute('aria-pressed', selected ? 'true' : 'false');
  });
  renderJobList();
}

async function refreshJobs() {
  const refreshStatus = document.getElementById('jobs-refresh-status');
  const refreshButton = document.getElementById('refresh-jobs');
  if (refreshButton) refreshButton.disabled = true;
  try {
    const response = await fetch('/api/v1/jobs', { cache: 'no-store' });
    if (!response.ok) throw new Error(response.statusText || 'could not load jobs');
    jobListState.jobs = await response.json();
    renderJobList();
    if (refreshStatus) refreshStatus.textContent = 'refreshes every 5 s';
  } catch (error) {
    if (refreshStatus) refreshStatus.textContent = `refresh failed: ${error.message}`;
  } finally {
    if (refreshButton) refreshButton.disabled = false;
  }
}

function initJobList() {
  const container = document.getElementById('job-list');
  if (!container) return;
  try {
    jobListState.jobs = JSON.parse(container.dataset.jobs || '[]');
  } catch (error) {
    jobListState.jobs = [];
  }
  renderJobList();

  const search = document.getElementById('job-search');
  if (search) {
    search.addEventListener('input', () => {
      jobListState.query = search.value.trim().toLowerCase();
      renderJobList();
    });
  }
  document.querySelectorAll('[data-job-filter]').forEach((button) => {
    button.addEventListener('click', () => setJobFilter(button.dataset.jobFilter || 'all'));
  });
  const refreshButton = document.getElementById('refresh-jobs');
  if (refreshButton) refreshButton.addEventListener('click', refreshJobs);

  window.setInterval(() => {
    if (!document.hidden) refreshJobs();
  }, 5000);
}

async function refreshAuthControls() {
  const containers = document.querySelectorAll('[data-auth-controls]');
  if (!containers.length) return;
  let authenticated = false;
  try {
    const response = await fetch('/api/v1/auth', { cache: 'no-store' });
    if (!response.ok) throw new Error(response.statusText);
    const data = await response.json();
    authenticated = Boolean(data.authenticated);
  } catch (error) {
    containers.forEach((container) => {
      container.innerHTML = '<span class="status-pill warning">Session unavailable</span>';
    });
    return;
  }

  containers.forEach((container) => {
    if (authenticated) {
      container.innerHTML = '<span class="status-pill success">Authenticated</span><button type="button" class="header-text-button" data-logout>Log out</button>';
    } else {
      container.innerHTML = '<span class="status-pill neutral">Not authenticated</span><a href="/login" class="header-action login-link">Log in</a>';
    }
  });

  document.querySelectorAll('[data-logout]').forEach((button) => {
    button.addEventListener('click', async () => {
      button.disabled = true;
      try {
        await postJson('/api/v1/logout', {});
        window.location.href = '/';
      } catch (error) {
        button.textContent = 'Logout failed';
        button.disabled = false;
      }
    });
  });
}

function setConnection(text, tone) {
  const node = document.getElementById('connection');
  if (!node) return;
  node.textContent = text;
  node.className = `status-pill ${tone || 'neutral'}`;
}

function renderPermissions(permissions) {
  document.querySelectorAll('.controls button[data-permission]').forEach((button) => {
    const allowed = Boolean(permissions && permissions[button.dataset.permission]);
    button.disabled = !allowed;
    button.title = allowed ? '' : 'Not allowed by this job\'s access policy for the current session.';
  });
}

function renderProcess(process, permissions) {
  const empty = document.getElementById('process-empty');
  const grid = document.getElementById('process-grid');
  const paths = document.querySelector('.process-paths');
  if (!process) {
    if (empty) empty.hidden = false;
    if (grid) grid.hidden = true;
    if (paths) paths.hidden = true;
    return;
  }

  if (empty) empty.hidden = true;
  if (grid) grid.hidden = false;
  if (paths) paths.hidden = false;
  setText('process-state', process.state);
  setText('process-ppid', process.parent_pid);
  setText('process-threads', process.threads);
  setText('process-rss', formatBytes(process.resident_memory_bytes));
  setText('process-vmsize', formatBytes(process.virtual_memory_bytes));
  setText('process-fds', process.open_file_descriptors);
  setText('process-read', formatBytes(process.read_bytes));
  setText('process-write', formatBytes(process.write_bytes));

  setText(
    'process-executable',
    permissions && permissions.view_command
      ? (process.current_executable || 'unavailable')
      : 'not available with current access'
  );
  setText(
    'process-command',
    permissions && permissions.view_command
      ? (process.current_command || 'unavailable')
      : 'not available with current access'
  );
  setText(
    'process-cwd',
    permissions && permissions.view_working_directory
      ? (process.current_working_directory || 'unavailable')
      : 'not available with current access'
  );
}

function renderJobDetails(data) {
  setText('job-id', data.id);
  setText('job-profile', data.profile);
  setText('job-started', formatDate(data.started_at));
  setText('job-finished', data.finished_at ? formatDate(data.finished_at) : 'running');
  setText('job-runtime', formatDuration(data.started_at, data.finished_at));
  setText('job-child-pid', data.child_pid);
  setText('job-wrapper-pid', data.wrapper_pid);
  setText('job-pgid', data.process_group_id);

  const stateNode = document.getElementById('job-state');
  if (stateNode) {
    stateNode.textContent = formatState(data.state);
    stateNode.className = `state status-pill ${stateTone(data.state)}`;
  }

  const command = document.getElementById('job-command');
  const copyButton = document.getElementById('copy-command');
  if (command) command.textContent = data.command || 'Not available with current access.';
  if (copyButton) {
    copyButton.disabled = !data.command;
    copyButton.dataset.command = data.command || '';
  }
  setText('job-executable', data.executable || 'Not available with current access.');
  setText('job-working-directory', data.working_directory || 'Not available with current access.');

  const log = data.log || {};
  setText('log-size', formatBytes(log.bytes_written));
  const truncated = document.getElementById('log-truncated');
  if (truncated) truncated.hidden = !log.truncated;

  const terminal = data.terminal || {};
  setText('terminal-attached', terminal.attached ? 'attached at launch' : 'not attached at launch');
  const deviceWrap = document.getElementById('terminal-device-wrap');
  if (deviceWrap) deviceWrap.hidden = !terminal.device;
  if (terminal.device) setText('terminal-device', terminal.device);
  const sizeWrap = document.getElementById('terminal-size-wrap');
  const size = terminal.initial_size;
  if (sizeWrap) sizeWrap.hidden = !size;
  if (size) setText('terminal-size', `${size.cols}×${size.rows}`);

  renderPermissions(data.permissions || {});
  renderProcess(data.process, data.permissions || {});
}

async function fetchJobDetails(jobId) {
  const res = await fetch(`/api/v1/jobs/${jobId}/details`, { cache: 'no-store' });
  if (!res.ok) {
    const err = await res.json().catch(() => ({}));
    throw new Error((err.error && err.error.message) || res.statusText);
  }
  return res.json();
}

async function refreshJobDetails(jobId) {
  const refresh = document.getElementById('details-refresh');
  try {
    const data = await fetchJobDetails(jobId);
    renderJobDetails(data);
    if (refresh) refresh.textContent = 'refreshes every 5 s';
    return data;
  } catch (error) {
    if (refresh) refresh.textContent = `refresh failed: ${error.message}`;
    return null;
  }
}

async function connectTerminal(jobId) {
  const terminal = document.getElementById('terminal');
  if (!terminal) return;

  setConnection('Live output: loading history…', 'neutral');
  try {
    const response = await fetch(`/api/v1/jobs/${jobId}/output?from=0`, { cache: 'no-store' });
    if (!response.ok) throw new Error(response.statusText || 'output unavailable');
    const data = await response.json();
    if (data.data_base64) appendBytes(terminal, b64decode(data.data_base64));
  } catch (error) {
    terminal.textContent = 'Terminal output is not available for the current session.';
    setConnection('Live output: unavailable', 'warning');
    return;
  }

  const protocol = window.location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${protocol}://${window.location.host}/api/v1/jobs/${jobId}/ws`);
  setConnection('Live output: connecting…', 'neutral');
  ws.onopen = () => { setConnection('Live output: connected', 'success'); };
  ws.onerror = () => { setConnection('Live output: connection error', 'warning'); };
  ws.onclose = () => {
    setConnection('Live output: disconnected', 'neutral');
  };
  ws.onmessage = (event) => {
    let msg;
    try { msg = JSON.parse(event.data); } catch (error) { return; }
    if (msg.type === 'output') {
      appendBytes(terminal, b64decode(msg.data_base64));
    } else if (msg.type === 'state_changed') {
      const stateEl = document.getElementById('job-state');
      if (stateEl) {
        stateEl.textContent = formatState(msg.state);
        stateEl.className = `state status-pill ${stateTone(msg.state)}`;
      }
      refreshJobDetails(jobId);
    }
  };
}

function bindSignalControls(jobId) {
  const feedback = document.getElementById('control-feedback');
  document.querySelectorAll('.controls button[data-signal]').forEach((button) => {
    button.addEventListener('click', async () => {
      const signal = button.dataset.signal;
      const signalLabel = button.dataset.signalLabel || signal.toUpperCase();
      if (signal === 'term' || signal === 'kill') {
        const action = signal === 'kill' ? 'immediately kill' : 'request termination of';
        if (!window.confirm(`${signalLabel} will ${action} this job. Continue?`)) return;
      }

      button.disabled = true;
      if (feedback) feedback.textContent = `Sending ${signalLabel}…`;
      try {
        await postJson(`/api/v1/jobs/${jobId}/signals/${signal}`, {});
        if (feedback) feedback.textContent = `${signalLabel} sent.`;
      } catch (error) {
        if (feedback) feedback.textContent = `Could not send ${signalLabel}: ${error.message}`;
      } finally {
        await refreshJobDetails(jobId);
      }
    });
  });
}

function jobUrl(jobId) {
  return `/jobs/${encodeURIComponent(jobId)}`;
}

function createIdempotencyKey() {
  if (typeof window.crypto.randomUUID === 'function') {
    return window.crypto.randomUUID();
  }
  const bytes = new Uint8Array(16);
  window.crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function initLaunchForm() {
  const form = document.getElementById('launch-form');
  if (!form) return;
  const errorEl = document.getElementById('launch-error');
  const statusEl = document.getElementById('launch-status');
  const button = document.getElementById('launch-button');

  async function waitForJob(jobId) {
    statusEl.textContent = 'launch accepted; waiting for the terminal to connect…';
    for (let attempt = 0; attempt < 40; attempt++) {
      const res = await fetch(`/api/v1/jobs/${encodeURIComponent(jobId)}`);
      if (res.ok) {
        window.location.href = jobUrl(jobId);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    statusEl.textContent = '';
    errorEl.textContent =
      'the launch was accepted but the job did not register; ' +
      'the terminal may have failed to open';
    const link = document.createElement('a');
    link.href = jobUrl(jobId);
    link.textContent = 'open job page';
    statusEl.appendChild(link);
  }

  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    errorEl.textContent = '';
    statusEl.textContent = '';
    const executable = document.getElementById('executable').value.trim();
    if (!executable) {
      errorEl.textContent = 'an executable is required';
      return;
    }
    const args = document.getElementById('arguments').value
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => line.length > 0);
    const workingDirectory = document.getElementById('working-directory').value.trim();
    const displayName = document.getElementById('display-name').value.trim();
    const profile = document.getElementById('profile').value;
    const backendSelect = document.getElementById('backend');
    const body = {
      executable,
      arguments: args,
      idempotency_key: createIdempotencyKey(),
      terminal_target: {
        type: 'new_terminal',
        backend: backendSelect ? backendSelect.value : null,
      },
    };
    if (workingDirectory) body.working_directory = workingDirectory;
    if (displayName) body.display_name = displayName;
    if (profile) body.profile_name = profile;

    button.disabled = true;
    try {
      const data = await postJson('/api/v1/launch', body);
      await waitForJob(data.job_id);
    } catch (error) {
      errorEl.textContent = error.message;
    } finally {
      button.disabled = false;
    }
  });
}

function initLoginForm() {
  const form = document.getElementById('login-form');
  if (!form) return;
  const errorEl = document.getElementById('login-error');
  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    errorEl.textContent = '';
    const password = document.getElementById('password').value;
    try {
      await postJson('/api/v1/login', { password });
      window.location.href = '/';
    } catch (error) {
      errorEl.textContent = 'Incorrect password.';
    }
  });
}

function bindCopyCommand() {
  const button = document.getElementById('copy-command');
  const feedback = document.getElementById('control-feedback');
  if (!button) return;
  button.addEventListener('click', async () => {
    if (!button.dataset.command) return;
    try {
      await navigator.clipboard.writeText(button.dataset.command);
      const original = button.textContent;
      button.textContent = 'Copied';
      window.setTimeout(() => { button.textContent = original; }, 1200);
    } catch (error) {
      if (feedback) feedback.textContent = `Could not copy command: ${error.message}`;
    }
  });
}

async function initJobDetailsPage() {
  const root = document.getElementById('job-details');
  if (!root) return;
  const jobId = root.dataset.job;
  bindSignalControls(jobId);
  bindCopyCommand();

  const initial = await refreshJobDetails(jobId);
  if (initial && initial.permissions && initial.permissions.view_output) {
    connectTerminal(jobId);
  } else {
    const terminal = document.getElementById('terminal');
    if (terminal) terminal.textContent = 'Terminal output is not available with the current access policy.';
    setConnection('Live output: not permitted', 'neutral');
  }

  window.setInterval(() => {
    if (!document.hidden) refreshJobDetails(jobId);
  }, 5000);
}

initJobList();
if (document.getElementById('job-details')) initJobDetailsPage();
initLaunchForm();
initLoginForm();
refreshAuthControls();
