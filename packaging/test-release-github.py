#!/usr/bin/env python3
"""Exercise release orchestration and failure handling without GitHub writes."""

import argparse
import contextlib
import io
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch


ROOT = Path(__file__).resolve().parents[1]
G = runpy.run_path(str(ROOT / "packaging/release-github.py"))["release"].__globals__


class ReleaseTests(unittest.TestCase):
    def test_repository_urls(self):
        for url in ("git@github.com:owner/project.git", "https://github.com/owner/project.git",
                    "https://github.com/owner/project", "ssh://git@github.com/owner/project.git"):
            with patch.dict(G, run=Mock(return_value=url)):
                self.assertEqual(G["repository"]("origin"), "owner/project")
        with patch.dict(G, run=Mock(return_value="git@example.org:owner/project.git")):
            with self.assertRaises(ValueError):
                G["repository"]("origin")

    def test_existing_release_and_mismatched_remote_tags(self):
        for responses in (("v0.1.0",), ("", "different\trefs/tags/v0.1.0")):
            with patch.dict(G, run=Mock(side_effect=responses), execute=Mock()):
                with self.assertRaises(ValueError):
                    G["check_remote"]("owner/project", "origin", "v0.1.0", "commit")
        for refs in ("commit\trefs/tags/v0.1.0",
                     "tag-object\trefs/tags/v0.1.0\ncommit\trefs/tags/v0.1.0^{}"):
            with patch.dict(G, run=Mock(side_effect=("", refs)), execute=Mock()):
                G["check_remote"]("owner/project", "origin", "v0.1.0", "commit")

    def test_api_errors_abort(self):
        failure = subprocess.CalledProcessError(1, ["gh", "api"])
        with patch.dict(G, run=Mock(side_effect=failure), execute=Mock()):
            with self.assertRaises(subprocess.CalledProcessError):
                G["check_remote"]("owner/project", "origin", "v0.1.0", "commit")

    def scenario(self, dry_run=False, dirty=False, failed_build=False, changed_source=False):
        # Retain generated files inside target; never create a live release or tag.
        work = Path(tempfile.mkdtemp(prefix="release-test-", dir=ROOT / "target"))
        assets = work / "target/debian"
        assets.mkdir(parents=True)
        for name in ("jobwrap_0.1.0-1_amd64.deb", "jobwrap-codex-skills_0.1.0-1_all.deb"):
            (assets / name).write_bytes(b"test asset " + name.encode())
        calls = []
        status_calls = 0

        def run(command):
            nonlocal status_calls
            if command[:3] == ["git", "status", "--porcelain"]:
                status_calls += 1
                return " M README.md" if dirty or (changed_source and status_calls > 1) else ""
            if command == ["git", "rev-parse", "HEAD"]:
                return "a" * 40
            if command[:2] == ["cargo", "metadata"]:
                return json.dumps({"packages": [{"name": "jobwrap-cli", "version": "0.1.0",
                                   "metadata": {"deb": {"maintainer": "Test <test@project.test>"}}}]})
            if command[:3] == ["git", "remote", "get-url"]:
                return "git@github.com:owner/project.git"
            if command[:3] == ["git", "show", "-s"]:
                return "1700000000"
            if command == ["dpkg", "--print-architecture"]:
                return "amd64"
            raise AssertionError("unexpected read: " + repr(command))

        def execute(command, env=None):
            calls.append(command)
            if command[1].endswith("build-deb.py"):
                self.assertEqual(env["SOURCE_DATE_EPOCH"], "1700000000")
                if failed_build:
                    raise subprocess.CalledProcessError(1, command)

        args = argparse.Namespace(dry_run=dry_run, draft=False, prerelease=False, remote="origin",
                                  tag=None, revision="1", maintainer=None, notes_file=None)
        check_remote = Mock()
        tag_result = subprocess.CompletedProcess([], 1, stdout="")
        with patch.dict(G, ROOT=work, run=run, execute=execute, check_remote=check_remote), \
                patch.object(G["subprocess"], "run", return_value=tag_result), \
                contextlib.redirect_stdout(io.StringIO()):
            if dirty or failed_build or changed_source:
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    G["release"](args)
            else:
                G["release"](args)
        writes = [c for c in calls if c[:2] in (["git", "tag"], ["git", "push"], ["gh", "release"])]
        if dry_run or dirty or failed_build or changed_source:
            self.assertFalse(writes)
        else:
            self.assertEqual([c[:2] for c in writes], [["git", "tag"], ["git", "push"], ["gh", "release"]])
            command = writes[-1]
            self.assertIn("--verify-tag", command)
            self.assertEqual(command[command.index("--repo") + 1], "owner/project")
            self.assertEqual(writes[0][4], "a" * 40)
            self.assertTrue(any(p.endswith("SHA256SUMS") for p in command))
        if dry_run:
            check_remote.assert_not_called()

    def test_publish_order_and_exact_commit(self):
        self.scenario()

    def test_dry_run_never_mutates_github_or_tags(self):
        self.scenario(dry_run=True)

    def test_dirty_tree_build_failure_and_source_changes_abort(self):
        self.scenario(dirty=True)
        self.scenario(failed_build=True)
        self.scenario(changed_source=True)


if __name__ == "__main__":
    unittest.main()
