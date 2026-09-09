//! Append-only terminal output logs.
//!
//! Raw terminal bytes are stored per job in `{job_id}.terminal`. An optional
//! index file `{job_id}.index` maps sequence numbers to byte offsets so log
//! reads can resume from arbitrary points.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use jobwrap_core::JobId;

/// Limits applied to output logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLimits {
    /// Maximum bytes recorded for a single job before recording stops.
    pub maximum_log_bytes: u64,
    /// Maximum total bytes across all logs.
    pub maximum_total_bytes: u64,
}

impl Default for LogLimits {
    fn default() -> Self {
        Self {
            maximum_log_bytes: 512 * 1024 * 1024,
            maximum_total_bytes: 4 * 1024 * 1024 * 1024,
        }
    }
}

/// A per-job append-only output log.
#[derive(Debug)]
pub struct OutputLog {
    file: File,
    path: PathBuf,
    bytes_written: u64,
    truncated: bool,
    limits: LogLimits,
}

impl OutputLog {
    /// The log file name for a job id.
    pub fn file_name(job_id: JobId) -> String {
        format!("{job_id}.terminal")
    }

    fn path_for(dir: &Path, job_id: JobId) -> PathBuf {
        dir.join(Self::file_name(job_id))
    }

    /// Open an existing log for reading.
    pub fn open(dir: &Path, job_id: JobId) -> std::io::Result<Self> {
        validate_log_dir(dir)?;
        let path = Self::path_for(dir, job_id);
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW);
        let file = options.open(&path)?;
        let bytes_written = file.metadata()?.len();
        Ok(Self {
            file,
            path,
            bytes_written,
            truncated: false,
            limits: LogLimits::default(),
        })
    }

    /// Create a new log for writing. Existing paths are never truncated and
    /// symlinks are rejected, even if a caller supplies a colliding job id.
    pub fn create(dir: &Path, job_id: JobId, limits: LogLimits) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        validate_log_dir(dir)?;
        #[cfg(unix)]
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let path = Self::path_for(dir, job_id);
        let mut options = OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options.open(&path)?;
        Ok(Self {
            file,
            path,
            bytes_written: 0,
            truncated: false,
            limits,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn size(&self) -> u64 {
        self.bytes_written
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Append bytes. Returns `false` once the maximum has been reached and
    /// recording has stopped.
    pub fn append(&mut self, data: &[u8]) -> bool {
        if self.truncated || data.is_empty() {
            return !self.truncated;
        }
        if self.bytes_written + data.len() as u64 > self.limits.maximum_log_bytes {
            self.truncated = true;
            self.flush().ok();
            return false;
        }
        if self.file.write_all(data).is_err() {
            self.truncated = true;
            return false;
        }
        self.bytes_written += data.len() as u64;
        true
    }

    /// Read up to `max_bytes` starting at byte offset `offset`.
    pub fn read_at(&mut self, offset: u64, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(offset))?;
        let cap = usize::try_from(max_bytes.min(1024 * 1024)).unwrap_or(1024 * 1024);
        let mut buf = vec![0u8; cap];
        let n = self.file.read(&mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Read for OutputLog {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

/// Sum the sizes of all `.terminal` files in a directory.
pub fn total_log_bytes(dir: &Path) -> u64 {
    if validate_log_dir(dir).is_err() {
        return 0;
    }
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(".terminal") {
                if let Ok(meta) = entry.metadata() {
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }
    total
}

/// Delete a job's log file. Returns the number of bytes freed.
pub fn delete_log(dir: &Path, job_id: JobId) -> std::io::Result<u64> {
    match validate_log_dir(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }
    let path = OutputLog::path_for(dir, job_id);
    let size = std::fs::symlink_metadata(&path)
        .map(|m| m.len())
        .unwrap_or(0);
    if let Err(error) = std::fs::remove_file(&path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    let index = dir.join(format!("{job_id}.index"));
    if let Err(error) = std::fs::remove_file(index) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    Ok(size)
}

fn validate_log_dir(dir: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(dir)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "log directory must be a real directory, not a symlink",
        ));
    }
    Ok(())
}
