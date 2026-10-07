#!/usr/bin/env python3
"""Build, verify, and publish downloadable Debian packages with GitHub CLI.

Run with --dry-run to build and preview locally without contacting GitHub.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shlex
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
local = runpy.run_path(str(ROOT / "packaging/build-deb.py"))["local"]


def run(args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def execute(args, env=None):
    print("+ " + shlex.join(str(arg) for arg in args), flush=True)
    subprocess.check_call(args, cwd=ROOT, env=env)


def repository(remote):
    url = run(["git", "remote", "get-url", "--push", remote])
    match = re.fullmatch(r"(?:git@github\.com:|https://github\.com/|ssh://git@github\.com/)([^/]+/[^/]+?)(?:\.git)?", url)
    if not match:
        raise ValueError("release remote must point to a github.com repository: " + url)
    return match.group(1)


def check_remote(repo, remote, tag, commit):
    execute(["gh", "auth", "status", "--hostname", "github.com"])
    # An API failure must abort, rather than being mistaken for an absent release.
    tags = run(["gh", "api", "repos/{}/releases?per_page=100".format(repo),
                "--paginate", "--jq", ".[].tag_name"]).splitlines()
    if tag in tags:
        raise ValueError("release already exists: " + tag)
    refs = run(["git", "ls-remote", remote, "refs/tags/" + tag, "refs/tags/" + tag + "^{}"])
    if refs:
        refs = dict(line.split("\t", 1)[::-1] for line in refs.splitlines())
        target = refs.get("refs/tags/" + tag + "^{}", refs.get("refs/tags/" + tag))
        if target != commit:
            raise ValueError("remote tag points to a different commit: " + tag)


def release(args):
    if run(["git", "status", "--porcelain", "--untracked-files=normal"]):
        raise ValueError("commit or stash working-tree changes before releasing")
    commit = run(["git", "rev-parse", "HEAD"])
    metadata = json.loads(run(["cargo", "metadata", "--offline", "--locked", "--no-deps", "--format-version", "1"]))
    package = next(p for p in metadata["packages"] if p["name"] == "jobwrap-cli")
    version = package["version"]
    tag = args.tag or ("v" + version + ("-deb." + args.revision if args.revision != "1" else ""))
    execute(["git", "check-ref-format", "refs/tags/" + tag])
    repo = repository(args.remote)
    maintainer = args.maintainer or package["metadata"]["deb"]["maintainer"]
    if not re.fullmatch(r"[^\r\n<>]+ <[^\s<>@]+@[^\s<>@]+>", maintainer):
        raise ValueError("supply --maintainer 'Name <email@example.org>'")
    if not args.dry_run and ("example.invalid" in maintainer or "example.org" in maintainer):
        raise ValueError("supply a real release contact with --maintainer")
    local_tag = subprocess.run(["git", "rev-parse", "--verify", "refs/tags/" + tag + "^{}"],
                               cwd=ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    if local_tag.returncode == 0 and local_tag.stdout.strip() != commit:
        raise ValueError("local tag points to a different commit: " + tag)
    if not args.dry_run:
        check_remote(repo, args.remote, tag, commit)

    print("Release {} on {} from {}".format(tag, repo, commit), flush=True)
    env = dict(os.environ)
    env["SOURCE_DATE_EPOCH"] = run(["git", "show", "-s", "--format=%ct", commit])
    execute([sys.executable, str(ROOT / "packaging/build-deb.py"), "--revision", args.revision,
             "--maintainer", maintainer], env=env)
    arch = run(["dpkg", "--print-architecture"])
    deb_version = version + "-" + args.revision
    assets = [local(ROOT / "target/debian" / "jobwrap_{}_{}.deb".format(deb_version, arch)),
              local(ROOT / "target/debian" / "jobwrap-codex-skills_{}_all.deb".format(deb_version))]
    execute([sys.executable, str(ROOT / "packaging/test-deb.py"), *map(str, assets)])
    # Catch source changes during the build before tagging or publishing it.
    if run(["git", "rev-parse", "HEAD"]) != commit or run(["git", "status", "--porcelain", "--untracked-files=normal"]):
        raise ValueError("source changed during the build; release aborted")

    # Use a commit/revision directory, independent of tag spelling (tags can contain '/').
    output = local(ROOT / "target/releases" / (commit + "-" + args.revision))
    output.mkdir(parents=True, exist_ok=True)
    checksums = local(output / "SHA256SUMS")
    checksums.write_text("".join(hashlib.sha256(p.read_bytes()).hexdigest() + "  " + p.name + "\n" for p in assets))
    notes = local(output / "release-notes.md")
    body = ("jobwrap {} (Debian revision {})\n\nBuilt from commit `{}` for `{}` on Linux.\n\n"
            "Download the application `.deb` and optionally the `jobwrap-codex-skills` `.deb`. "
            "The optional skill package makes the skill available to all machine users.\n\n"
            "Install as an administrator:\n\n```sh\napt install ./{}\n"
            "# With optional Codex skills:\napt install ./{} ./{}\n```\n\n"
            "Verify downloaded packages with `sha256sum -c SHA256SUMS` "
            "(download both packages for this check).\n\n"
            "The application includes manpages and Bash/Zsh/Fish completions. "
            "It does not enable a daemon at installation.\n").format(
                version, args.revision, commit, arch, assets[0].name, assets[0].name, assets[1].name)
    if args.notes_file:
        body += "\n" + local(ROOT / args.notes_file).read_text()
    notes.write_text(body)
    assets.append(checksums)
    command = ["gh", "release", "create", tag, *map(str, assets), "--repo", repo,
               "--verify-tag", "--title", "jobwrap " + tag, "--notes-file", str(notes)]
    if args.draft:
        command.append("--draft")
    if args.prerelease or "-" in version:
        command.extend(["--prerelease", "--latest=false"])
    print(body, flush=True)
    if args.dry_run:
        print("Dry run: would create/push tag {} at {} and run:\n{}".format(
            tag, commit, shlex.join(command)), flush=True)
        return
    if local_tag.returncode != 0:
        execute(["git", "tag", "-a", tag, commit, "-m", "jobwrap " + tag])
    execute(["git", "push", args.remote, "refs/tags/" + tag + ":refs/tags/" + tag])
    execute(command)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="build and verify locally; no GitHub access or tags")
    parser.add_argument("--draft", action="store_true", help="upload a draft instead of publishing")
    parser.add_argument("--prerelease", action="store_true", help="mark as prerelease, without setting latest")
    parser.add_argument("--remote", default="origin", help="GitHub push remote (default: origin)")
    parser.add_argument("--tag", help="release tag (default: vVERSION, or vVERSION-deb.REVISION)")
    parser.add_argument("--revision", default="1", help="Debian revision (default: 1)")
    parser.add_argument("--maintainer", help="real release contact: Name <email@example.org>")
    parser.add_argument("--notes-file", help="additional release notes, inside this repository")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9.+~]+", args.revision):
        parser.error("invalid Debian revision")
    if args.remote.startswith("-") or (args.tag and args.tag.startswith("-")):
        parser.error("remote and tag must not start with '-'")
    try:
        release(args)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, "Release failed: {}\n".format(error))


if __name__ == "__main__":
    main()
