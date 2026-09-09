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
