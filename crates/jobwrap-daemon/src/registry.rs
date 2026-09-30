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
    /// Registered terminals for cooperative existing-terminal launches.
    pub terminals: TerminalRegistry,
    /// Help-probe previews and results.
    pub probe_store: ProbeStore,
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
            terminals: TerminalRegistry::new(),
            probe_store: ProbeStore::new(),
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
            log: Default::default(),
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
    pub fn append_output(&self, job_id: JobId, sequence: u64, data: &[u8]) {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let Some(job) = jobs.get_mut(&job_id) else {
            return;
        };
        if let Some(log) = job.log.as_mut() {
            let previous_bytes = job.record.log.bytes_written;
            let was_truncated = job.record.log.truncated;
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
            job.record.log.last_sequence = sequence;
            job.record.log.bytes_written = log.size();
            job.record.log.truncated = log.is_truncated();
            if !recording {
                job.record.log.truncated = true;
            }
            if previous_bytes / (1024 * 1024) != job.record.log.bytes_written / (1024 * 1024)
                || was_truncated != job.record.log.truncated
            {
                let _ = self
                    .store
                    .lock()
                    .expect("store lock")
                    .update_job(&job.record);
            }
        } else {
            job.record.log.last_sequence = sequence;
        }
        let _ = self.broadcast_tx.send(jobwrap_web::ServerEvent::Output {
            job_id,
            sequence,
            data: data.to_vec(),
        });
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
            jobwrap_protocol::WrapperToDaemon::Exited { code } => {
                job.record.finished_at = Some(Utc::now());
                Some(JobState::Exited { code })
            }
            jobwrap_protocol::WrapperToDaemon::Signaled { signal } => {
                job.record.finished_at = Some(Utc::now());
                Some(JobState::Signaled { signal })
            }
            jobwrap_protocol::WrapperToDaemon::Ready => None,
            jobwrap_protocol::WrapperToDaemon::TerminalLost => None,
            jobwrap_protocol::WrapperToDaemon::Bye => None,
        };
        if let Some(state) = new_state {
            if let Ok(state) = job.record.state.transition(state) {
                job.record.state = state;
                let _ = self
                    .store
                    .lock()
                    .expect("store lock")
                    .update_job(&job.record);
                let _ = self
                    .broadcast_tx
                    .send(jobwrap_web::ServerEvent::StateChanged { job_id, state });
            }
        }
    }

    /// Mark a wrapper as disconnected but keep the job alive.
    pub fn wrapper_disconnected(&self, job_id: JobId) {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        if let Some(job) = jobs.get_mut(&job_id) {
            job.wrapper_tx = None;
            if let Ok(state) = job.record.state.transition(JobState::Disconnected) {
                job.record.state = state;
                let _ = self
                    .store
                    .lock()
                    .expect("store lock")
                    .update_job(&job.record);
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
    }
}
