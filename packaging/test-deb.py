#!/usr/bin/env python3
"""Inspect real release packages without installing or extracting them.

Usage: python3 packaging/test-deb.py MAIN.deb SKILLS.deb
"""

import gzip
import hashlib
import io
from pathlib import Path
import subprocess
import sys
import tarfile


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
