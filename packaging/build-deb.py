#!/usr/bin/env python3
"""Build release .deb files with Cargo and Debian tools, without installing them.

Python 3.8+; staging directories are retained under target/debian for inspection.
The main package's assets have one source of truth: Cargo's deb metadata.
"""

import argparse
from email.utils import formatdate
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]


def local(path):
    """Reject escaping paths and symlinks before any generated-file write."""
    path = Path(path).absolute()
    path.resolve().relative_to(ROOT)
    for part in (path, *path.parents):
        if part == ROOT:
            break
        if part.is_symlink():
            raise ValueError("refusing symlink: {}".format(part))
    return path


def write(path, data, mode=0o644):
    path = local(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(mode)


def run(args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def build_package(name, version, architecture, assets, metadata, output, depends=""):
    work = Path(tempfile.mkdtemp(prefix=name + "-", dir=local(output)))
    stage = local(work / "debian" / name)
    stage.mkdir(parents=True)
    print("Staging {} in {}:".format(name, stage), flush=True)
    for source, dest, mode in assets:
        print("  {} -> {}".format(source.relative_to(ROOT), dest), flush=True)
    conffiles = []
    for source, dest, mode in assets:
        dest = Path(dest)
        if dest.is_absolute() or ".." in dest.parts:
            raise ValueError("invalid asset destination: {}".format(dest))
        data = local(source).read_bytes()
        if str(dest).startswith("usr/share/man/") or dest.name in ("README.md", "changelog"):
            dest = dest.with_name(dest.name + ".gz")
            data = gzip.compress(data, mtime=0)
        write(stage / dest, data, int(mode, 8))
        if dest.parts[0] == "etc":
            conffiles.append("/" + dest.as_posix())

    doc = stage / "usr/share/doc" / name
    write(doc / "copyright", ("Copyright: " + metadata["copyright"] + "\n\n").encode()
          + (ROOT / "LICENSE").read_bytes())
    if name == "jobwrap-codex-skills":
        write(doc / "changelog.gz", gzip.compress((ROOT / "CHANGELOG.md").read_bytes(), mtime=0))
    else:
        # dpkg-shlibdeps needs a source control file even when writing deps to stdout.
        write(work / "debian/control", b"Source: jobwrap\n\nPackage: jobwrap\nArchitecture: any\n")
        deps = run(["dpkg-shlibdeps", "-O", "-edebian/jobwrap/usr/bin/jobwrap",
                    "-edebian/jobwrap/usr/libexec/jobwrap/jobwrapd"], cwd=work)
        depends = next(line.split("=", 1)[1] for line in deps.splitlines()
                       if line.startswith("shlibs:Depends="))

    timestamp = int(os.environ.get("SOURCE_DATE_EPOCH", time.time()))
    changelog = ("{} ({}) unstable; urgency=medium\n\n"
                 "  * Build the jobwrap release package.\n\n"
                 " -- {}  {}\n").format(name, version, metadata["maintainer"],
                                         formatdate(timestamp, localtime=False))
    write(doc / "changelog.Debian.gz", gzip.compress(changelog.encode(), mtime=0))

    fields = ["Package: " + name, "Version: " + version, "Architecture: " + architecture,
              "Maintainer: " + metadata["maintainer"], "Section: utils", "Priority: optional"]
    if depends:
        fields.append("Depends: " + depends)
    if name == "jobwrap":
        fields.append("Suggests: " + metadata["suggests"])
        description = "Wrap Linux processes with monitoring and scoped control"
        extended = " Run commands in a pseudo-terminal while a per-user daemon exposes\n status, output, and narrowly authorized process control."
    else:
        description = "Optional system-wide jobwrap skill for Codex"
        extended = " Install jobwrap workflow instructions and references in Codex's\n administrator skill directory, making the skill available to all users."
    fields.append("Installed-Size: " + str(sum(p.stat().st_size for p in stage.rglob("*")
                                             if p.is_file()) // 1024 + 1))
    fields.append("Description: " + description + "\n" + extended)
    write(stage / "DEBIAN/control", ("\n".join(fields) + "\n").encode())
    if conffiles:
        write(stage / "DEBIAN/conffiles", ("\n".join(sorted(conffiles)) + "\n").encode())
    checksums = []
    for path in sorted(stage.rglob("*")):
        rel = path.relative_to(stage)
        if path.is_file() and rel.parts[0] != "DEBIAN":
            checksums.append(hashlib.md5(path.read_bytes()).hexdigest() + "  " + rel.as_posix())
    write(stage / "DEBIAN/md5sums", ("\n".join(checksums) + "\n").encode())
    archive = local(output / "{}_{}_{}.deb".format(name, version, architecture))
    print(run(["dpkg-deb", "--root-owner-group", "--build", str(stage), str(archive)]))
    return archive


def main():
    os.umask(0o022)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex-skills", choices=("yes", "no"), default="yes",
                        help="build the separate optional skills package (default: yes)")
    parser.add_argument("--skills-only", action="store_true", help="build only the skills package")
    parser.add_argument("--no-build", action="store_true", help="package existing release binaries")
    parser.add_argument("--revision", default="1", help="Debian package revision (default: 1)")
    parser.add_argument("--maintainer", help="release maintainer, e.g. Name <email@example.org>")
    args = parser.parse_args()
    if not args.revision or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.+~" for c in args.revision):
        parser.error("revision must contain only letters, digits, '.', '+', or '~'")
    if args.skills_only and args.codex_skills == "no":
        parser.error("--skills-only conflicts with --codex-skills no")

    metadata = json.loads(run(["cargo", "metadata", "--offline", "--locked", "--no-deps", "--format-version", "1"]))
    package = next(p for p in metadata["packages"] if p["name"] == "jobwrap-cli")
    deb = dict(package["metadata"]["deb"])
    if args.maintainer:
        if "\n" in args.maintainer or "\r" in args.maintainer:
            parser.error("maintainer must be a single line")
        deb["maintainer"] = args.maintainer
    version = package["version"] + "-" + args.revision
    output = local(ROOT / "target/debian")
    output.mkdir(parents=True, exist_ok=True)
    if not args.skills_only:
        if not args.no_build:
            subprocess.check_call(["cargo", "build", "--release", "--workspace", "--locked", "--offline"], cwd=ROOT)
        assets = []
        for source, dest, mode in deb["assets"]:
            if source.startswith("target/release/"):
                source = local(Path(metadata["target_directory"]) / "release" / Path(source).name)
            else:
                source = local(Path(package["manifest_path"]).parent / source).resolve()
            if dest.endswith("/"):
                dest += source.name
            assets.append((source, dest, mode))
        build_package("jobwrap", version, run(["dpkg", "--print-architecture"]), assets, deb, output)
    if args.codex_skills == "yes":
        assets = [(p, "etc/codex/skills/jobwrap/" + p.relative_to(ROOT / "skills/jobwrap").as_posix(), "644")
                  for p in sorted((ROOT / "skills/jobwrap").rglob("*")) if p.is_file()]
        build_package("jobwrap-codex-skills", version, "all", assets, deb, output,
                      depends="jobwrap (= {})".format(version))


if __name__ == "__main__":
    main()
