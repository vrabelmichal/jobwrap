//! Embedded server-rendered HTML and plain JavaScript.
//!
//! No Node build pipeline: assets ship inside the binary so the daemon and
//! browser always agree on versions.

use jobwrap_core::JobRecord;

pub const APP_JS: &str = include_str!("../../../web/app.js");
pub const APP_CSS: &str = include_str!("../../../web/app.css");

pub fn index_page(jobs_json: &str, authenticated: bool) -> String {
    let auth_badge = if authenticated {
        "authenticated"
    } else {
        r#"<a href="/login" class="login-link">log in</a>"#
    };
    let jobs_json = html_attribute_escape(jobs_json);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>jobwrap</title>
<link rel="stylesheet" href="/static/app.css">
</head>
<body>
<header><h1>jobwrap</h1><span class="auth-badge">{auth_badge}</span></header>
<main>
<div id="job-list" data-jobs='{jobs_json}'></div>
<noscript><p>JavaScript is required to view live job data.</p></noscript>
</main>
<script src="/static/app.js"></script>
</body>
</html>"#
    )
}

pub fn job_page(record: &JobRecord) -> String {
    let display_name = html_escape(record.display_name.as_str());
    let state = html_escape(&record.state.describe());
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>jobwrap: {display_name}</title>
<link rel="stylesheet" href="/static/app.css">
</head>
<body>
<header class="job-header">
  <a href="/" class="back-link">&larr; jobs</a>
  <div class="job-header-title">
    <span class="eyebrow">job details</span>
    <h1>{display_name}</h1>
  </div>
  <span id="job-state" class="state status-pill">{state}</span>
</header>
<main id="job-details" class="job-main" data-job="{}">
  <div class="details-layout">
    <section class="card overview-card">
      <div class="section-heading">
        <div>
          <span class="eyebrow">record</span>
          <h2>Overview</h2>
        </div>
        <span id="details-refresh" class="section-meta">refreshes every 5 s</span>
      </div>
      <dl class="detail-grid">
        <div><dt>Job ID</dt><dd id="job-id">{}</dd></div>
        <div><dt>Profile</dt><dd id="job-profile">—</dd></div>
        <div><dt>Started</dt><dd id="job-started">—</dd></div>
        <div><dt>Finished</dt><dd id="job-finished">—</dd></div>
        <div><dt>Runtime</dt><dd id="job-runtime">—</dd></div>
        <div><dt>Child PID</dt><dd id="job-child-pid">—</dd></div>
        <div><dt>Wrapper PID</dt><dd id="job-wrapper-pid">—</dd></div>
        <div><dt>Process group</dt><dd id="job-pgid">—</dd></div>
      </dl>
    </section>

    <section class="card process-card">
      <div class="section-heading">
        <div>
          <span class="eyebrow">live Linux process</span>
          <h2>Process snapshot</h2>
        </div>
        <span class="section-meta">from /proc</span>
      </div>
      <p id="process-empty" class="muted process-empty" hidden>No live process snapshot is available. This is expected for completed, disconnected, or lost jobs.</p>
      <dl id="process-grid" class="detail-grid compact-grid">
        <div><dt>Process state</dt><dd id="process-state">—</dd></div>
        <div><dt>Parent PID</dt><dd id="process-ppid">—</dd></div>
        <div><dt>Threads</dt><dd id="process-threads">—</dd></div>
        <div><dt>Resident memory</dt><dd id="process-rss">—</dd></div>
        <div><dt>Virtual memory</dt><dd id="process-vmsize">—</dd></div>
        <div><dt>Open file descriptors</dt><dd id="process-fds">—</dd></div>
        <div><dt>Disk read</dt><dd id="process-read">—</dd></div>
        <div><dt>Disk written</dt><dd id="process-write">—</dd></div>
      </dl>
      <div class="process-paths">
        <div><span class="field-label">Current executable</span><code id="process-executable">—</code></div>
        <div><span class="field-label">Current working directory</span><code id="process-cwd">—</code></div>
        <div><span class="field-label">Current argv</span><code id="process-command" class="wrap-code">—</code></div>
      </div>
    </section>

    <section class="card command-card full-width">
      <div class="section-heading">
        <div>
          <span class="eyebrow">launch configuration</span>
          <h2>Command</h2>
        </div>
        <button id="copy-command" class="secondary-button" type="button" disabled>Copy command</button>
      </div>
      <pre id="job-command" class="command-block">Loading…</pre>
      <div class="command-meta">
        <div><span class="field-label">Executable</span><code id="job-executable">—</code></div>
        <div><span class="field-label">Launch working directory</span><code id="job-working-directory">—</code></div>
      </div>
    </section>

    <section class="card terminal-card full-width">
      <div class="section-heading terminal-heading">
        <div>
          <span class="eyebrow">recorded + live</span>
          <h2>Terminal output</h2>
        </div>
        <span id="connection" class="status-pill neutral" title="This is the browser's live output connection, separate from the job state.">Live output: checking…</span>
      </div>
      <pre id="terminal" class="terminal" data-job="{}"></pre>
      <div class="terminal-meta">
        <span>Recorded: <strong id="log-size">—</strong></span>
        <span id="log-truncated" hidden>recording truncated at configured limit</span>
        <span>Terminal: <strong id="terminal-attached">—</strong></span>
        <span id="terminal-device-wrap" hidden>Device: <code id="terminal-device"></code></span>
        <span id="terminal-size-wrap" hidden>Initial size: <strong id="terminal-size"></strong></span>
      </div>
    </section>

    <section class="card controls-card full-width">
      <div class="section-heading">
        <div>
          <span class="eyebrow">process control</span>
          <h2>Signals</h2>
        </div>
        <span class="section-meta">disabled buttons are not allowed by this job's access policy</span>
      </div>
      <div class="controls signal-grid">
        <button data-signal="int" data-permission="send_interrupt" data-signal-label="SIGINT">
          <strong>Interrupt</strong><span>SIGINT · like Ctrl+C</span>
        </button>
        <button data-signal="term" data-permission="send_terminate" data-signal-label="SIGTERM">
          <strong>Terminate</strong><span>SIGTERM · graceful stop request</span>
        </button>
        <button data-signal="stop" data-permission="send_stop" data-signal-label="SIGSTOP">
          <strong>Pause</strong><span>SIGSTOP · suspend execution</span>
        </button>
        <button data-signal="cont" data-permission="send_continue" data-signal-label="SIGCONT">
          <strong>Resume</strong><span>SIGCONT · continue a paused job</span>
        </button>
        <button data-signal="kill" data-permission="send_kill" data-signal-label="SIGKILL" class="danger-control">
          <strong>Kill</strong><span>SIGKILL · immediate, cannot be handled</span>
        </button>
      </div>
      <p id="control-feedback" class="inline-feedback" aria-live="polite"></p>
    </section>
  </div>
  <noscript><p>JavaScript is required for process details and live output.</p></noscript>
</main>
<script src="/static/app.js"></script>
</body>
</html>"#,
        record.id, record.id, record.id
    )
}

pub fn login_page() -> String {
    r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>jobwrap login</title>
<link rel="stylesheet" href="/static/app.css">
</head>
<body>
<header><h1>jobwrap</h1></header>
<main>
<form id="login-form" class="login-form">
  <label for="password">Password</label>
  <input type="password" id="password" name="password" autocomplete="current-password">
  <button type="submit">Log in</button>
  <p id="login-error" class="error"></p>
</form>
</main>
<script>
document.getElementById('login-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const password = document.getElementById('password').value;
  const res = await fetch('/api/v1/login', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ password }),
  });
  if (res.ok) { window.location.href = '/'; }
  else { document.getElementById('login-error').textContent = 'incorrect password'; }
});
</script>
</body>
</html>"#
        .to_string()
}

pub fn not_found_page(id: &str) -> String {
    let id = html_escape(id);
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>not found</title>
<link rel="stylesheet" href="/static/app.css"></head><body><main><h1>job not found</h1>
<p>No job with id <code>{id}</code> exists.</p><p><a href="/">back to jobs</a></p></main></body></html>"#
    )
}

pub fn error_page(message: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>error</title>
<link rel="stylesheet" href="/static/app.css"></head><body><main><h1>error</h1>
<p>{}</p><p><a href="/">back to jobs</a></p></main></body></html>"#,
        html_escape(message)
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn html_attribute_escape(s: &str) -> String {
    html_escape(s).replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_json_cannot_escape_its_attribute() {
        let page = index_page(r#"[{"display_name":"' onmouseover='alert(1)"}]"#, false);
        assert!(!page.contains("data-jobs='[{\"display_name\":\"' onmouseover="));
        assert!(page.contains("&#39; onmouseover=&#39;"));
    }
}
