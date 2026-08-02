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
  node.textContent += text;
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
      `<td>${escapeHtml(job.state)}</td><td>${escapeHtml(job.profile_name)}</td>` +
      `<td>${escapeHtml(job.started_at || '')}</td></tr>`;
  }
  html += '</tbody></table>';
  container.innerHTML = html;
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
      stateEl.textContent = msg.state.type;
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

if (document.getElementById('job-list')) renderJobList();
if (document.getElementById('terminal')) connectTerminal();
