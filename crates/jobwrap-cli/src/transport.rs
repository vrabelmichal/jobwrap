//! A bounded disk queue decouples PTY draining from daemon availability.
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::wrap::{Finalizer, WrapperLink};
use jobwrap_protocol::{codec, ClientToDaemon, DaemonToClient, ToWrapper};
use jobwrap_pty::{ChildStateChange, DaemonCommand, RelaySinks};

struct Journal {
    file: File,
    bytes: u64,
    limit: u64,
    warned: bool,
}
impl Journal {
    fn append(&mut self, msg: &ClientToDaemon) -> std::io::Result<()> {
        let frame = codec::encode_frame(msg).map_err(other_error)?;
        if matches!(msg, ClientToDaemon::Output { .. })
            && self.bytes + frame.len() as u64 > self.limit
        {
            if !self.warned {
                eprintln!("jobwrap: recovery journal full; further output will not be recorded");
                self.warned = true;
                self.append(&ClientToDaemon::WrapperRecordingTruncated)?;
            }
            return Ok(());
        }
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&frame)?;
        self.bytes += frame.len() as u64;
        self.file.sync_data()
    }
    fn next(&mut self, offset: u64) -> std::io::Result<Option<(ClientToDaemon, u64)>> {
        if offset >= self.bytes {
            return Ok(None);
        }
        self.file.seek(SeekFrom::Start(offset))?;
        let mut header = [0; 4];
        self.file.read_exact(&mut header)?;
        let size = u32::from_be_bytes(header) as usize;
        let mut body = vec![0; size];
        self.file.read_exact(&mut body)?;
        let msg = serde_json::from_slice(&body).map_err(other_error)?;
        Ok(Some((msg, offset + 4 + size as u64)))
    }
}

pub(crate) fn sinks(
    link: WrapperLink,
    commands: mpsc::Sender<DaemonCommand>,
) -> (RelaySinks, Option<Finalizer>) {
    let path = link
        .paths
        .logs_dir
        .join(format!("{}.recovery", link.job_id));
    let journal_dir = std::fs::create_dir_all(&link.paths.logs_dir).and_then(|_| {
        let meta = std::fs::symlink_metadata(&link.paths.logs_dir)?;
        if meta.is_dir() && !meta.file_type().is_symlink() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid recovery directory",
            ))
        }
    });
    let journal = journal_dir
        .and_then(|_| {
            OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
        })
        .map(|file| {
            Arc::new(Mutex::new(Journal {
                file,
                bytes: 0,
                limit: 512 * 1024 * 1024,
                warned: false,
            }))
        });
    let journal = match journal {
        Ok(j) => j,
        Err(e) => {
            eprintln!(
                "jobwrap: cannot create recovery journal {}: {e}",
                path.display()
            );
            return (RelaySinks::default(), None);
        }
    };
    if let Err(e) = journal
        .lock()
        .expect("journal lock")
        .append(&ClientToDaemon::RegisterJob(link.register.clone()))
    {
        eprintln!("jobwrap: recovery identity journal failed: {e}");
    }
    let (tx, rx) = mpsc::sync_channel(256);
    let output_journal = journal.clone();
    let state_journal = journal.clone();
    let live_tx = tx.clone();
    let record = link.register.record_output;
    let mut sequence = 0;
    let sinks = RelaySinks {
        on_output: Some(Box::new(move |data| {
            sequence += 1;
            let msg = ClientToDaemon::Output {
                sequence,
                data_base64: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    data,
                ),
            };
            if record {
                if let Err(e) = output_journal.lock().expect("journal lock").append(&msg) {
                    eprintln!("jobwrap: output journal failed: {e}");
                }
            } else {
                let _ = live_tx.try_send(msg);
            }
        })),
        on_state: Some(Box::new(move |state| {
            let msg = match state {
                ChildStateChange::Stopped => ClientToDaemon::WrapperStopped,
                ChildStateChange::Continued => ClientToDaemon::WrapperContinued,
            };
            let _ = state_journal.lock().expect("journal lock").append(&msg);
        })),
        on_signal: None,
    };
    let handle = std::thread::spawn(move || {
        let mut connection = Some(link.read);
        let mut responses = Responses::new();
        let mut offset = 0;
        let mut pending = None;
        let mut finished_at = None;
        loop {
            // Persist final status even when the daemon is offline. The worker
            // briefly retries delivery; the daemon can recover after worker exit.
            while let Ok(msg) = rx.try_recv() {
                if matches!(msg, ClientToDaemon::WrapperBye) {
                    continue;
                }
                if matches!(msg, ClientToDaemon::Output { .. }) {
                    if connection.is_some() && pending.is_none() {
                        pending = Some((msg, offset));
                    }
                } else if let Err(e) = journal.lock().expect("journal lock").append(&msg) {
                    eprintln!("jobwrap: final status journal failed: {e}");
                } else if matches!(
                    msg,
                    ClientToDaemon::WrapperExited { .. } | ClientToDaemon::WrapperSignaled { .. }
                ) {
                    finished_at = Some(std::time::Instant::now());
                }
            }
            if finished_at.map_or(false, |at| {
                std::time::Instant::now().duration_since(at) > Duration::from_secs(3)
            }) {
                eprintln!("jobwrap: final result retained for daemon recovery");
                break;
            }
            if connection.is_none() {
                if let Ok((mut read, mut write, _)) =
                    crate::daemon::connect_wrapper_stream(&link.paths, false)
                {
                    let result =
                        codec::encode_frame(&ClientToDaemon::RegisterJob(link.register.clone()))
                            .map_err(other_error)
                            .and_then(|frame| write.write_all(&frame));
                    let _ = read.set_read_timeout(Some(Duration::from_secs(5)));
                    if result.is_ok()
                        && matches!(
                            codec::read_frame(&mut read),
                            Ok(DaemonToClient::Registered(_))
                        )
                    {
                        responses = Responses::new();
                        connection = Some(read);
                        // Replaying the journal is idempotent by sequence.
                        offset = 0;
                        pending = None;
                    }
                }
                if connection.is_none() {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
            if pending.is_none() {
                match journal.lock().expect("journal lock").next(offset) {
                    Ok(msg) => pending = msg,
                    Err(e) => {
                        eprintln!("jobwrap: recovery journal read failed: {e}");
                        break;
                    }
                }
            }
            if let Some((ClientToDaemon::RegisterJob(_), next)) = pending.as_ref() {
                offset = *next;
                pending = None;
                continue;
            }
            let stream = connection.as_mut().expect("connection");
            if let Some((msg, next)) = pending.as_ref() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let sent = codec::encode_frame(msg)
                    .map_err(other_error)
                    .and_then(|frame| stream.write_all(&frame));
                let mut acknowledged = false;
                if sent.is_ok() {
                    loop {
                        match responses.next(stream) {
                            Ok(DaemonToClient::WrapperAck) => {
                                acknowledged = true;
                                break;
                            }
                            Ok(DaemonToClient::ToWrapper(cmd)) => forward(cmd, &commands),
                            _ => break,
                        }
                    }
                }
                if acknowledged {
                    if matches!(
                        msg,
                        ClientToDaemon::WrapperExited { .. }
                            | ClientToDaemon::WrapperSignaled { .. }
                    ) {
                        break;
                    }
                    offset = *next;
                    pending = None;
                } else {
                    connection = None;
                }
            } else {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                match responses.next(stream) {
                    Ok(DaemonToClient::ToWrapper(cmd)) => forward(cmd, &commands),
                    Ok(_) => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => connection = None,
                }
            }
        }
    });
    (
        sinks,
        Some(Finalizer {
            writer_tx: Some(tx),
            writer_handle: Some(handle),
        }),
    )
}
fn forward(cmd: ToWrapper, tx: &mpsc::Sender<DaemonCommand>) {
    let cmd = match cmd {
        ToWrapper::SendInput { data_base64 } => {
            DaemonCommand::Input(jobwrap_protocol::decode_input(&data_base64).unwrap_or_default())
        }
        ToWrapper::SendSignal { signal } => DaemonCommand::Signal(signal),
        ToWrapper::Resize { window_size } => DaemonCommand::Resize(window_size),
    };
    let _ = tx.send(cmd);
}

fn other_error(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e)
}

struct Responses {
    decoder: codec::FrameDecoder,
    pending: std::collections::VecDeque<DaemonToClient>,
}
impl Responses {
    fn new() -> Self {
        Self {
            decoder: codec::FrameDecoder::new(),
            pending: std::collections::VecDeque::new(),
        }
    }
    fn next(
        &mut self,
        stream: &mut std::os::unix::net::UnixStream,
    ) -> std::io::Result<DaemonToClient> {
        loop {
            if let Some(msg) = self.pending.pop_front() {
                return Ok(msg);
            }
            let mut bytes = [0; 8192];
            let n = stream.read(&mut bytes)?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "daemon disconnected",
                ));
            }
            self.pending
                .extend(self.decoder.push(&bytes[..n]).map_err(other_error)?);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_control_frame_survives_a_read_timeout() {
        let (mut client, mut server) = std::os::unix::net::UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let frame = codec::encode_frame(&DaemonToClient::WrapperAck).unwrap();
        server.write_all(&frame[..2]).unwrap();
        let mut responses = Responses::new();
        assert!(responses.next(&mut client).is_err());
        server.write_all(&frame[2..]).unwrap();
        assert_eq!(
            responses.next(&mut client).unwrap(),
            DaemonToClient::WrapperAck
        );
    }
}
