//! The private Unix-socket server handling wrapper and CLI connections.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use jobwrap_core::Principal;
use jobwrap_protocol::{
    codec, error_response, ClientToDaemon, DaemonToClient, HelloRole, ToWrapper, WrapperToDaemon,
    PROTOCOL_VERSION,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

use crate::registry::Registry;
use crate::service::DaemonService;

/// Serve the Unix socket until the process exits.
pub async fn serve(
    registry: Arc<Registry>,
    service: Arc<DaemonService>,
    path: PathBuf,
) -> std::io::Result<()> {
    let listener = UnixListener::bind(&path)?;
    let connection_limit = Arc::new(tokio::sync::Semaphore::new(128));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!(path = %path.display(), "unix socket listening");

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                continue;
            }
        };
        let registry = registry.clone();
        let service = service.clone();
        let permit = match connection_limit.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!("Unix socket connection limit reached");
                continue;
            }
        };
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = handle_connection(stream, registry, service).await {
                tracing::debug!(error = %e, "connection closed");
            }
        });
    }
}

async fn handle_connection(
    mut stream: UnixStream,
    registry: Arc<Registry>,
    service: Arc<DaemonService>,
) -> std::io::Result<()> {
    use nix::sys::socket::{getsockopt, sockopt};
    let credentials =
        getsockopt(&stream, sockopt::PeerCredentials).map_err(std::io::Error::from)?;
    let peer_uid = credentials.uid();
    if peer_uid != nix::unistd::Uid::current().as_raw() {
        return Ok(());
    }
    // Expect a Hello first.
    let hello = match read_request(&mut stream).await {
        Some(ClientToDaemon::Hello(hello)) => hello,
        Some(_) => return Ok(()),
        None => return Ok(()),
    };
    if hello.protocol_version != PROTOCOL_VERSION {
        write_response(
            &mut stream,
            &error_response("version_rejected", "protocol version mismatch"),
        )
        .await?;
        return Ok(());
    }
    if hello.uid != peer_uid || hello.pid != credentials.pid() {
        write_response(
            &mut stream,
            &error_response(
                "identity_rejected",
                "hello identity does not match socket peer",
            ),
        )
        .await?;
        return Ok(());
    }
    write_response(
        &mut stream,
        &DaemonToClient::HelloAck {
            protocol_version: PROTOCOL_VERSION,
            daemon_pid: std::process::id() as i32,
        },
    )
    .await?;

    match hello.role {
        HelloRole::Wrapper => handle_wrapper(stream, registry, peer_uid).await,
        HelloRole::Cli => handle_cli(stream, registry, service, peer_uid).await,
    }
}

/// A wrapper connection: register the job, stream output, relay control.
async fn handle_wrapper(
    stream: UnixStream,
    registry: Arc<Registry>,
    peer_uid: u32,
) -> std::io::Result<()> {
    let (mut read, mut write) = stream.into_split();
    let first = match read_request_from(&mut read).await {
        Some(m) => m,
        None => return Ok(()),
    };

    // The `attach-launch` helper (started by a terminal backend) first
    // retrieves the pending structured launch over this connection, then
    // reconnects to register the resulting job.
    if let ClientToDaemon::AttachLaunch { launch_id } = &first {
        match registry.attach_launch(launch_id) {
            Ok((request, job_id)) => {
                write_response(
                    &mut write,
                    &DaemonToClient::PendingLaunch { request, job_id },
                )
                .await?;
            }
            Err(e) => {
                write_response(&mut write, &error_response("launch_not_found", e)).await?;
            }
        }
        return Ok(());
    }

    let ClientToDaemon::RegisterJob(mut register) = first else {
        return Ok(());
    };
    register.owner_uid = peer_uid;

    // Build the control channel. The writer task starts after the registration
    // reply is written so replies stay ordered.
    let (wrapper_tx, wrapper_rx) = mpsc::channel::<ToWrapper>(64);

    let job_id = register.id;
    let registered = registry.register_job(register, wrapper_tx);

    let write_result = match &registered {
        Ok(reg) => write_response(&mut write, &DaemonToClient::Registered(reg.clone())).await,
        Err(message) => {
            write_response(
                &mut write,
                &error_response("registration_failed", message.clone()),
            )
            .await
        }
    };
    if write_result.is_err() || registered.is_err() {
        if registered.is_ok() {
            registry.wrapper_disconnected(
                job_id,
                "registration acknowledgement could not be delivered",
            );
        }
        return Ok(());
    }

    // The wrapper control writer task owns the write half from here on.
    let writer_task = tokio::spawn(async move {
        let mut write = write;
        let mut wrapper_rx = wrapper_rx;
        while let Some(msg) = wrapper_rx.recv().await {
            let Some(frame) = codec::encode_frame(&DaemonToClient::ToWrapper(msg)).ok() else {
                break;
            };
            if write.write_all(&frame).await.is_err() {
                break;
            }
        }
    });

    // Stream frames from the wrapper.
    let disconnect_reason = loop {
        let msg = match read_request_result(&mut read).await {
            Ok(msg) => msg,
            Err(error) => break error.to_string(),
        };
        match msg {
            ClientToDaemon::Output {
                sequence,
                data_base64,
            } => {
                if let Some(data) = jobwrap_protocol::decode_input(&data_base64) {
                    registry.append_output(job_id, sequence, &data);
                }
            }
            ClientToDaemon::WrapperStopped => {
                registry.handle_wrapper_event(job_id, WrapperToDaemon::Stopped)
            }
            ClientToDaemon::WrapperContinued => {
                registry.handle_wrapper_event(job_id, WrapperToDaemon::Continued)
            }
            ClientToDaemon::WrapperExited { code } => {
                registry.handle_wrapper_event(job_id, WrapperToDaemon::Exited { code });
                break "child exit reported".to_owned();
            }
            ClientToDaemon::WrapperSignaled { signal } => {
                registry.handle_wrapper_event(job_id, WrapperToDaemon::Signaled { signal });
                break "child termination reported".to_owned();
            }
            ClientToDaemon::WrapperBye => {
                break "wrapper sent goodbye without a final child status".to_owned()
            }
            ClientToDaemon::WrapperTerminalLost => {
                break "wrapper reported terminal loss".to_owned()
            }
            _ => {}
        }
    };

    writer_task.abort();
    registry.wrapper_disconnected(job_id, &disconnect_reason);
    Ok(())
}

/// A CLI connection: request/response loop.
async fn handle_cli(
    mut stream: UnixStream,
    registry: Arc<Registry>,
    _service: Arc<DaemonService>,
    peer_uid: u32,
) -> std::io::Result<()> {
    let owner = Principal::LocalUnixUser { uid: peer_uid };
    loop {
        let Some(request) = read_request(&mut stream).await else {
            break;
        };
        let response = handle_cli_message(&registry, &owner, request);
        write_response(&mut stream, &response).await?;
    }
    Ok(())
}

fn handle_cli_message(
    registry: &Registry,
    owner: &Principal,
    request: ClientToDaemon,
) -> DaemonToClient {
    use jobwrap_protocol::ApiErrorCode as Code;
    let err = |code: Code, msg: &str| error_response(code.as_str(), msg);

    match request {
        ClientToDaemon::Hello(_) => err(Code::BadRequest, "unexpected hello"),
        ClientToDaemon::ListJobs => {
            let jobs = registry.list_jobs(owner);
            DaemonToClient::JobList { jobs }
        }
        ClientToDaemon::ShowJob { job_id } => match registry.get_job(owner, job_id) {
            Ok(record) => {
                let summary = crate::registry::summary_for(&record);
                DaemonToClient::JobDetail { job: Some(summary) }
            }
            Err(_) => DaemonToClient::JobDetail { job: None },
        },
        ClientToDaemon::GetOutput {
            job_id,
            sequence_start,
        } => match registry.get_output(owner, job_id, sequence_start) {
            Ok(slice) => DaemonToClient::Output { slice },
            Err(e) => err(Code::PermissionDenied, &e.message),
        },
        ClientToDaemon::SendInput {
            job_id,
            data_base64,
        } => {
            let Some(data) = jobwrap_protocol::decode_input(&data_base64) else {
                return err(Code::BadRequest, "input is not valid base64");
            };
            match registry.send_input(owner, job_id, &data) {
                Ok(()) => DaemonToClient::Ack,
                Err(e) => err(e.code, &e.message),
            }
        }
        ClientToDaemon::SendSignal { job_id, signal } => {
            match registry.send_signal(owner, job_id, signal) {
                Ok(()) => DaemonToClient::Ack,
                Err(e) => err(e.code, &e.message),
            }
        }
        ClientToDaemon::AuthStatus => {
            let auth = registry.auth();
            DaemonToClient::AuthStatus {
                password_set: auth.password_set(),
                token_count: auth.list_tokens().len(),
                trust_local_owner: registry.config.authentication.trust_local_owner,
            }
        }
        ClientToDaemon::SetPassword { password } => match registry.auth().set_password(&password) {
            Ok(()) => DaemonToClient::Ack,
            Err(e) => err(Code::BadRequest, &e),
        },
        ClientToDaemon::RemovePassword => match registry.auth().remove_password() {
            Ok(()) => DaemonToClient::Ack,
            Err(e) => err(Code::Internal, &e),
        },
        ClientToDaemon::TokenCreate(req) => match registry.auth().create_token(&req) {
            Ok((token_id, token)) => DaemonToClient::TokenCreated { token_id, token },
            Err(e) => err(Code::BadRequest, &e),
        },
        ClientToDaemon::TokenList => {
            let tokens = registry.auth().list_tokens();
            DaemonToClient::TokenList { tokens }
        }
        ClientToDaemon::TokenRevoke { token_id } => match registry.auth().revoke_token(&token_id) {
            Ok(()) => DaemonToClient::TokenRevoked { token_id },
            Err(e) => err(Code::Internal, &e),
        },
        ClientToDaemon::LaunchJob(req) => match registry.launch(owner, &req) {
            Ok((launch_id, job_id)) => DaemonToClient::Launched { launch_id, job_id },
            Err(e) => err(Code::PermissionDenied, &e),
        },
        ClientToDaemon::AttachLaunch { launch_id } => match registry.attach_launch(&launch_id) {
            Ok((request, job_id)) => DaemonToClient::PendingLaunch { request, job_id },
            Err(e) => err(Code::BadRequest, &e),
        },
        ClientToDaemon::IdentifyTarget { target } => {
            match registry.identify_target(owner, &target) {
                Ok(info) => DaemonToClient::TargetInfo { info },
                Err(e) => err(Code::PermissionDenied, &e),
            }
        }
        ClientToDaemon::SearchManPages { target, section: _ } => {
            match registry.search_man_pages(owner, &target) {
                Ok(matches) => DaemonToClient::ManPageSearch { matches },
                Err(e) => err(Code::PermissionDenied, &e),
            }
        }
        ClientToDaemon::PreviewHelpProbe(req) => match registry.preview_help_probe(owner, &req) {
            Ok(preview) => DaemonToClient::ProbePreview { preview },
            Err(e) => err(Code::PermissionDenied, &e),
        },
        ClientToDaemon::ExecuteHelpProbe { preview_id } => {
            match registry.execute_help_probe(owner, &preview_id) {
                Ok(result) => DaemonToClient::ProbeResult { result },
                Err(e) => err(Code::BadRequest, &e),
            }
        }
        ClientToDaemon::GetHelpProbe { probe_id } => {
            match registry.get_help_probe(owner, &probe_id) {
                Ok(Some(result)) => DaemonToClient::ProbeResult { result },
                Ok(None) => err(Code::NotFound, "probe not found"),
                Err(e) => err(Code::PermissionDenied, &e),
            }
        }
        ClientToDaemon::DeleteHelpProbe { probe_id } => {
            match registry.delete_help_probe(owner, &probe_id) {
                Ok(true) => DaemonToClient::ProbeDeleted { probe_id },
                Ok(false) => err(Code::NotFound, "probe not found"),
                Err(e) => err(Code::PermissionDenied, &e),
            }
        }
        ClientToDaemon::ListTerminals => {
            let terminals = registry.list_terminals(owner);
            DaemonToClient::TerminalList { terminals }
        }
        ClientToDaemon::RegisterTerminal(t) => {
            let state = if t.ready {
                crate::terminal::TerminalState::Ready
            } else {
                crate::terminal::TerminalState::Busy
            };
            match registry.register_terminal(crate::terminal::RegisteredTerminal {
                terminal_id: t.terminal_id,
                owner_uid: match owner {
                    Principal::LocalUnixUser { uid } => *uid,
                    _ => unreachable!("CLI socket principals are local users"),
                },
                state,
                shell_type: t.shell_type,
                working_directory: t.working_directory,
                control_path: t.control_path,
                registered_at: Utc::now(),
                last_heartbeat: Utc::now(),
            }) {
                Ok(()) => DaemonToClient::Ack,
                Err(error) => err(Code::BadRequest, &error),
            }
        }
        ClientToDaemon::Output { .. } => err(Code::BadRequest, "output is wrapper-only"),
        ClientToDaemon::WrapperReady
        | ClientToDaemon::WrapperStopped
        | ClientToDaemon::WrapperContinued
        | ClientToDaemon::WrapperExited { .. }
        | ClientToDaemon::WrapperSignaled { .. }
        | ClientToDaemon::WrapperTerminalLost
        | ClientToDaemon::WrapperBye => err(Code::BadRequest, "wrapper events are wrapper-only"),
        ClientToDaemon::RegisterJob(_) => err(Code::BadRequest, "registration is wrapper-only"),
    }
}

async fn read_request(stream: &mut UnixStream) -> Option<ClientToDaemon> {
    read_request_from(stream).await
}

async fn read_request_from(read: &mut (impl AsyncReadExt + Unpin)) -> Option<ClientToDaemon> {
    read_request_result(read).await.ok()
}

async fn read_request_result(
    read: &mut (impl AsyncReadExt + Unpin),
) -> std::io::Result<ClientToDaemon> {
    let mut header = [0u8; 4];
    read.read_exact(&mut header).await?;
    let len = u32::from_be_bytes(header) as usize;
    if len > jobwrap_protocol::codec::MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "wrapper frame exceeds size limit",
        ));
    }
    let mut body = vec![0u8; len];
    read.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid protocol frame"))
}

async fn write_response(
    stream: &mut (impl AsyncWriteExt + Unpin),
    response: &DaemonToClient,
) -> std::io::Result<()> {
    let frame = codec::encode_frame(response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    stream.write_all(&frame).await
}
