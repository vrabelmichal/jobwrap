//! Best-effort launch context detection; never runs Python or an environment manager.
use jobwrap_core::execution::PythonEnvironment;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) fn detect(
    executable: &OsString,
    arguments: &[OsString],
    env: &HashMap<OsString, OsString>,
    cwd: &Path,
) -> Option<PythonEnvironment> {
    let command = Path::new(executable);
    let resolved = if command.components().count() > 1 {
        Some(absolute(command, cwd))
    } else {
        env.get(&OsString::from("PATH")).and_then(|path| {
            std::env::split_paths(path)
                .map(|p| absolute(&p.join(command), cwd))
                .find(|p| p.is_file())
        })
    };
    let exe_name = command.file_name()?.to_string_lossy();
    let direct_python = exe_name.starts_with("python");
    // An explicitly selected interpreter is more authoritative than activation.
    if direct_python {
        if let Some(path) = resolved
            .as_ref()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            if path.join("pyvenv.cfg").is_file() || path.join("conda-meta").is_dir() {
                return Some(describe(
                    path.to_path_buf(),
                    resolved,
                    "interpreter path",
                    None,
                ));
            }
        }
    }
    if exe_name == "uv" && arguments.first().map(|s| s == "run").unwrap_or(false) {
        let selected = env
            .get(&OsString::from("UV_PROJECT_ENVIRONMENT"))
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.join(".venv"));
        let selected = absolute(&selected, cwd);
        if selected.join("pyvenv.cfg").is_file() {
            return Some(describe(
                selected,
                None,
                "uv project environment (inferred at launch)",
                Some("uv"),
            ));
        }
    }
    for (key, kind) in [("VIRTUAL_ENV", "venv"), ("CONDA_PREFIX", "conda")] {
        if let Some(path) = env.get(&OsString::from(key)).filter(|s| !s.is_empty()) {
            return Some(describe(
                absolute(Path::new(path), cwd),
                if direct_python { resolved } else { None },
                "inherited activation",
                Some(kind),
            ));
        }
    }
    None
}
fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}
fn describe(
    path: PathBuf,
    interpreter: Option<PathBuf>,
    evidence: &str,
    hint: Option<&str>,
) -> PythonEnvironment {
    let mut cfg = String::new();
    if let Ok(file) = std::fs::File::open(path.join("pyvenv.cfg")) {
        let _ = file.take(16384).read_to_string(&mut cfg);
    }
    let kind = if hint == Some("uv") || cfg.lines().any(|l| l.trim_start().starts_with("uv =")) {
        "uv"
    } else if hint == Some("conda") || path.join("conda-meta").is_dir() {
        "conda"
    } else {
        "venv"
    };
    PythonEnvironment {
        kind: kind.into(),
        name: path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
        path: path.to_string_lossy().into_owned(),
        interpreter: interpreter.map(|p| p.to_string_lossy().into_owned()),
        evidence: evidence.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_is_recorded_without_running_environment_tools() {
        let mut env = HashMap::new();
        env.insert("CONDA_PREFIX".into(), "/envs/science".into());
        env.insert("SECRET_TOKEN".into(), "never persisted".into());
        let info = detect(&"bash".into(), &[], &env, Path::new("/work")).unwrap();
        assert_eq!(info.kind, "conda");
        assert_eq!(info.name, "science");
        assert_eq!(info.evidence, "inherited activation");
        assert!(!serde_json::to_string(&info).unwrap().contains("SECRET"));
        env.insert("VIRTUAL_ENV".into(), "/work/.venv".into());
        assert_eq!(
            detect(&"python".into(), &[], &env, Path::new("/work"))
                .unwrap()
                .kind,
            "venv"
        );
    }
    #[test]
    fn explicit_interpreters_override_activation_and_uv_is_identified() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("target")
            .join(format!("environment-test-{}", std::process::id()));
        let uv = root.join(".venv");
        std::fs::create_dir_all(uv.join("bin")).unwrap();
        std::fs::write(uv.join("pyvenv.cfg"), "uv = 0.9.0\n").unwrap();
        let mut env = HashMap::new();
        env.insert("VIRTUAL_ENV".into(), "/different/env".into());
        let direct = detect(&uv.join("bin/python").into_os_string(), &[], &env, &root).unwrap();
        assert_eq!(direct.kind, "uv");
        assert_eq!(direct.path, uv.to_string_lossy());
        let inferred = detect(
            &"uv".into(),
            &["run".into(), "script.py".into()],
            &env,
            &root,
        )
        .unwrap();
        assert_eq!(inferred.kind, "uv");
        assert!(inferred.evidence.contains("inferred"));
        let conda = root.join("conda");
        std::fs::create_dir_all(conda.join("conda-meta")).unwrap();
        let direct = detect(&conda.join("bin/python").into_os_string(), &[], &env, &root).unwrap();
        assert_eq!(direct.kind, "conda");
        assert_eq!(direct.path, conda.to_string_lossy());
    }
}
