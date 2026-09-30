//! Embedded server-rendered HTML and plain JavaScript.
//!
//! No Node build pipeline: assets ship inside the binary so the daemon and
//! browser always agree on versions.

use crate::service::LaunchCapabilities;
use jobwrap_core::JobRecord;

pub const APP_JS: &str = include_str!("../../../web/app.js");
pub const APP_CSS: &str = include_str!("../../../web/app.css");

const AUTH_CONTROLS: &str = r#"<div class="auth-controls" data-auth-controls>
  <span class="status-pill neutral">Session: checking…</span>
</div>"#;

pub fn index_page(jobs_json: &str, _authenticated: bool) -> String {
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
<header class="app-header">
  <div class="header-inner">
    <div class="header-primary">
      <a href="/" class="brand-link">jobwrap</a>
    </div>
    <div class="header-actions">
      <a href="/jobs/new" class="primary-link">+ New job</a>
      {AUTH_CONTROLS}
    </div>
  </div>
</header>
<main class="index-main">
  <section class="page-intro">
    <div>
      <span class="eyebrow">process dashboard</span>
      <h1>Jobs</h1>
      <p class="muted">Monitor wrapped processes, find jobs quickly, and open a job for live output and controls.</p>
    </div>
    <span id="jobs-refresh-status" class="section-meta">refreshes every 5 s</span>
  </section>

  <section class="summary-strip" aria-label="Job summary">
    <button class="summary-tile is-selected" data-job-filter="all" type="button">
      <span class="summary-label">All</span><strong id="summary-all">0</strong>
    </button>
    <button class="summary-tile" data-job-filter="active" type="button">
      <span class="summary-label">Active</span><strong id="summary-active">0</strong>
    </button>
    <button class="summary-tile" data-job-filter="attention" type="button">
      <span class="summary-label">Needs attention</span><strong id="summary-attention">0</strong>
    </button>
    <button class="summary-tile" data-job-filter="finished" type="button">
      <span class="summary-label">Finished</span><strong id="summary-finished">0</strong>
    </button>
  </section>

  <section class="card jobs-card">
    <div class="jobs-toolbar">
      <label class="search-field" for="job-search">
        <span class="sr-only">Search jobs</span>
        <input id="job-search" type="search" placeholder="Search name, profile, PID or job ID" autocomplete="off">
      </label>
      <div class="filter-group" aria-label="Filter jobs">
        <button type="button" class="filter-button is-selected" data-job-filter="all">All</button>
        <button type="button" class="filter-button" data-job-filter="active">Active</button>
        <button type="button" class="filter-button" data-job-filter="attention">Attention</button>
        <button type="button" class="filter-button" data-job-filter="finished">Finished</button>
      </div>
      <button id="refresh-jobs" type="button" class="secondary-button">Refresh</button>
    </div>
    <div id="job-list" data-jobs='{jobs_json}'></div>
    <p id="job-list-meta" class="list-meta" aria-live="polite"></p>
  </section>
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
<header class="app-header job-header">
  <div class="header-inner">
    <div class="header-primary">
      <a href="/" class="brand-link">jobwrap</a>
      <a href="/" class="back-link">&larr; Jobs</a>
      <div class="job-header-title">
        <span class="eyebrow">job details</span>
        <h1>{display_name}</h1>
      </div>
    </div>
    <div class="header-actions">
      <span id="job-state" class="state status-pill neutral">{state}</span>
      {AUTH_CONTROLS}
    </div>
  </div>
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

/// The configuration an owner needs to enable daemon-side process creation.
pub const LAUNCH_CONFIG_SNIPPET: &str = "[launch]\nenabled = true\nrequire_preview = false";

/// The new-job page: a thin client over the same launch API the CLI uses.
pub fn new_job_page(capabilities: &LaunchCapabilities, authenticated: bool) -> String {
    let body = if !authenticated {
        "<div class=\"notice\"><h2>Authentication required</h2>\
<p>Log in to create a new job from the web interface.</p>\
<p><a href=\"/login\" class=\"primary-link inline-action\">Log in</a></p></div>"
            .to_string()
    } else if !capabilities.enabled {
        format!(
            "<div class=\"notice\"><h2>Process creation is disabled</h2>\
<p>Daemon-side process creation is an opt-in feature. The ordinary \
<code>jobwrap COMMAND</code> path always works. To start jobs from this web \
interface (and the API), add the following to your configuration and restart \
the daemon:</p><pre><code>{}</code></pre>\
<p class=\"muted\">With <code>require_preview = false</code> only the \
<strong>new-terminal</strong> mode is available; managed and \
existing-terminal daemon launches fail closed by design. Every launch still \
requires an authenticated session and a fresh idempotency key.</p></div>",
            LAUNCH_CONFIG_SNIPPET
        )
    } else if capabilities.require_preview {
        "<div class=\"notice\"><h2>Launch preview is required but not implemented</h2>\
<p>Your configuration has <code>[launch] require_preview = true</code>, but the \
launch preview/confirmation flow is not implemented yet, so every launch fails \
closed. Owners can set <code>require_preview = false</code> to allow direct \
new-terminal launches.</p></div>"
            .to_string()
    } else {
        render_launch_form(capabilities)
    };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>jobwrap: new job</title>
<link rel="stylesheet" href="/static/app.css">
</head>
<body>
<header class="app-header">
  <div class="header-inner">
    <div class="header-primary">
      <a href="/" class="brand-link">jobwrap</a>
      <a href="/" class="back-link">&larr; Jobs</a>
      <div class="page-header-title"><span class="eyebrow">process creation</span><h1>New job</h1></div>
    </div>
    <div class="header-actions">{AUTH_CONTROLS}</div>
  </div>
</header>
<main class="form-main">
{body}
</main>
<script src="/static/app.js"></script>
</body>
</html>"#
    )
}

fn render_launch_form(capabilities: &LaunchCapabilities) -> String {
    let mut profile_options = String::new();
    for profile in &capabilities.profiles {
        let selected = if *profile == capabilities.default_profile {
            " selected"
        } else {
            ""
        };
        profile_options.push_str(&format!(
            "<option value=\"{}\"{}>{}</option>",
            html_escape(profile),
            selected,
            html_escape(profile)
        ));
    }

    let backend_field = if capabilities.allow_backend_selection {
        let mut options = String::new();
        for (backend, available) in &capabilities.backends {
            let disabled = if *available { "" } else { " disabled" };
            let suffix = if *available { "" } else { " (not installed)" };
            options.push_str(&format!(
                "<option value=\"{}\"{}>{}{}</option>",
                html_escape(backend),
                disabled,
                html_escape(backend),
                suffix
            ));
        }
        format!(
            "<label for=\"backend\">Terminal backend</label>\n  \
<select id=\"backend\" name=\"backend\">{options}</select>"
        )
    } else {
        String::new()
    };

    let backend_note = if capabilities.backend_available {
        format!(
            "<p class=\"muted\">The command runs in a new <code>{}</code> \
terminal window via the one-time <code>jobwrap attach-launch</code> helper; \
arguments are never placed on the emulator command line.</p>",
            html_escape(&capabilities.preferred_backend)
        )
    } else {
        format!(
            "<p class=\"warning-notice\">The configured terminal backend \
<code>{}</code> is not installed; launches will fail until a supported \
backend (gnome-terminal or xterm) is available.</p>",
            html_escape(&capabilities.preferred_backend)
        )
    };

    let modes_note = "<p class=\"muted\">Only the <strong>new-terminal</strong> \
mode is currently available. Managed and existing-terminal daemon launches \
fail closed by design (see the security model documentation).</p>";

    format!(
        r#"<section class="card launch-card">
  <div class="section-heading">
    <div><span class="eyebrow">launch configuration</span><h2>Start a wrapped process</h2></div>
  </div>
  <form id="launch-form" class="launch-form">
    <label for="executable">Executable</label>
    <input type="text" id="executable" name="executable" required
           placeholder="/usr/bin/python3" autocomplete="off">
    <label for="arguments">Arguments (one per line, no shell)</label>
    <textarea id="arguments" name="arguments" rows="4"
              placeholder="--config&#10;analysis.yaml"></textarea>
    <label for="working-directory">Working directory (optional)</label>
    <input type="text" id="working-directory" name="working-directory"
           autocomplete="off">
    <label for="display-name">Job name (optional)</label>
    <input type="text" id="display-name" name="display-name" autocomplete="off">
    <label for="profile">Profile</label>
    <select id="profile" name="profile">{profile_options}</select>
    {backend_field}
    <button type="submit" id="launch-button">Launch job</button>
    <p id="launch-error" class="error"></p>
    <p id="launch-status" class="muted"></p>
  </form>
  <div class="launch-notes">{backend_note}{modes_note}</div>
</section>"#
    )
}

pub fn login_page() -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>jobwrap login</title>
<link rel="stylesheet" href="/static/app.css">
</head>
<body>
<header class="app-header">
  <div class="header-inner">
    <div class="header-primary">
      <a href="/" class="brand-link">jobwrap</a>
      <a href="/" class="back-link">&larr; Jobs</a>
      <div class="page-header-title"><span class="eyebrow">session</span><h1>Log in</h1></div>
    </div>
    <div class="header-actions">{AUTH_CONTROLS}</div>
  </div>
</header>
<main class="login-main">
  <section class="card login-card">
    <div class="section-heading">
      <div><span class="eyebrow">controller access</span><h2>Authenticate to jobwrap</h2></div>
    </div>
    <p class="muted">Log in to view protected command details and use controls allowed by each job's access policy.</p>
    <form id="login-form" class="login-form">
      <label for="password">Password</label>
      <input type="password" id="password" name="password" autocomplete="current-password" autofocus>
      <button type="submit">Log in</button>
      <p id="login-error" class="error" aria-live="polite"></p>
    </form>
  </section>
</main>
<script src="/static/app.js"></script>
</body>
</html>"#
    )
}

pub fn not_found_page(id: &str) -> String {
    let id = html_escape(id);
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>not found</title>
<link rel="stylesheet" href="/static/app.css"></head><body>
<header class="app-header"><div class="header-inner"><div class="header-primary"><a href="/" class="brand-link">jobwrap</a><a href="/" class="back-link">&larr; Jobs</a></div><div class="header-actions">{AUTH_CONTROLS}</div></div></header>
<main class="message-main"><section class="card message-card"><span class="eyebrow">not found</span><h1>Job not found</h1>
<p>No job with id <code>{id}</code> exists.</p><p><a href="/" class="primary-link inline-action">Back to jobs</a></p></section></main><script src="/static/app.js"></script></body></html>"#
    )
}

pub fn error_page(message: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>error</title>
<link rel="stylesheet" href="/static/app.css"></head><body>
<header class="app-header"><div class="header-inner"><div class="header-primary"><a href="/" class="brand-link">jobwrap</a><a href="/" class="back-link">&larr; Jobs</a></div><div class="header-actions">{AUTH_CONTROLS}</div></div></header>
<main class="message-main"><section class="card message-card"><span class="eyebrow">request failed</span><h1>Error</h1>
<p>{}</p><p><a href="/" class="primary-link inline-action">Back to jobs</a></p></section></main><script src="/static/app.js"></script></body></html>"#,
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

    fn ready_capabilities() -> LaunchCapabilities {
        LaunchCapabilities {
            enabled: true,
            require_preview: false,
            profiles: vec!["standard".into(), "private\"".into()],
            default_profile: "standard".into(),
            default_terminal_mode: "new-terminal".into(),
            preferred_backend: "gnome-terminal".into(),
            backend_available: true,
            allow_backend_selection: false,
            backends: Vec::new(),
        }
    }

    #[test]
    fn embedded_json_cannot_escape_its_attribute() {
        let page = index_page(r#"[{"display_name":"' onmouseover='alert(1)"}]"#, false);
        assert!(!page.contains("data-jobs='[{\"display_name\":\"' onmouseover="));
        assert!(page.contains("&#39; onmouseover=&#39;"));
    }

    #[test]
    fn index_links_to_the_new_job_page() {
        assert!(index_page("[]", true).contains("href=\"/jobs/new\""));
    }

    #[test]
    fn pages_share_authentication_controls() {
        assert!(index_page("[]", false).contains("data-auth-controls"));
        assert!(login_page().contains("data-auth-controls"));
    }

    #[test]
    fn ready_page_renders_form_and_escapes_options() {
        let page = new_job_page(&ready_capabilities(), true);
        assert!(page.contains("id=\"launch-form\""));
        assert!(page.contains("value=\"private&quot;\""));
        assert!(page.contains("<option value=\"standard\" selected>"));
        assert!(!page.contains("id=\"backend\""));
        assert!(page.contains("gnome-terminal"));
    }

    #[test]
    fn ready_page_offers_backend_selection_only_when_allowed() {
        let mut capabilities = ready_capabilities();
        capabilities.allow_backend_selection = true;
        capabilities.backends = vec![("gnome-terminal".into(), true), ("xterm".into(), false)];
        let page = new_job_page(&capabilities, true);
        assert!(page.contains("id=\"backend\""));
        assert!(page.contains("xterm (not installed)"));
        assert!(page.contains("disabled>xterm"));
    }

    #[test]
    fn disabled_page_shows_guidance_without_a_form() {
        let mut capabilities = ready_capabilities();
        capabilities.enabled = false;
        let page = new_job_page(&capabilities, true);
        assert!(page.contains("Process creation is disabled"));
        assert!(page.contains("[launch]"));
        assert!(page.contains("enabled = true"));
        assert!(page.contains("require_preview = false"));
        assert!(!page.contains("id=\"launch-form\""));
    }

    #[test]
    fn preview_required_page_fails_closed_with_guidance() {
        let mut capabilities = ready_capabilities();
        capabilities.require_preview = true;
        let page = new_job_page(&capabilities, true);
        assert!(page.contains("require_preview"));
        assert!(!page.contains("id=\"launch-form\""));
    }

    #[test]
    fn anonymous_page_asks_for_login() {
        let page = new_job_page(&ready_capabilities(), false);
        assert!(page.contains("href=\"/login\""));
        assert!(!page.contains("id=\"launch-form\""));
    }

    #[test]
    fn unavailable_backend_warns_before_any_launch() {
        let mut capabilities = ready_capabilities();
        capabilities.backend_available = false;
        let page = new_job_page(&capabilities, true);
        assert!(page.contains("is not installed"));
        assert!(page.contains("launch-form"));
    }
}
