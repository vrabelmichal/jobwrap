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
}

function renderJobList() {
  const container = document.getElementById('job-list');
  if (!container) return;
  const jobs = JSON.parse(container.dataset.jobs || '[]');
  if (!jobs.length) {
    container.innerHTML = '<p class="muted">no jobs</p>';
    return;
  }
  let html = '<table class="jobs"><thead><tr>' +
    '<th>name</th><th>state</th><th>profile</th><th>started</th></tr></thead><tbody>';
  for (const job of jobs) {
    html += `<tr><td><a href="/jobs/${job.id}">${escapeHtml(job.display_name)}</a></td>` +
      `<td>${escapeHtml(formatState(job.state))}</td><td>${escapeHtml(job.profile_name)}</td>` +
      `<td>${escapeHtml(job.started_at || '')}</td></tr>`;
  }
  html += '</tbody></table>';
  container.innerHTML = html;
}

function formatState(state) {
  if (!state || typeof state === 'string') return state || 'unknown';
  if (state.type === 'exited') return `exited (${state.code})`;
  if (state.type === 'signaled') return `signaled (${state.signal})`;
  return state.type || 'unknown';
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
  if (stateNode) stateNode.textContent = formatState(data.state);

  const command = document.getElementById('job-command');
  const copyButton = document.getElementById('copy-command');
  if (command) {
    command.textContent = data.command || 'Not available with current access.';
  }
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
    setConnection('Live output: offline (job state is shown above)', 'neutral');
  };
  ws.onmessage = (event) => {
    let msg;
    try { msg = JSON.parse(event.data); } catch (error) { return; }
    if (msg.type === 'output') {
      appendBytes(terminal, b64decode(msg.data_base64));
    } else if (msg.type === 'state_changed') {
      const stateEl = document.getElementById('job-state');
      if (stateEl) stateEl.textContent = formatState(msg.state);
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

if (document.getElementById('job-list')) renderJobList();
if (document.getElementById('job-details')) initJobDetailsPage();
