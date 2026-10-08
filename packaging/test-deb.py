#!/usr/bin/env python3
"""Inspect real release packages without installing them.

Copy packaged binaries under target/debian to verify their build identity.

Usage: python3 packaging/test-deb.py MAIN.deb SKILLS.deb
"""

import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request


def contents(archive, option):
    data = subprocess.check_output(["dpkg-deb", option, str(archive)])
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        files = {}
        for member in tar.getmembers():
            assert member.uid == member.gid == 0, member.name
            if member.isdir():
                assert member.mode == 0o755, member.name
            if member.isfile():
                assert member.mode == 0o644 or member.mode == 0o755, member.name
                files[member.name.lstrip("./")] = (member, tar.extractfile(member).read())
        return files


def inspect(archive):
    files = contents(archive, "--fsys-tarfile")
    control = contents(archive, "--ctrl-tarfile")
    assert not set(control).intersection({"preinst", "postinst", "prerm", "postrm", "config", "templates"})
    assert not any(p.startswith(("home/", "root/", "var/", "debian/")) for p in files)
    fields = dict(line.split(": ", 1) for line in control["control"][1].decode().splitlines()
                  if not line.startswith(" "))
    checksums = dict(line.split("  ", 1)[::-1] for line in control["md5sums"][1].decode().splitlines())
    assert set(checksums) == set(files)
    for path, (_, data) in files.items():
        assert hashlib.md5(data).hexdigest() == checksums[path], path
    return files, control, fields


def verify_server_build(work, version, commit):
    """Check the packaged daemon in an isolated, project-local environment."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    config = work / "config/jobwrap/config.toml"
    log_path = work / "daemon.log"
    print("Server check artifacts: {}, {}".format(config, log_path), flush=True)
    config.parent.mkdir(parents=True)
    config.write_text('config_version = 1\n[server]\nbind = "loopback"\nport = {}\n'.format(port))
    env = dict(os.environ, XDG_CONFIG_HOME=str(work / "config"),
               XDG_STATE_HOME=str(work / "state"), XDG_RUNTIME_DIR=str(work / "runtime"),
               XDG_DATA_HOME=str(work / "data"))
    base = "http://127.0.0.1:{}".format(port)
    with log_path.open("w") as log:
        daemon = subprocess.Popen([str(work / "jobwrapd"), "--foreground"], env=env,
                                  stdout=log, stderr=log)
        try:
            for attempt in range(100):
                assert daemon.poll() is None, "daemon exited; inspect " + str(log_path)
                try:
                    with urllib.request.urlopen(base + "/api/v1/server", timeout=1) as response:
                        info = json.load(response)
                    break
                except urllib.error.URLError:
                    time.sleep(0.05)
            else:
                raise AssertionError("daemon did not become ready")
            assert info["version"] == version, info
            assert info["git_commit"] == commit, info
            assert info["build_dirty"] is False, info
            for page in ("/", "/login", "/jobs/new", "/jobs/invalid"):
                with urllib.request.urlopen(base + page, timeout=5) as response:
                    html = response.read().decode()
                assert 'class="build-footer"' in html, page
                assert "jobwrap " + version in html, page
                assert 'title="{}"'.format(commit) in html, page
        finally:
            daemon.terminate()
            daemon.wait(timeout=10)
    print("Packaged CLI, API and web footer build identities match.", flush=True)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main_files, _, main_fields = inspect(Path(sys.argv[1]))
    skill_files, skill_control, skill_fields = inspect(Path(sys.argv[2]))
    assert main_fields["Package"] == "jobwrap"
    assert "libc6" in main_fields["Depends"]
    assert "jobwrap-codex-skills" in main_fields["Suggests"]
    assert not any(p.startswith("etc/") for p in main_files)
    for binary in ("usr/bin/jobwrap", "usr/libexec/jobwrap/jobwrapd"):
        member, data = main_files[binary]
        assert member.mode == 0o755 and data.startswith(b"\x7fELF"), binary
    for section, name in ((1, "jobwrap"), (5, "jobwrap.toml"), (8, "jobwrapd")):
        path = "usr/share/man/man{}/{}.{}.gz".format(section, name, section)
        assert gzip.decompress(main_files[path][1]).startswith(b".TH"), path
    for path in ("usr/share/bash-completion/completions/jobwrap",
                 "usr/share/zsh/vendor-completions/_jobwrap",
                 "usr/share/fish/vendor_completions.d/jobwrap.fish"):
        assert main_files[path][1], path
    assert b"Type=simple" in main_files["usr/lib/systemd/user/jobwrapd.service"][1]
    assert "usr/lib/systemd/user/jobwrapd.socket" not in main_files
    assert skill_fields["Package"] == "jobwrap-codex-skills"
    assert skill_fields["Architecture"] == "all"
    assert skill_fields["Depends"] == "jobwrap (= {})".format(main_fields["Version"])
    expected = {"etc/codex/skills/jobwrap/SKILL.md", "etc/codex/skills/jobwrap/references/cli.md",
                "etc/codex/skills/jobwrap/references/api.md"}
    assert {p for p in skill_files if p.startswith("etc/")} == expected
    assert set(skill_control["conffiles"][1].decode().splitlines()) == {"/" + p for p in expected}
    root = Path(__file__).resolve().parents[1]
    # Execute the binaries from the archive, not a potentially stale local
    # build. All verification artifacts stay inside the repository.
    directory = root / "target/debian"
    directory.resolve().relative_to(root)
    assert not directory.is_symlink()
    directory.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="version-check-", dir=directory))
    version = main_fields["Version"].rsplit("-", 1)[0]
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    for binary in ("usr/bin/jobwrap", "usr/libexec/jobwrap/jobwrapd"):
        path = work / Path(binary).name
        path.resolve().relative_to(root)
        assert not path.is_symlink()
        print("Verifying packaged binary: " + str(path), flush=True)
        path.write_bytes(main_files[binary][1])
        path.chmod(0o755)
        reported = subprocess.check_output([str(path), "--version"], text=True).splitlines()
        assert reported[0] == path.name + " " + version, reported
        assert "commit: " + commit in reported, reported
        assert "build dirty: false" in reported, reported
    verify_server_build(work, version, commit)
    for path in expected:
        assert skill_files[path][1] == (root / "skills/jobwrap" / path.split("jobwrap/", 1)[1]).read_bytes()
    for fields, files in ((main_fields, main_files), (skill_fields, skill_files)):
        prefix = "usr/share/doc/" + fields["Package"] + "/"
        assert "MIT" in files[prefix + "copyright"][1].decode()
        assert gzip.decompress(files[prefix + "changelog.gz"][1])
        assert gzip.decompress(files[prefix + "changelog.Debian.gz"][1]).startswith(
            (fields["Package"] + " (" + fields["Version"] + ")").encode())

    # Completing a signal must use its argument position, without executing jobwrap.
    script = '''
set -e
source packaging/completions/jobwrap.bash
jobwrap() { echo 'completion executed jobwrap' >&2; exit 99; }
COMP_WORDS=(jobwrap signal example-job t); COMP_CWORD=3
_jobwrap
[[ "${COMPREPLY[*]}" == term ]]
COMP_WORDS=(jobwrap signal ''); COMP_CWORD=2
_jobwrap
[[ ${#COMPREPLY[@]} == 0 ]]
COMP_WORDS=(jobwrap daemon re); COMP_CWORD=2
_jobwrap
[[ "${COMPREPLY[*]}" == restart ]]
COMP_WORDS=(jobwrap daemon start --f); COMP_CWORD=3
_jobwrap
[[ "${COMPREPLY[*]}" == --foreground ]]
'''
    subprocess.check_call(["bash", "-c", script], cwd=root)
    print("Package contents, dependencies, ownership, conffiles and Bash completion checks passed.")


if __name__ == "__main__":
    main()
