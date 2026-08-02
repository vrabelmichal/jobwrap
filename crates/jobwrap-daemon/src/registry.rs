//! The live job registry.
//!
//! Owns the map of running jobs, their wrapper connections, the SQLite store,
//! and the event broadcast used by the web layer.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::Utc;
use jobwrap_config::{EffectiveConfig, RuntimePaths};
use jobwrap_core::{
    authorize, AccessLevel, Event, EventId, EventKind, JobId, JobRecord, JobState, Principal,
};
use jobwrap_protocol::{JobSummary, RegisterResult, ToWrapper};
use jobwrap_store::{LogLimits, OutputLog, Store};
use tokio::sync::{broadcast, mpsc};

use crate::registry::auth::AuthOps;

/// A job with an attached wrapper.
#[derive(Debug)]
pub struct LiveJob {
    pub record: JobRecord,
    /// Channel to the wrapper's writer task (for input and control).
    pub wrapper_tx: Option<mpsc::Sender<ToWrapper>>,
    /// The append-only output log.
    pub log: Option<OutputLog>,
}

/// The registry state.
pub struct Registry {
    store: Mutex<Store>,
    jobs: Mutex<HashMap<JobId, LiveJob>>,
    pub runtime: RuntimePaths,
    pub config: EffectiveConfig,
    broadcast_tx: broadcast::Sender<jobwrap_web::ServerEvent>,
    /// Attempts for password login rate limiting.
    login_attempts: Mutex<Vec<chrono::DateTime<Utc>>>,
}

pub mod auth;

impl Registry {
    pub fn new(store: Store, runtime: RuntimePaths, config: EffectiveConfig) -> Self {
        let (broadcast_tx, _) = broadcast::channel(1024);
        Self {
            store: Mutex::new(store),
            jobs: Mutex::new(HashMap::new()),
            runtime,
            config,
            broadcast_tx,
            login_attempts: Mutex::new(Vec::new()),
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
            if record.state.is_finished() {
                continue;
            }
            let mut record = record;
            record.state = JobState::Disconnected;
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

        let log = if self.config.defaults.record_output {
            OutputLog::create(&self.runtime.logs_dir, id, self.log_limits()).ok()
        } else {
            None
        };

        {
            let store = self.store.lock().expect("store lock");
            let _ = store.insert_job(&record);
            let _ = store.insert_event(&Event::new(id, EventId(1), EventKind::Registered));
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

    /// Append a chunk of terminal output for a job.
    pub fn append_output(&self, job_id: JobId, sequence: u64, data: &[u8]) {
        let mut jobs = self.jobs.lock().expect("jobs lock");
        let Some(job) = jobs.get_mut(&job_id) else {
            return;
        };
        if let Some(log) = job.log.as_mut() {
            let recording = log.append(data);
            job.record.log.last_sequence = sequence;
            job.record.log.bytes_written = log.size();
            job.record.log.truncated = log.is_truncated();
            if !recording {
                job.record.log.truncated = true;
            }
            let _ = self
                .store
                .lock()
                .expect("store lock")
                .update_job(&job.record);
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
        let pgid = job
            .record
            .process_group_id
            .ok_or_else(|| jobwrap_web::ApiError::conflict("no process group recorded"))?;
        let nix_signal = match signal {
            jobwrap_core::Signal::Interrupt => nix::sys::signal::Signal::SIGINT,
            jobwrap_core::Signal::Terminate => nix::sys::signal::Signal::SIGTERM,
            jobwrap_core::Signal::Hangup => nix::sys::signal::Signal::SIGHUP,
            jobwrap_core::Signal::Quit => nix::sys::signal::Signal::SIGQUIT,
            jobwrap_core::Signal::Stop => nix::sys::signal::Signal::SIGSTOP,
            jobwrap_core::Signal::Continue => nix::sys::signal::Signal::SIGCONT,
            jobwrap_core::Signal::Kill => nix::sys::signal::Signal::SIGKILL,
        };
        nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid.0), nix_signal)
            .map_err(|_| jobwrap_web::ApiError::conflict("the process group no longer exists"))?;
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
        // Remove from the live registry and the database; keep or remove the
        // log according to the policy (we remove it for a delete).
        let _ = jobwrap_store::delete_log(&self.runtime.logs_dir, job_id);
        let _ = self.store.lock().expect("store lock").delete_job(job_id);
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

    pub fn get_output(
        &self,
        principal: &Principal,
        job_id: JobId,
        _sequence_start: u64,
    ) -> Result<jobwrap_protocol::OutputSlice, jobwrap_web::ApiError> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let job = jobs
            .get(&job_id)
            .ok_or_else(|| jobwrap_web::ApiError::not_found("no such job"))?;
        check_authorized(principal, &job.record, jobwrap_core::Permission::ViewOutput)?;
        let data = if job.log.is_some() {
            match jobwrap_store::OutputLog::open(&self.runtime.logs_dir, job_id) {
                Ok(mut log) => log.read_at(0, 4 * 1024 * 1024).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        Ok(jobwrap_protocol::OutputSlice {
            job_id,
            sequence_start: 0,
            data_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
            truncated: false,
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

fn check_authorized(
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
