//! HTTP handlers.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use jobwrap_core::{JobId, Principal, Signal};

use crate::assets;
use crate::error::ApiError;
use crate::events::ServerEvent;
use crate::router::RouterState;
use crate::service::JobService;

const SESSION_COOKIE: &str = "jobwrap_session";

fn service(state: &RouterState) -> &Arc<dyn JobService> {
    &state.service
}

fn principal_from_headers(service: &Arc<dyn JobService>, headers: &HeaderMap) -> Principal {
    let session = cookie_value(headers, SESSION_COOKIE);
    let bearer = bearer_token(headers);
    service.resolve_principal(session.as_deref(), bearer.as_deref())
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie = headers.get("cookie")?.to_str().ok()?;
    for part in cookie.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{name}=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    auth.strip_prefix("Bearer ").map(str::to_string)
}

/// Reject cross-origin state-changing requests from browsers.
fn check_csrf(headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        return Ok(()); // Non-browser clients (curl, tokens) have no Origin.
    };
    let expected = service_base_origin(headers);
    if origin == expected {
        Ok(())
    } else {
        Err(ApiError::permission_denied("cross-origin request rejected"))
    }
}

fn service_base_origin(headers: &HeaderMap) -> String {
    // Only used for loopback deployments; a reverse proxy should be used for
    // remote access.
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("127.0.0.1")
        .to_string();
    format!("http://{host}")
}

fn job_id(path: &str) -> Result<JobId, ApiError> {
    path.parse().map_err(|_| ApiError::not_found("no such job"))
}

fn render(html: String) -> Response {
    Html(html).into_response()
}

// ---- Pages ----

pub async fn index(State(state): State<RouterState>, headers: HeaderMap) -> Response {
    let principal = principal_from_headers(service(&state), &headers);
    let jobs = state.service.list_jobs(&principal);
    let jobs_json = serde_json::to_string(&jobs).unwrap_or_else(|_| "[]".into());
    let authenticated = !matches!(principal, Principal::Anonymous);
    render(assets::index_page(&jobs_json, authenticated))
}

pub async fn jobs_index(State(state): State<RouterState>, headers: HeaderMap) -> Response {
    index(State(state), headers).await
}

pub async fn login_page() -> Response {
    render(assets::login_page())
}

pub async fn job_page(
    State(state): State<RouterState>,
    Path(job): Path<String>,
    headers: HeaderMap,
) -> Response {
    let id = match job_id(&job) {
        Ok(id) => id,
        Err(_) => return render(assets::not_found_page(&job)),
    };
    let principal = principal_from_headers(service(&state), &headers);
    match state.service.get_job(&principal, id) {
        Ok(record) => render(assets::job_page(&record)),
        Err(ApiError { message, .. }) => render(assets::error_page(&message)),
    }
}

// ---- Static assets ----

pub async fn app_js() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "text/javascript")],
        assets::APP_JS,
    )
        .into_response()
}

pub async fn app_css() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "text/css")],
        assets::APP_CSS,
    )
        .into_response()
}

// ---- API ----

#[derive(Debug, Serialize)]
struct ServerInfoResponse {
    version: String,
    auth_required: bool,
}

pub async fn server_info(State(state): State<RouterState>) -> impl IntoResponse {
    let info = state.service.server_info();
    Json(json!({
        "version": info.version,
        "auth_required": info.auth_required,
        "bind": info.bind,
        "port": info.port,
    }))
}

pub async fn auth_status(State(state): State<RouterState>, headers: HeaderMap) -> Response {
    let principal = principal_from_headers(service(&state), &headers);
    let authenticated = !matches!(principal, Principal::Anonymous);
    Json(json!({
        "authenticated": authenticated,
        "password_set": state.service.server_info().auth_required,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    password: String,
}

pub async fn login(
    State(state): State<RouterState>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    if let Err(error) = check_csrf(&headers) {
        return error.into_response();
    }
    let service = service(&state);
    match service.login(&req.password) {
        Ok(session_token) => {
            let cookie =
                format!("{SESSION_COOKIE}={session_token}; Path=/; HttpOnly; SameSite=Strict");
            let mut response = Json(json!({ "ok": true })).into_response();
            let parsed: axum::http::HeaderValue = cookie.parse().expect("valid cookie");
            response
                .headers_mut()
                .insert(axum::http::header::SET_COOKIE, parsed);
            response
        }
        Err(e) => e.into_response(),
    }
}

pub async fn logout(State(state): State<RouterState>, headers: HeaderMap) -> Response {
    if let Err(error) = check_csrf(&headers) {
        return error.into_response();
    }
    let service = service(&state);
    let session = cookie_value(&headers, SESSION_COOKIE);
    if let Some(session) = session {
        service.logout(&session);
    }
    StatusCode::NO_CONTENT.into_response()
}

pub async fn list_jobs(State(state): State<RouterState>, headers: HeaderMap) -> Response {
    let principal = principal_from_headers(service(&state), &headers);
    let jobs = state.service.list_jobs(&principal);
    Json(jobs).into_response()
}

pub async fn get_job(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = job_id(&id)?;
    let principal = principal_from_headers(service(&state), &headers);
    let record = state.service.get_job(&principal, id)?;
    Ok(Json(summarize_record(&record)))
}

fn summarize_record(record: &jobwrap_core::JobRecord) -> serde_json::Value {
    json!({
        "id": record.id.to_string(),
        "name": record.display_name.as_str(),
        "state": record.state,
        "profile": record.profile_name,
        "started_at": record.started_at.to_rfc3339(),
        "finished_at": record.finished_at.map(|t| t.to_rfc3339()),
        "terminal_attached": record.terminal.attached,
    })
}

#[derive(Debug, Deserialize)]
pub struct OutputQuery {
    #[serde(default)]
    from: Option<u64>,
}

pub async fn get_output(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    Query(q): Query<OutputQuery>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = job_id(&id)?;
    let principal = principal_from_headers(service(&state), &headers);
    let slice = state
        .service
        .get_output(&principal, id, q.from.unwrap_or(0))?;
    Ok(Json(json!({
        "job_id": slice.job_id.to_string(),
        "sequence_start": slice.sequence_start,
        "truncated": slice.truncated,
        "data_base64": slice.data_base64,
    })))
}

pub async fn get_events(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let id = job_id(&id)?;
    let principal = principal_from_headers(service(&state), &headers);
    let events = state.service.get_events(&principal, id)?;
    let out: Vec<serde_json::Value> = events
        .into_iter()
        .map(|e| json!({ "id": e.id.0, "at": e.at.to_rfc3339(), "kind": e.kind }))
        .collect();
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
pub struct InputRequest {
    data_base64: String,
}

pub async fn send_input(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<InputRequest>,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let id = job_id(&id)?;
    let principal = principal_from_headers(service(&state), &headers);
    let data = jobwrap_protocol::decode_input(&req.data_base64)
        .ok_or_else(|| ApiError::bad_request("invalid base64 data"))?;
    state.service.send_input(&principal, id, &data)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn send_signal(
    State(state): State<RouterState>,
    Path((id, signal)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let id = job_id(&id)?;
    let signal = parse_signal(&signal)?;
    let principal = principal_from_headers(service(&state), &headers);
    state.service.send_signal(&principal, id, signal)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_job(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let id = job_id(&id)?;
    let principal = principal_from_headers(service(&state), &headers);
    state.service.delete_job(&principal, id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Request body for POST /api/v1/launch.
#[derive(Debug, Deserialize)]
pub struct ApiLaunchRequest {
    pub executable: String,
    pub arguments: Option<Vec<String>>,
    pub working_directory: Option<String>,
    pub display_name: Option<String>,
    pub profile_name: Option<String>,
    pub idempotency_key: Option<String>,
    pub terminal_target: Option<jobwrap_core::TerminalTarget>,
}

pub async fn launch_job(
    State(state): State<RouterState>,
    headers: HeaderMap,
    Json(req): Json<ApiLaunchRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_csrf(&headers)?;
    let principal = principal_from_headers(service(&state), &headers);
    let key = req
        .idempotency_key
        .clone()
        .ok_or_else(|| ApiError::bad_request("idempotency_key is required for process creation"))?;
    if key.is_empty() || key.len() > 128 {
        return Err(ApiError::bad_request(
            "idempotency_key must contain 1 to 128 characters",
        ));
    }
    let launch_req = jobwrap_protocol::LaunchRequest {
        executable: req.executable,
        arguments: req.arguments.unwrap_or_default(),
        working_directory: req.working_directory,
        display_name: req.display_name,
        profile_name: req.profile_name,
        idempotency_key: key,
        terminal_target: req.terminal_target.unwrap_or_default(),
    };
    let (launch_id, job_id) = state.service.launch(&principal, launch_req)?;
    Ok(Json(json!({
        "launch_id": launch_id,
        "job_id": job_id.to_string(),
        "state": "running",
    })))
}

fn parse_signal(raw: &str) -> Result<Signal, ApiError> {
    raw.parse()
        .map_err(|_| ApiError::bad_request(format!("unknown signal `{raw}`")))
}

// ---- Documentation ----

#[derive(Debug, Deserialize)]
pub struct TargetRequest {
    target: String,
}

pub async fn identify_target(
    State(state): State<RouterState>,
    headers: HeaderMap,
    Json(req): Json<TargetRequest>,
) -> Result<Json<jobwrap_protocol::TargetInfo>, ApiError> {
    let principal = principal_from_headers(service(&state), &headers);
    let info = state.service.identify_target(&principal, &req.target)?;
    Ok(Json(info))
}

#[derive(Debug, Deserialize)]
pub struct ManSearchRequest {
    target: String,
}

pub async fn search_man_pages(
    State(state): State<RouterState>,
    headers: HeaderMap,
    Json(req): Json<ManSearchRequest>,
) -> Result<Json<Vec<jobwrap_protocol::ManPageMatch>>, ApiError> {
    let principal = principal_from_headers(service(&state), &headers);
    let matches = state.service.search_man_pages(&principal, &req.target)?;
    Ok(Json(matches))
}

pub async fn fetch_man_page(
    State(state): State<RouterState>,
    Path((name, section)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Option<jobwrap_protocol::ManPage>>, ApiError> {
    let principal = principal_from_headers(service(&state), &headers);
    let page = state
        .service
        .fetch_man_page(&principal, &name, Some(section))?;
    Ok(Json(page))
}

// ---- Help probes ----

#[derive(Debug, Deserialize)]
pub struct ProbeRequest {
    target: String,
    probe_argument: Option<String>,
    kind: String,
    idempotency_key: Option<String>,
}

pub async fn preview_help_probe(
    State(state): State<RouterState>,
    headers: HeaderMap,
    Json(req): Json<ProbeRequest>,
) -> Result<Json<jobwrap_protocol::ProbePreview>, ApiError> {
    check_csrf(&headers)?;
    let principal = principal_from_headers(service(&state), &headers);
    let kind = match req.kind.as_str() {
        "interpreter" => jobwrap_protocol::HelpProbeKind::Interpreter,
        "executable" => jobwrap_protocol::HelpProbeKind::Executable,
        "script" => jobwrap_protocol::HelpProbeKind::Script,
        "custom" => jobwrap_protocol::HelpProbeKind::Custom,
        other => {
            return Err(ApiError::bad_request(format!(
                "unknown probe kind `{other}`"
            )))
        }
    };
    let key = req.idempotency_key.unwrap_or_default();
    let probe_req = jobwrap_protocol::HelpProbeRequest {
        target: req.target,
        probe_argument: req.probe_argument,
        kind,
        idempotency_key: key,
    };
    let preview = state.service.preview_help_probe(&principal, probe_req)?;
    Ok(Json(preview))
}

pub async fn execute_help_probe(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<jobwrap_protocol::HelpProbeResult>, ApiError> {
    check_csrf(&headers)?;
    let principal = principal_from_headers(service(&state), &headers);
    let result = state.service.execute_help_probe(&principal, &id)?;
    Ok(Json(result))
}

pub async fn get_help_probe(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Option<jobwrap_protocol::HelpProbeResult>>, ApiError> {
    let principal = principal_from_headers(service(&state), &headers);
    let result = state.service.get_help_probe(&principal, &id)?;
    Ok(Json(result))
}

pub async fn delete_help_probe(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    check_csrf(&headers)?;
    let principal = principal_from_headers(service(&state), &headers);
    let _ = state.service.delete_help_probe(&principal, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Terminals ----

pub async fn list_terminals(
    State(state): State<RouterState>,
    headers: HeaderMap,
) -> Result<Json<Vec<jobwrap_protocol::TerminalInfo>>, ApiError> {
    let principal = principal_from_headers(service(&state), &headers);
    let terminals = state.service.list_terminals(&principal);
    Ok(Json(terminals))
}

// ---- WebSocket ----

pub async fn job_ws(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    check_csrf(&headers)?;
    let id = job_id(&id)?;
    let principal = principal_from_headers(service(&state), &headers);
    // Output authorization is intentionally stronger than status access.
    let _ = state.service.get_output(&principal, id, u64::MAX)?;
    let permit = state
        .websocket_limit
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::conflict("too many live terminal connections"))?;
    let broadcast = state.service.broadcast();
    Ok(ws.on_upgrade(move |socket| ws_loop(socket, id, broadcast, permit)))
}

async fn ws_loop(
    mut socket: WebSocket,
    job_id: JobId,
    broadcast: tokio::sync::broadcast::Sender<ServerEvent>,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let mut rx = broadcast.subscribe();
    loop {
        tokio::select! {
            frame = rx.recv() => {
                let event = match frame {
                    Ok(ServerEvent::Output { job_id: jid, sequence, data }) if jid == job_id => {
                        Message::Text(format!(
                            "{{\"type\":\"output\",\"job_id\":\"{}\",\"sequence\":{},\"data_base64\":\"{}\"}}",
                            jid,
                            sequence,
                            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data)
                        ))
                    }
                    Ok(ServerEvent::StateChanged { job_id: jid, state }) if jid == job_id => {
                        let state_json = serde_json::to_string(&state).unwrap_or_else(|_| "{}".into());
                        Message::Text(format!(
                            "{{\"type\":\"state_changed\",\"job_id\":\"{}\",\"state\":{}}}",
                            jid, state_json
                        ))
                    }
                    _ => continue,
                };
                if socket.send(event).await.is_err() {
                    break;
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(Message::Text(text))) => {
                        // Client messages: send_input and resize are handled by
                        // the HTTP API; ignore control frames here for now.
                        let _ = text;
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}
