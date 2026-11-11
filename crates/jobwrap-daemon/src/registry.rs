//! The live job registry.
//!
//! Owns the map of running jobs, their wrapper connections, the SQLite store,
//! and the event broadcast used by the web layer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use chrono::Utc;
use jobwrap_config::{EffectiveConfig, RuntimePaths};
use jobwrap_core::{
    authorize, AccessLevel, Event, EventId, EventKind, JobId, JobRecord, JobState, Principal,
};
use jobwrap_protocol::{JobSummary, RegisterResult, ToWrapper};
use jobwrap_store::{LogLimits, OutputLog, Store};
use tokio::sync::{broadcast, mpsc};

use crate::launch::LaunchStore;
use crate::probe::ProbeStore;
use crate::registry::auth::AuthOps;
use crate::terminal::{RegisteredTerminal, TerminalRegistry};

/// A job with an attached wrapper.
#[derive(Debug)]
pub struct LiveJob {
    pub record: JobRecord,
    /// Channel to the wrapper's writer task (for input and control).
    pub wrapper_tx: Option<mpsc::Sender<ToWrapper>>,
    /// The append-only output log.
    pub log: Option<OutputLog>,
}

#[derive(Debug, Clone)]
struct IdempotencyEntry {
    request: jobwrap_protocol::LaunchRequest,
    launch_id: String,
    job_id: JobId,
    created_at: chrono::DateTime<Utc>,
}

/// The registry state.
pub struct Registry {
    store: Mutex<Store>,
    jobs: Mutex<HashMap<JobId, LiveJob>>,
    pub runtime: RuntimePaths,
    pub config: EffectiveConfig,
    broadcast_tx: broadcast::Sender<jobwrap_web::ServerEvent>,
    recorded_log_bytes: AtomicU64,
    /// Attempts for password login rate limiting.
    login_attempts: Mutex<Vec<chrono::DateTime<Utc>>>,
    /// Launch idempotency, scoped by requester. The original request is kept
    /// so reusing a key for different arguments can be rejected.
    idempotency: Mutex<HashMap<(String, String), IdempotencyEntry>>,
    /// Pending one-time launches and the launch concurrency counter.
    pub launch_store: LaunchStore,
    pub detached: crate::detached::DetachedStore,
    /// Registered terminals for cooperative existing-terminal launches.
    pub terminals: TerminalRegistry,
    /// Help-probe previews and results.
    pub probe_store: ProbeStore,
    /// Disconnected records without a known service that were already
    /// checked for a detached service, so they are probed once per run.
    probed_for_service: Mutex<std::collections::HashSet<JobId>>,
}

pub mod auth;

impl Registry {
    pub fn new(store: Store, runtime: RuntimePaths, config: EffectiveConfig) -> Self {
        let (broadcast_tx, _) = broadcast::channel(1024);
        let recorded_log_bytes = jobwrap_store::total_log_bytes(&runtime.logs_dir);
        Self {
            store: Mutex::new(store),
            jobs: Mutex::new(HashMap::new()),
            runtime,
            config,
            broadcast_tx,
            recorded_log_bytes: AtomicU64::new(recorded_log_bytes),
            login_attempts: Mutex::new(Vec::new()),
            idempotency: Mutex::new(HashMap::new()),
            launch_store: LaunchStore::new(),
            detached: crate::detached::DetachedStore::default(),
            terminals: TerminalRegistry::new(),
            probe_store: ProbeStore::new(),
            probed_for_service: Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn broadcast(&self) -> broadcast::Sender<jobwrap_web::ServerEvent> {
        self.broadcast_tx.clone()
    }

    fn log_limits(&self) -> LogLimits {
        LogLimits {
            maximum_log_bytes: self.config.defaults.maximum_log_bytes,
            maximum_total_bytes: 4 * 1024 * 1024 * 1024,
        }
    }

    /// Load jobs persisted in SQLite as disconnected records.
    pub fn load_persisted_jobs(&self) -> Result<(), jobwrap_store::StoreError> {
        let store = self.store.lock().expect("store lock");
        let records = store.list_jobs()?;
        let mut jobs = self.jobs.lock().expect("jobs lock");
        for record in records {
            let mut record = record;
            if !record.state.is_finished() {
                record.state = JobState::Disconnected;
            }
            jobs.insert(
                record.id,
                LiveJob {
                    record,
                    wrapper_tx: None,
                    log: None,
                },
            );
        }
        Ok(())
    }

    /// Register a wrapper's job.
    pub fn register_job(
        &self,
        register: jobwrap_protocol::RegisterJob,
        wrapper_tx: mpsc::Sender<ToWrapper>,
    ) -> Result<RegisterResult, String> {
        validate_registration(&register)?;
        let id = register.id;
        {
            let mut jobs = self.jobs.lock().expect("jobs lock");
            if let Some(job) = jobs.get_mut(&id) {
                if register.worker_identity.is_none()
                    || job.record.worker_identity != register.worker_identity
                    || job.record.wrapper_pid != Some(jobwrap_core::ProcessId(register.wrapper_pid))
                    || job.record.owner_uid != register.owner_uid
                    || job.wrapper_tx.as_ref().is_some_and(|tx| !tx.is_closed())
                {
                    return Err("job identity mismatch or wrapper already connected".into());
                }
                if !job.record.state.is_finished() {
                    job.record.state = JobState::Running;
                }
                if job.log.is_none() && register.record_output && self.config.defaults.record_output
                {
                    let mut log =
                        OutputLog::open(&self.runtime.logs_dir, id).map_err(|e| e.to_string())?;
                    log.recover(
                        job.record.log.bytes_written,
                        job.record.log.truncated,
                        self.log_limits(),
                    )
                    .map_err(|e| e.to_string())?;
                    job.log = Some(log);
                }
                job.wrapper_tx = Some(wrapper_tx);
                self.store
                    .lock()
                    .expect("store lock")
                    .update_job(&job.record)
                    .map_err(|e| e.to_string())?;
                let _ = self
                    .broadcast_tx
                    .send(jobwrap_web::ServerEvent::StateChanged {
                        job_id: id,
                        state: job.record.state,
                    });
                return Ok(RegisterResult {
                    job_id: id,
                    sequence_start: job.record.log.last_sequence,
                });
            }
        }
        let profile = self
            .config
            .profiles
            .get(&register.profile_name)
            .map(|p| p.access.clone())
            .ok_or_else(|| format!("unknown profile `{}`", register.profile_name))?;

        let record = JobRecord {
            id,
            display_name: jobwrap_core::JobName::new(register.display_name)
                .map_err(|e| e.to_string())?,
            owner_uid: register.owner_uid,
            wrapper_pid: Some(jobwrap_core::ProcessId(register.wrapper_pid)),
            child_pid: Some(jobwrap_core::ProcessId(register.child_pid)),
            process_group_id: Some(jobwrap_core::ProcessGroupId(register.process_group_id)),
            command: jobwrap_core::CommandDisplay::new([register.command.clone()])
                .map_err(|e| e.to_string())?,
            executable: register.executable.into(),
            arguments: register.arguments,
            working_directory: register.working_directory.into(),
            profile_name: register.profile_name,
            access_policy: profile.into_policy(),
            state: JobState::Running,
            started_at: Utc::now(),
            finished_at: None,
            terminal: jobwrap_core::TerminalMetadata {
                attached: register.terminal_attached,
                initial_size: register.terminal_size,
                device: register.terminal_device,
            },
            worker_identity: register.worker_identity,
            log: Default::default(),
            service_unit: self
                .detached
                .take_claim(id)
                .then(|| jobwrap_core::detached_service_unit(id)),
        };

        let mut jobs = self.jobs.lock().expect("jobs lock");
        if jobs.contains_key(&id) {
            return Err("job already registered".to_string());
        }
        if jobs.len() >= 10_000 {
            let oldest_finished = jobs
                .iter()
                .filter(|(_, job)| job.record.state.is_finished())
                .min_by_key(|(_, job)| job.record.started_at)
                .map(|(id, _)| *id);
            if let Some(oldest) = oldest_finished {
                jobs.remove(&oldest);
            } else {
                return Err("live job registry capacity reached".to_string());
            }
        }

        {
            let store = self.store.lock().expect("store lock");
            if store
                .get_job(id)
                .map_err(|e| format!("could not check job id: {e}"))?
                .is_some()
            {
                return Err("job id already exists in persistent storage".to_string());
            }
        }

        let log = if self.config.defaults.record_output && register.record_output {
            Some(
                OutputLog::create(&self.runtime.logs_dir, id, self.log_limits())
                    .map_err(|e| format!("could not create output log: {e}"))?,
            )
        } else {
            None
        };

        {
            let store = self.store.lock().expect("store lock");
            if let Err(error) = store.insert_job(&record) {
                drop(store);
                let _ = jobwrap_store::delete_log(&self.runtime.logs_dir, id);
                return Err(format!("could not persist job: {error}"));
            }
            if let Err(error) =
                store.insert_event(&Event::new(id, EventId(1), EventKind::Registered))
            {
                tracing::warn!(job_id = %id, error = %error, "could not persist registration event");
            }
        }

        jobs.insert(
            id,
            LiveJob {
                record,
                wrapper_tx: Some(wrapper_tx),
                log,
            },
        );

        let _ = self
            .broadcast_tx
            .send(jobwrap_web::ServerEvent::StateChanged {
                job_id: id,
                state: JobState::Running,
            });

        Ok(RegisterResult {
            job_id: id,
            sequence_start: 0,
        })
    }

    /// Launch a process via the API, web, or CLI.
    ///
    /// All modes funnel through [`crate::launch::execute`]. New-terminal
    /// launches create a one-time launch capability for the `jobwrap
    /// attach-launch` helper. Other modes currently fail closed.
    pub fn launch(
        &self,
        principal: &Principal,
        req: &jobwrap_protocol::LaunchRequest,
    ) -> Result<(String, JobId), String> {
        if !self.config.launch.enabled {
            return Err("process creation via the API is disabled by configuration".into());
        }
        if self.config.launch.require_preview {
            return Err(
                "launch preview is required by configuration, but preview execution is not implemented; no process was started"
                    .into(),
            );
        }
        validate_launch_request(self, req)?;
        let idempotency_key = &req.idempotency_key;
        if idempotency_key.is_empty() || idempotency_key.len() > 128 {
            return Err("idempotency key must contain 1 to 128 characters".into());
        }
        let requester = principal_label(principal);
        let map_key = (requester, idempotency_key.clone());

        // Idempotency: replaying the exact request returns the original result;
        // changing any field while reusing the key is an error.
        {
            let mut keys = self.idempotency.lock().expect("idempotency lock");
            let cutoff = Utc::now() - chrono::Duration::hours(24);
            keys.retain(|_, entry| entry.created_at >= cutoff);
            if let Some(entry) = keys.get(&map_key) {
                if entry.request != *req {
                    return Err(
                        "idempotency key was already used for a different launch request".into(),
                    );
                }
                return Ok((entry.launch_id.clone(), entry.job_id));
            }
        }

        if !self
            .launch_store
            .inc_concurrent(self.config.launch.maximum_concurrent_jobs)
        {
            return Err("launch capacity exceeded; too many concurrent jobs".into());
        }

        let result = crate::launch::execute(self, principal, req);

        // Only record idempotency on success.
        if let Ok((launch_id, job_id)) = &result {
            let mut keys = self.idempotency.lock().expect("idempotency lock");
            if keys.len() >= 1024 {
                if let Some(oldest) = keys
                    .iter()
                    .min_by_key(|(_, entry)| entry.created_at)
                    .map(|(key, _)| key.clone())
                {
                    keys.remove(&oldest);
                }
            }
            keys.insert(
                map_key,
                IdempotencyEntry {
                    request: req.clone(),
                    launch_id: launch_id.clone(),
                    job_id: *job_id,
                    created_at: Utc::now(),
                },
            );
        }
        // This counter limits simultaneous launch setup. Pending terminal
        // capabilities have their own separately bounded store.
        self.launch_store.dec_concurrent();
        result
    }

    /// The `attach-launch` helper retrieves a pending launch.
    pub fn attach_launch(
        &self,
        launch_id: &str,
    ) -> Result<(jobwrap_protocol::LaunchRequest, JobId), String> {
        crate::launch::attach_launch(self, launch_id)
    }

    // ---- terminals ----

    pub fn list_terminals(&self, principal: &Principal) -> Vec<jobwrap_protocol::TerminalInfo> {
        crate::launch::list_terminals(self, principal)
    }

    pub fn register_terminal(&self, terminal: RegisteredTerminal) -> Result<(), String> {
        crate::launch::register_terminal(self, terminal)
    }

    // ---- documentation ----

    pub fn identify_target(
        &self,
        principal: &Principal,
        target: &str,
    ) -> Result<jobwrap_protocol::TargetInfo, String> {
        use jobwrap_core::{authorize_global, AuthorizationDecision, GlobalPermission};
        match authorize_global(principal, GlobalPermission::InspectStaticMetadata) {
            AuthorizationDecision::Allow => {}
            _ => return Err("inspection not authorized".into()),
        }
        Ok(crate::docs::identify(target).info)
    }

    pub fn search_man_pages(
        &self,
        principal: &Principal,
        target: &str,
    ) -> Result<Vec<jobwrap_protocol::ManPageMatch>, String> {
        use jobwrap_core::{authorize_global, AuthorizationDecision, GlobalPermission};
        match authorize_global(principal, GlobalPermission::InspectManPage) {
            AuthorizationDecision::Allow => {}
            _ => return Err("man-page inspection not authorized".into()),
        }
        Ok(crate::docs::search_man_pages(target))
    }

    pub fn fetch_man_page(
        &self,
        principal: &Principal,
        name: &str,
        section: Option<String>,
    ) -> Result<Option<jobwrap_protocol::ManPage>, String> {
        use jobwrap_core::{authorize_global, AuthorizationDecision, GlobalPermission};
        match authorize_global(principal, GlobalPermission::InspectManPage) {
            AuthorizationDecision::Allow => {}
            _ => return Err("man-page inspection not authorized".into()),
        }
        Ok(crate::docs::fetch_man_page(name, section.as_deref()))
    }

    // ---- help probes ----

    pub fn preview_help_probe(
        &self,
        principal: &Principal,
        req: &jobwrap_protocol::HelpProbeRequest,
    ) -> Result<jobwrap_protocol::ProbePreview, String> {
        use jobwrap_core::{authorize_global, AuthorizationDecision, GlobalPermission};
        let permission = match req.kind {
            jobwrap_protocol::HelpProbeKind::Interpreter => GlobalPermission::ProbeInterpreterHelp,
            jobwrap_protocol::HelpProbeKind::Executable => GlobalPermission::ProbeExecutableHelp,
            jobwrap_protocol::HelpProbeKind::Script => GlobalPermission::ProbeScriptHelp,
            jobwrap_protocol::HelpProbeKind::Custom => GlobalPermission::ProbeScriptHelp,
        };
        match req.kind {
            jobwrap_protocol::HelpProbeKind::Interpreter
                if !self.config.help.allow_interpreter_probes =>
            {
                return Err("interpreter help probes are disabled by configuration".into())
            }
            jobwrap_protocol::HelpProbeKind::Executable
            | jobwrap_protocol::HelpProbeKind::Script
                if !self.config.help.allow_script_probes =>
            {
                return Err("target-executing help probes are disabled by configuration".into())
            }
            jobwrap_protocol::HelpProbeKind::Custom => {
                return Err("custom help probes are not supported".into())
            }
            _ => {}
        }
        match authorize_global(principal, permission) {
            AuthorizationDecision::Allow => {}
            AuthorizationDecision::Disabled => return Err("help probes are disabled".into()),
            _ => return Err("help probe not authorized".into()),
        }
        crate::probe::preview_help_probe(&self.probe_store, req)
    }

    pub fn execute_help_probe(
        &self,
        _principal: &Principal,
        preview_id: &str,
    ) -> Result<jobwrap_protocol::HelpProbeResult, String> {
        // Authorization was validated at preview time; the probe is bound to
        // the stored immutable preview.
        let timeout = Duration::from_secs(self.config.help.probe_timeout_seconds.clamp(1, 30));
        let limit = usize::try_from(self.config.help.probe_output_limit_bytes.clamp(1, 8 << 20))
            .unwrap_or(1 << 20);
        let result =
            crate::probe::execute_help_probe(&self.probe_store, preview_id, timeout, limit)?;

        // Phase 8: cache the classification keyed by target fingerprint and
        // probe argument.
        if self.config.help.cache_results {
            let identified = crate::docs::identify(&result.invocation.executable);
            let cache_key = crate::docs::cache_key(
                &identified.info.resolved_path,
                identified.info.fingerprint.as_deref().unwrap_or(""),
                &result.invocation.arguments.join(" "),
            );
            let store = self.store.lock().expect("store lock");
            let _ = store.help_cache_put(&jobwrap_store::HelpCacheEntry {
                key: cache_key,
                target_path: identified.info.resolved_path,
                target_fingerprint: identified.info.fingerprint.unwrap_or_default(),
                interpreter: identified.interpreter,
                probe_argument: result.invocation.arguments.join(" "),
                classification: result.classification.to_string(),
                output_digest: {
                    use sha2::Digest;
                    let d = sha2::Sha256::digest(result.stdout.as_bytes());
                    d.iter().map(|b| format!("{b:02x}")).collect::<String>()
                },
                warning: if result.warning.is_empty() {
                    None
                } else {
                    Some(result.warning.clone())
                },
                cached_at: Utc::now(),
            });
        }
        Ok(result)
    }

    pub fn get_help_probe(
        &self,
        principal: &Principal,
        probe_id: &str,
    ) -> Result<Option<jobwrap_protocol::HelpProbeResult>, String> {
        let _ = principal;
        Ok(self.probe_store.get_result(probe_id))
    }

    pub fn delete_help_probe(&self, principal: &Principal, probe_id: &str) -> Result<bool, String> {
        let _ = principal;
        Ok(self.probe_store.delete_result(probe_id))
    }

    /// Append a chunk of terminal output for a job.
    pub fn append_output(&self, job_id: JobId, sequence: u64, data: &[u8]) -> Result<(), String> {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let Some(job) = jobs.get_mut(&job_id) else {
            return Ok(());
        };
        if sequence <= job.record.log.last_sequence {
            return Ok(());
        }
        let previous = job.record.clone();
        if let Some(log) = job.log.as_mut() {
            let requested = data.len() as u64;
            let global_limit = self.log_limits().maximum_total_bytes;
            let reserved = !job.record.log.truncated
                && self
                    .recorded_log_bytes
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                        current
                            .checked_add(requested)
                            .filter(|next| *next <= global_limit)
                    })
                    .is_ok();
            let recording = reserved && log.append(data);
            if reserved && !recording {
                self.recorded_log_bytes
                    .fetch_sub(requested, Ordering::AcqRel);
            }
            log.sync().map_err(|e| e.to_string())?;
            job.record.log.last_sequence = sequence;
            job.record.log.bytes_written = log.size();
            job.record.log.truncated = log.is_truncated();
            if !recording {
                job.record.log.truncated = true;
            }
        } else {
            job.record.log.last_sequence = sequence;
        }
        if let Err(error) = self
            .store
            .lock()
            .expect("store lock")
            .update_job(&job.record)
        {
            job.record = previous;
            if let Some(log) = job.log.as_mut() {
                let _ = log.recover(
                    job.record.log.bytes_written,
                    job.record.log.truncated,
                    self.log_limits(),
                );
            }
            return Err(error.to_string());
        }
        let _ = self.broadcast_tx.send(jobwrap_web::ServerEvent::Output {
            job_id,
            sequence,
            data: data.to_vec(),
        });
        Ok(())
    }

    pub fn recording_truncated(&self, id: JobId) {
        if let Some(job) = self.jobs.lock().expect("jobs lock").get_mut(&id) {
            job.record.log.truncated = true;
            let _ = self
                .store
                .lock()
                .expect("store lock")
                .update_job(&job.record);
        }
    }

    /// Handle a wrapper state report (stopped/continued/exited/signaled).
    pub fn handle_wrapper_event(&self, job_id: JobId, event: jobwrap_protocol::WrapperToDaemon) {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let Some(job) = jobs.get_mut(&job_id) else {
            return;
        };
        let new_state = match event {
            jobwrap_protocol::WrapperToDaemon::Stopped => Some(JobState::Stopped),
            jobwrap_protocol::WrapperToDaemon::Continued => Some(JobState::Running),
            jobwrap_protocol::WrapperToDaemon::Exited { code } => Some(JobState::Exited { code }),
            jobwrap_protocol::WrapperToDaemon::Signaled { signal } => {
                Some(JobState::Signaled { signal })
            }
            jobwrap_protocol::WrapperToDaemon::Ready => None,
            jobwrap_protocol::WrapperToDaemon::TerminalLost => None,
            jobwrap_protocol::WrapperToDaemon::Bye => None,
        };
        if let Some(state) = new_state {
            if let Ok(state) = job.record.state.transition(state) {
                if state.is_finished() {
                    job.record.finished_at = Some(Utc::now());
                }
                job.record.state = state;
                let store = self.store.lock().expect("store lock");
                if let Err(error) = store.update_job(&job.record) {
                    tracing::error!(%job_id, %error, "persisting wrapper state failed");
                }
                if let Err(error) = store.insert_event(&Event::new(
                    job_id,
                    EventId(0),
                    EventKind::StateChanged { state },
                )) {
                    tracing::error!(%job_id, %error, "persisting wrapper state event failed");
                }
                let _ = self
                    .broadcast_tx
                    .send(jobwrap_web::ServerEvent::StateChanged { job_id, state });
            }
        }
    }

    /// Mark a wrapper as disconnected but keep the job alive.
    pub fn wrapper_disconnected(&self, job_id: JobId, reason: &str) {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        if let Some(job) = jobs.get_mut(&job_id) {
            job.wrapper_tx = None;
            if job.record.state.is_finished() || job.record.state == JobState::Disconnected {
                return;
            }
            if let Ok(state) = job.record.state.transition(JobState::Disconnected) {
                job.record.state = state;
                let store = self.store.lock().expect("store lock");
                if let Err(error) = store.update_job(&job.record) {
                    tracing::error!(%job_id, %error, "persisting wrapper disconnection failed");
                }
                for kind in [
                    EventKind::WrapperDisconnected,
                    EventKind::Audit { detail: format!("Wrapper connection lost: {reason}. Child outcome is unknown; the workload may still be running.") },
                    EventKind::StateChanged { state },
                ] {
                    if let Err(error) = store.insert_event(&Event::new(job_id, EventId(0), kind)) {
                        tracing::error!(%job_id, %error, "persisting wrapper disconnection event failed");
                    }
                }
                tracing::warn!(%job_id, reason, "wrapper disconnected; child outcome unknown");
                let _ = self
                    .broadcast_tx
                    .send(jobwrap_web::ServerEvent::StateChanged { job_id, state });
            }
        }
    }

    // ---- control ----

    pub fn send_input(
        &self,
        principal: &Principal,
        job_id: JobId,
        data: &[u8],
    ) -> Result<(), jobwrap_web::ApiError> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, jobwrap_core::Permission::SendInput)?;
        let tx = job.wrapper_tx.as_ref().ok_or_else(|| {
            jobwrap_web::ApiError::conflict("the wrapper is disconnected; input cannot be sent")
        })?;
        let msg = ToWrapper::SendInput {
            data_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
        };
        tx.try_send(msg)
            .map_err(|_| jobwrap_web::ApiError::conflict("the wrapper disconnected"))
    }

    pub fn send_signal(
        &self,
        principal: &Principal,
        job_id: JobId,
        signal: jobwrap_core::Signal,
    ) -> Result<(), jobwrap_web::ApiError> {
        let permission = match signal {
            jobwrap_core::Signal::Interrupt => jobwrap_core::Permission::SendInterrupt,
            jobwrap_core::Signal::Terminate => jobwrap_core::Permission::SendTerminate,
            jobwrap_core::Signal::Hangup => jobwrap_core::Permission::SendTerminate,
            jobwrap_core::Signal::Quit => jobwrap_core::Permission::SendInterrupt,
            jobwrap_core::Signal::Stop => jobwrap_core::Permission::SendStop,
            jobwrap_core::Signal::Continue => jobwrap_core::Permission::SendContinue,
            jobwrap_core::Signal::Kill => jobwrap_core::Permission::SendKill,
        };
        let jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, permission)?;
        if !job.record.state.is_controllable() {
            return Err(jobwrap_web::ApiError::conflict(
                "the job is no longer controllable",
            ));
        }
        let tx = job.wrapper_tx.as_ref().ok_or_else(|| {
            jobwrap_web::ApiError::conflict(
                "the wrapper is disconnected; refusing to signal a potentially reused process id",
            )
        })?;
        tx.try_send(ToWrapper::SendSignal { signal })
            .map_err(|_| jobwrap_web::ApiError::conflict("the wrapper disconnected"))?;
        let _ = self.store.lock().expect("store lock").record_audit(
            Some(job_id),
            &principal_label(principal),
            &format!("signal:{}", signal.short()),
            "",
        );
        Ok(())
    }

    pub fn resize(
        &self,
        principal: &Principal,
        job_id: JobId,
        ws: jobwrap_core::WindowSize,
    ) -> Result<(), jobwrap_web::ApiError> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, jobwrap_core::Permission::ViewStatus)?;
        if let Some(tx) = job.wrapper_tx.as_ref() {
            let _ = tx.try_send(ToWrapper::Resize { window_size: ws });
        }
        Ok(())
    }

    pub fn delete_job(
        &self,
        principal: &Principal,
        job_id: JobId,
    ) -> Result<(), jobwrap_web::ApiError> {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, jobwrap_core::Permission::Delete)?;
        if !job.record.state.is_finished() {
            return Err(jobwrap_web::ApiError::conflict(
                "a running or disconnected job record cannot be deleted; stop it and wait for completion first",
            ));
        }
        // Remove from the live registry and the database; keep or remove the
        // log according to the policy (we remove it for a delete).
        let freed = jobwrap_store::delete_log(&self.runtime.logs_dir, job_id)
            .map_err(|e| jobwrap_web::ApiError::internal(format!("could not delete log: {e}")))?;
        self.recorded_log_bytes.fetch_sub(
            freed.min(self.recorded_log_bytes.load(Ordering::Acquire)),
            Ordering::AcqRel,
        );
        self.store
            .lock()
            .expect("store lock")
            .delete_job(job_id)
            .map_err(|e| jobwrap_web::ApiError::internal(format!("could not delete job: {e}")))?;
        jobs.remove(&job_id);
        Ok(())
    }

    // ---- retention ----

    /// Delete finished jobs (exited, signaled, lost) that ended more than
    /// `older_than_days` ago, with their logs and events. Only ever called
    /// for an explicit `jobwrap prune` or with `[retention] auto_prune`.
    pub fn prune_finished_jobs(
        &self,
        older_than_days: u32,
        dry_run: bool,
        actor: &str,
    ) -> Result<(Vec<jobwrap_protocol::PrunedJob>, u64), String> {
        if older_than_days == 0 {
            return Err("refusing to prune with an age of 0 days".into());
        }
        let cutoff = jobwrap_protocol::query::days_ago(Utc::now(), older_than_days);
        let candidates = self
            .store
            .lock()
            .expect("store lock")
            .finished_jobs_before(cutoff)
            .map_err(|e| format!("could not list old jobs: {e}"))?;
        let describe = |record: &JobRecord| jobwrap_protocol::PrunedJob {
            id: record.id,
            display_name: record.display_name.as_str().to_string(),
            state: record.state,
            started_at: record.started_at,
            finished_at: record.finished_at,
        };
        if dry_run {
            return Ok((candidates.iter().map(describe).collect(), 0));
        }
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let mut store = self.store.lock().expect("store lock");
        let mut pruned = Vec::new();
        let mut freed = 0u64;
        for record in candidates {
            // The live registry is authoritative for state.
            if jobs
                .get(&record.id)
                .is_some_and(|job| !job.record.state.is_finished())
            {
                continue;
            }
            match jobwrap_store::delete_log(&self.runtime.logs_dir, record.id) {
                Ok(bytes) => freed += bytes,
                Err(error) => {
                    tracing::warn!(job_id = %record.id, %error, "could not delete log; keeping record");
                    continue;
                }
            }
            if let Err(error) = store.delete_job(record.id) {
                tracing::warn!(job_id = %record.id, %error, "could not delete job record");
                continue;
            }
            jobs.remove(&record.id);
            pruned.push(describe(&record));
        }
        self.recorded_log_bytes.fetch_sub(
            freed.min(self.recorded_log_bytes.load(Ordering::Acquire)),
            Ordering::AcqRel,
        );
        if !pruned.is_empty() {
            let _ = store.record_audit(
                None,
                actor,
                "prune",
                &format!(
                    "deleted {} finished jobs older than {older_than_days} days",
                    pruned.len()
                ),
            );
        }
        Ok((pruned, freed))
    }

    // ---- detached services ----

    /// The service to ask about for a disconnected record, and whether the
    /// record already knows it is detached.
    ///
    /// Records from before service tracking, or whose registration predates
    /// a daemon upgrade, are probed once under the deterministic unit name.
    fn service_candidate(&self, record: &JobRecord, force: bool) -> Option<(String, bool)> {
        if let Some(unit) = &record.service_unit {
            return Some((unit.clone(), true));
        }
        if record.state != JobState::Disconnected {
            return None;
        }
        let first_probe = self
            .probed_for_service
            .lock()
            .expect("probe set lock")
            .insert(record.id);
        (force || first_probe).then(|| (jobwrap_core::detached_service_unit(record.id), false))
    }

    /// Ask systemd about a job's detached service. `Ok(None)` for jobs that
    /// are not detached. Blocks on a short `systemctl` query.
    pub fn detached_service_status(
        &self,
        principal: &Principal,
        job_id: JobId,
    ) -> Result<Option<jobwrap_protocol::ServiceUnitStatus>, jobwrap_web::ApiError> {
        let record = self.get_job(principal, job_id)?;
        // A finished record keeps its unit name; the collected service has
        // nothing more to say.
        if record.state.is_finished() {
            return Ok(None);
        }
        let Some((unit, known)) = self.service_candidate(&record, true) else {
            return Ok(None);
        };
        let status = crate::service_unit::query(&unit);
        self.apply_service_status(job_id, &status, known);
        let detached = known || status.is_running();
        Ok(detached.then_some(status))
    }

    /// Recover a completed worker even if it exited before daemon restart.
    /// Only the stored identity and policy are trusted; journal registration
    /// cannot create jobs or replace their access permissions.
    fn recover_finished_journals(&self) {
        use jobwrap_protocol::ClientToDaemon as Msg;
        use std::io::{Read, Seek};
        use std::os::unix::fs::OpenOptionsExt;
        fn frame(file: &mut std::fs::File) -> Option<Msg> {
            let mut header = [0; 4];
            file.read_exact(&mut header).ok()?;
            let size = u32::from_be_bytes(header) as usize;
            if size > jobwrap_protocol::codec::MAX_FRAME_BYTES {
                return None;
            }
            let mut body = vec![0; size];
            file.read_exact(&mut body).ok()?;
            serde_json::from_slice(&body).ok()
        }
        let candidates: Vec<_> = self
            .jobs
            .lock()
            .expect("jobs lock")
            .values()
            .filter(|job| {
                job.record.state == JobState::Disconnected || job.record.state == JobState::Lost
            })
            .filter(|job| job.record.worker_identity.is_some())
            .map(|job| job.record.clone())
            .collect();
        for record in candidates {
            let path = self
                .runtime
                .logs_dir
                .join(format!("{}.recovery", record.id));
            let Ok(mut file) = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(path)
            else {
                continue;
            };
            let Some(Msg::RegisterJob(reg)) = frame(&mut file) else {
                continue;
            };
            if reg.id != record.id
                || reg.worker_identity != record.worker_identity
                || reg.owner_uid != record.owner_uid
                || Some(jobwrap_core::ProcessId(reg.wrapper_pid)) != record.wrapper_pid
            {
                continue;
            }
            let mut final_state = None;
            while let Some(msg) = frame(&mut file) {
                match msg {
                    Msg::WrapperExited { code } => final_state = Some(JobState::Exited { code }),
                    Msg::WrapperSignaled { signal } => {
                        final_state = Some(JobState::Signaled { signal })
                    }
                    _ => {}
                }
            }
            let Some(state) = final_state else {
                continue;
            };
            // Freeze the on-disk boundary; a final report is written only
            // after PTY EOF, so no later output can race this recovery.
            {
                let mut jobs = self.jobs.lock().expect("jobs lock");
                let Some(job) = jobs.get_mut(&record.id) else {
                    continue;
                };
                if job.wrapper_tx.as_ref().is_some_and(|tx| !tx.is_closed()) {
                    continue;
                }
                if reg.record_output && self.config.defaults.record_output && job.log.is_none() {
                    let Ok(mut log) = OutputLog::open(&self.runtime.logs_dir, record.id) else {
                        continue;
                    };
                    if log
                        .recover(
                            job.record.log.bytes_written,
                            job.record.log.truncated,
                            self.log_limits(),
                        )
                        .is_err()
                    {
                        continue;
                    }
                    job.log = Some(log);
                }
            }
            if file.rewind().is_err() {
                continue;
            }
            let mut failed = false;
            while let Some(msg) = frame(&mut file) {
                match msg {
                    Msg::Output {
                        sequence,
                        data_base64,
                    } => {
                        if let Some(data) = jobwrap_protocol::decode_input(&data_base64) {
                            if self.append_output(record.id, sequence, &data).is_err() {
                                failed = true;
                                break;
                            }
                        }
                    }
                    Msg::WrapperRecordingTruncated => self.recording_truncated(record.id),
                    _ => {}
                }
            }
            if failed {
                continue;
            }
            let mut jobs = self.jobs.lock().expect("jobs lock");
            if let Some(job) = jobs.get_mut(&record.id) {
                job.record.state = state;
                job.record.finished_at = file
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(chrono::DateTime::<Utc>::from);
                let store = self.store.lock().expect("store lock");
                if store.update_job(&job.record).is_ok() {
                    let _ = store.insert_event(&Event::new(
                        record.id,
                        EventId(0),
                        EventKind::Audit {
                            detail:
                                "Recovered worker output and final result from durable journal."
                                    .into(),
                        },
                    ));
                    let _ = self
                        .broadcast_tx
                        .send(jobwrap_web::ServerEvent::StateChanged {
                            job_id: record.id,
                            state,
                        });
                }
            }
        }
    }

    /// Settle disconnected detached jobs against the service manager.
    pub fn reconcile_detached_services(&self) {
        self.recover_finished_journals();
        let candidates: Vec<(JobId, String, bool)> = {
            let jobs = self.jobs.lock().expect("jobs lock");
            jobs.values()
                .filter(|job| job.record.state == JobState::Disconnected)
                .filter_map(|job| {
                    self.service_candidate(&job.record, false)
                        .map(|(unit, known)| (job.record.id, unit, known))
                })
                .collect()
        };
        for (job_id, unit, known) in candidates {
            let status = crate::service_unit::query(&unit);
            self.apply_service_status(job_id, &status, known);
        }
    }

    /// Record what the service manager said about a disconnected job.
    ///
    /// A running service under the job's own name identifies an older
    /// detached record. A known detached service that has ended means the
    /// workload is gone while nobody was connected: the job becomes `lost`
    /// because its exit status cannot be recovered. An unreachable manager
    /// changes nothing.
    fn apply_service_status(
        &self,
        job_id: JobId,
        status: &jobwrap_protocol::ServiceUnitStatus,
        known: bool,
    ) {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let Some(job) = jobs.get_mut(&job_id) else {
            return;
        };
        if job.record.state != JobState::Disconnected {
            return;
        }
        let store = self.store.lock().expect("store lock");
        if !known {
            if !status.is_running() {
                return;
            }
            job.record.service_unit = Some(status.unit.clone());
            if let Err(error) = store.update_job(&job.record) {
                tracing::error!(%job_id, %error, "persisting detached service failed");
            }
            let detail = format!(
                "Found the running detached service {} for this record.",
                status.unit
            );
            let _ =
                store.insert_event(&Event::new(job_id, EventId(0), EventKind::Audit { detail }));
            return;
        }
        if !status.has_ended() {
            return;
        }
        let Ok(state) = job.record.state.transition(JobState::Lost) else {
            return;
        };
        job.record.state = state;
        // The end happened at some point before now; now is the only bound.
        job.record.finished_at = Some(Utc::now());
        if let Err(error) = store.update_job(&job.record) {
            tracing::error!(%job_id, %error, "persisting ended detached service failed");
        }
        let detail = format!(
            "The detached service {} is no longer running ({}). The daemon was not connected when the workload ended, so its exit status is unknown; the finish time is when this was noticed.",
            status.unit,
            status.describe()
        );
        for kind in [
            EventKind::Audit { detail },
            EventKind::StateChanged { state },
        ] {
            if let Err(error) = store.insert_event(&Event::new(job_id, EventId(0), kind)) {
                tracing::error!(%job_id, %error, "persisting detached service event failed");
            }
        }
        tracing::info!(%job_id, unit = %status.unit, "detached service ended while disconnected");
        let _ = self
            .broadcast_tx
            .send(jobwrap_web::ServerEvent::StateChanged { job_id, state });
    }

    // ---- queries ----

    pub fn list_jobs(&self, principal: &Principal) -> Vec<JobSummary> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let mut out: Vec<JobSummary> = jobs
            .values()
            .filter(|job| {
                matches!(
                    authorize(principal, &job.record, jobwrap_core::Permission::ViewStatus),
                    jobwrap_core::AuthorizationDecision::Allow
                )
            })
            .map(to_summary)
            .collect();
        out.sort_by_key(|j| j.started_at);
        out.reverse();
        out
    }

    pub fn get_job(
        &self,
        principal: &Principal,
        job_id: JobId,
    ) -> Result<JobRecord, jobwrap_web::ApiError> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, jobwrap_core::Permission::ViewStatus)?;
        Ok(job.record.clone())
    }

    /// Check live output access independently of log availability or offsets.
    pub fn authorize_output(
        &self,
        principal: &Principal,
        job_id: JobId,
    ) -> Result<(), jobwrap_web::ApiError> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, jobwrap_core::Permission::ViewOutput)
    }

    pub fn get_output(
        &self,
        principal: &Principal,
        job_id: JobId,
        sequence_start: u64,
    ) -> Result<jobwrap_protocol::OutputSlice, jobwrap_web::ApiError> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, jobwrap_core::Permission::ViewOutput)?;
        let data = match jobwrap_store::OutputLog::open(&self.runtime.logs_dir, job_id) {
            Ok(mut log) => log
                .read_at(sequence_start, 1024 * 1024)
                .map_err(|e| jobwrap_web::ApiError::internal(format!("could not read log: {e}")))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(jobwrap_web::ApiError::internal(format!(
                    "could not open log: {error}"
                )))
            }
        };
        Ok(jobwrap_protocol::OutputSlice {
            job_id,
            sequence_start,
            data_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
            truncated: job.record.log.truncated,
        })
    }

    pub fn get_events(
        &self,
        principal: &Principal,
        job_id: JobId,
    ) -> Result<Vec<Event>, jobwrap_web::ApiError> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, jobwrap_core::Permission::ViewStatus)?;
        let store = self.store.lock().expect("store lock");
        store
            .events_for_job(job_id)
            .map_err(|e| jobwrap_web::ApiError::internal(e.to_string()))
    }

    // ---- auth plumbing ----

    pub fn auth(&self) -> AuthOps<'_> {
        AuthOps { registry: self }
    }
}

fn validate_registration(register: &jobwrap_protocol::RegisterJob) -> Result<(), String> {
    if register.wrapper_pid <= 0
        || register.child_pid <= 0
        || register.process_group_id <= 0
        || register.session_id <= 0
    {
        return Err("registration contains an invalid process id".into());
    }
    if register.command.len() > 64 * 1024
        || register.executable.len() > 4096
        || register.working_directory.len() > 4096
        || register.profile_name.len() > 128
        || register.executable.contains('\0')
        || register.working_directory.contains('\0')
        || register
            .terminal_device
            .as_ref()
            .is_some_and(|device| device.len() > 4096 || device.contains('\0'))
    {
        return Err("registration metadata exceeds size limits".into());
    }
    if register.arguments.len() > 4096
        || register
            .arguments
            .iter()
            .try_fold(0usize, |total, argument| total.checked_add(argument.len()))
            .map_or(true, |total| total > 256 * 1024)
        || register
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
    {
        return Err("registration arguments exceed size limits".into());
    }
    Ok(())
}

fn validate_launch_request(
    registry: &Registry,
    request: &jobwrap_protocol::LaunchRequest,
) -> Result<(), String> {
    if request.executable.is_empty()
        || request.executable.len() > 4096
        || request.executable.contains('\0')
    {
        return Err("executable must contain 1 to 4096 non-NUL bytes".into());
    }
    if request.arguments.len() > 4096
        || request
            .arguments
            .iter()
            .try_fold(0usize, |total, argument| total.checked_add(argument.len()))
            .map_or(true, |total| total > 256 * 1024)
        || request
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
    {
        return Err("launch arguments exceed size limits or contain NUL".into());
    }
    if let Some(name) = &request.display_name {
        jobwrap_core::JobName::new(name.clone()).map_err(|error| error.to_string())?;
    }
    let profile = request
        .profile_name
        .as_deref()
        .unwrap_or(&registry.config.launch.default_profile);
    if !registry.config.profiles.contains_key(profile) {
        return Err(format!("unknown profile `{profile}`"));
    }
    if let Some(directory) = &request.working_directory {
        if directory.len() > 4096 || !std::path::Path::new(directory).is_dir() {
            return Err("working directory does not exist or exceeds 4096 bytes".into());
        }
    }
    Ok(())
}

pub(crate) fn check_authorized(
    principal: &Principal,
    record: &JobRecord,
    permission: jobwrap_core::Permission,
) -> Result<(), jobwrap_web::ApiError> {
    match authorize(principal, record, permission) {
        jobwrap_core::AuthorizationDecision::Allow => Ok(()),
        jobwrap_core::AuthorizationDecision::Disabled => Err(
            jobwrap_web::ApiError::permission_denied("this operation is disabled for this job"),
        ),
        jobwrap_core::AuthorizationDecision::Deny { .. } => {
            Err(jobwrap_web::ApiError::permission_denied(format!(
                "you are not allowed to {}",
                permission.describe()
            )))
        }
    }
}

fn principal_label(principal: &Principal) -> String {
    match principal {
        Principal::Anonymous => "anonymous".to_string(),
        Principal::BrowserSession { session_id } => format!("session:{session_id}"),
        Principal::ApiToken { token_id, .. } => format!("token:{token_id}"),
        Principal::LocalUnixUser { uid } => format!("unix:{uid}"),
    }
}

fn to_summary(job: &LiveJob) -> JobSummary {
    summary_for(&job.record)
}

/// Build a job summary from a record.
pub fn summary_for(record: &JobRecord) -> JobSummary {
    let visibility = match record.access_policy.output {
        AccessLevel::Public => "public".to_string(),
        AccessLevel::Authenticated => "authenticated".to_string(),
        AccessLevel::Owner => "private".to_string(),
        _ => "restricted".to_string(),
    };
    JobSummary {
        id: record.id,
        display_name: record.display_name.as_str().to_string(),
        profile_name: record.profile_name.clone(),
        state: record.state,
        owner_uid: record.owner_uid,
        wrapper_pid: record.wrapper_pid.map(|p| p.0),
        child_pid: record.child_pid.map(|p| p.0),
        started_at: record.started_at,
        finished_at: record.finished_at,
        terminal: record.terminal.clone(),
        last_sequence: record.log.last_sequence,
        output_bytes: record.log.bytes_written,
        log_truncated: record.log.truncated,
        visibility,
        service_unit: record.service_unit.clone(),
    }
}
