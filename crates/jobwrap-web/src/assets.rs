//! Embedded server-rendered HTML and plain JavaScript.
//!
//! No Node build pipeline: assets ship inside the binary so the daemon and
//! browser always agree on versions.

use crate::service::LaunchCapabilities;
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
<header><h1>jobwrap</h1><a href="/jobs/new" class="new-job-link">+ new job</a><span class="auth-badge">{auth_badge}</span></header>
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
<title>jobwrap: {}</title>
<link rel="stylesheet" href="/static/app.css">
</head>
<body>
<header><a href="/" class="back-link">&larr; jobs</a><h1>{}</h1><span id="job-state" class="state">{}</span></header>
<main>
<pre id="terminal" class="terminal" data-job="{}"></pre>
<div id="connection" class="connection"></div>
<section class="controls">
  <button data-signal="int">Interrupt</button>
  <button data-signal="term">Terminate</button>
  <button data-signal="stop">Pause</button>
  <button data-signal="cont">Resume</button>
  <button data-signal="kill">Kill</button>
</section>
</main>
<script src="/static/app.js"></script>
</body>
</html>"#,
        display_name, display_name, state, record.id
    )
}

/// The configuration an owner needs to enable daemon-side process creation.
pub const LAUNCH_CONFIG_SNIPPET: &str = "[launch]\nenabled = true\nrequire_preview = false";

/// The new-job page: a thin client over the same launch API the CLI uses.
pub fn new_job_page(capabilities: &LaunchCapabilities, authenticated: bool) -> String {
    let body = if !authenticated {
        "<p>Log in to create a new job.</p>\
<p><a href=\"/login\" class=\"login-link\">log in</a></p>"
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
<header><a href="/" class="back-link">&larr; jobs</a><h1>new job</h1></header>
<main>
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
            "<p class=\"warning\">The configured terminal backend \
<code>{}</code> is not installed; launches will fail until a supported \
backend (gnome-terminal or xterm) is available.</p>",
            html_escape(&capabilities.preferred_backend)
        )
    };

    let modes_note = "<p class=\"muted\">Only the <strong>new-terminal</strong> \
mode is currently available. Managed and existing-terminal daemon launches \
fail closed by design (see the security model documentation).</p>";

    format!(
        r#"<form id="launch-form" class="launch-form">
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
  <button type="submit" id="launch-button">Launch</button>
  <p id="launch-error" class="error"></p>
  <p id="launch-status" class="muted"></p>
</form>
{backend_note}
{modes_note}"#
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
