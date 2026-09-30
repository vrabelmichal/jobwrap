// jobwrap browser client (plain JS, no build step).
'use strict';

function b64decode(b64) {
  // Atob on a binary string then map to bytes.
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
  return res.json();
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

function connectTerminal() {
  const terminal = document.getElementById('terminal');
  if (!terminal) return;
  const jobId = terminal.dataset.job;
  const stateEl = document.getElementById('job-state');
  const connEl = document.getElementById('connection');

  // Load history, then subscribe for live output.
  fetch(`/api/v1/jobs/${jobId}/output?from=0`)
    .then((r) => r.json())
    .then((data) => {
      if (data.data_base64) appendBytes(terminal, b64decode(data.data_base64));
    })
    .catch(() => { /* output may be private */ });

  const protocol = window.location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${protocol}://${window.location.host}/api/v1/jobs/${jobId}/ws`);
  connEl.textContent = 'connecting…';
  ws.onopen = () => { connEl.textContent = 'live'; };
  ws.onclose = () => { connEl.textContent = 'disconnected'; };
  ws.onmessage = (event) => {
    let msg;
    try { msg = JSON.parse(event.data); } catch (e) { return; }
    if (msg.type === 'output') {
      const bytes = b64decode(msg.data_base64);
      appendBytes(terminal, bytes);
    } else if (msg.type === 'state_changed' && stateEl) {
      stateEl.textContent = formatState(msg.state);
    }
  };

  document.querySelectorAll('.controls button').forEach((button) => {
    button.addEventListener('click', () => {
      const signal = button.dataset.signal;
      const label = button.textContent;
      if (['term', 'kill'].includes(signal)) {
        if (!window.confirm(`Send ${label} to ${jobId}?`)) return;
      }
      postJson(`/api/v1/jobs/${jobId}/signals/${signal}`, {})
        .catch((e) => { connEl.textContent = `error: ${e.message}`; });
    });
  });
}

function jobUrl(jobId) {
  return `/jobs/${encodeURIComponent(jobId)}`;
}

function initLaunchForm() {
  const form = document.getElementById('launch-form');
  if (!form) return;
  const errorEl = document.getElementById('launch-error');
  const statusEl = document.getElementById('launch-status');
  const button = document.getElementById('launch-button');

  // Poll briefly until the wrapper in the new terminal has registered the
  // job, then open its page. Launch accepted + not yet registered is normal:
  // the terminal window has to start first.
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
    // Arguments are submitted one per line: structured, never shell-parsed.
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
      idempotency_key: crypto.randomUUID(),
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
    } catch (e) {
      errorEl.textContent = e.message;
    } finally {
      button.disabled = false;
    }
  });
}

if (document.getElementById('job-list')) renderJobList();
if (document.getElementById('terminal')) connectTerminal();
initLaunchForm();
