//! The private Unix-socket server handling wrapper and CLI connections.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use jobwrap_core::Principal;
use jobwrap_protocol::{
    codec, error_response, ClientToDaemon, DaemonToClient, Hello, HelloRole, ToWrapper,
    WrapperToDaemon, PROTOCOL_VERSION,
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
        tokio::spawn(async move {
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
    write_response(
        &mut stream,
        &DaemonToClient::HelloAck {
            protocol_version: PROTOCOL_VERSION,
            daemon_pid: std::process::id() as i32,
        },
    )
    .await?;

    match hello.role {
        HelloRole::Wrapper => handle_wrapper(stream, registry, hello).await,
        HelloRole::Cli => handle_cli(stream, registry, service, hello).await,
    }
}

/// A wrapper connection: register the job, stream output, relay control.
async fn handle_wrapper(
    stream: UnixStream,
    registry: Arc<Registry>,
    _hello: Hello,
) -> std::io::Result<()> {
    let (mut read, mut write) = stream.into_split();
    let Some(ClientToDaemon::RegisterJob(register)) = read_request_from(&mut read).await else {
        return Ok(());
    };

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
    loop {
        let Some(msg) = read_request_from(&mut read).await else {
            break;
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
                break;
            }
            ClientToDaemon::WrapperSignaled { signal } => {
                registry.handle_wrapper_event(job_id, WrapperToDaemon::Signaled { signal });
                break;
            }
            ClientToDaemon::WrapperBye | ClientToDaemon::WrapperTerminalLost => break,
            _ => {}
        }
    }

    writer_task.abort();
    registry.wrapper_disconnected(job_id);
    Ok(())
}

/// A CLI connection: request/response loop.
async fn handle_cli(
    mut stream: UnixStream,
    registry: Arc<Registry>,
    _service: Arc<DaemonService>,
    hello: Hello,
) -> std::io::Result<()> {
    let owner = Principal::LocalUnixUser { uid: hello.uid };
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
            let data = jobwrap_protocol::decode_input(&data_base64).unwrap_or_default();
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
    let mut header = [0u8; 4];
    if read.read_exact(&mut header).await.is_err() {
        return None;
    }
    let len = u32::from_be_bytes(header) as usize;
    if len > jobwrap_protocol::codec::MAX_FRAME_BYTES {
        return None;
    }
    let mut body = vec![0u8; len];
    if read.read_exact(&mut body).await.is_err() {
        return None;
    }
    serde_json::from_slice(&body).ok()
}

async fn write_response(
    stream: &mut (impl AsyncWriteExt + Unpin),
    response: &DaemonToClient,
) -> std::io::Result<()> {
    let frame = codec::encode_frame(response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    stream.write_all(&frame).await
}
