#!/usr/bin/env python3
"""Exercise real detached services, agent cgroup teardown, and CLI queries.

Run after `cargo build --workspace --offline`, with a working systemd user
manager. All test files and daemon state stay under target/; only the services
created by this test are stopped. No production daemon or jobs are touched.
"""

import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / "target/debug/jobwrap"
DAEMON = ROOT / "target/debug/jobwrapd"


def wait_for(check, message, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError(message)


def main():
    subprocess.run(["systemctl", "--user", "show-environment"], check=True, stdout=subprocess.DEVNULL)
    work = Path(tempfile.mkdtemp(prefix="detached-test-", dir=str(ROOT / "target")))
    print("Artifacts: " + str(work), flush=True)
    for folder in ["config/jobwrap", "state", "runtime", "data"]:
        (work / folder).mkdir(parents=True, exist_ok=True)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        daemon_port = listener.getsockname()[1]
    (work / "config/jobwrap/config.toml").write_text(
        'config_version = 1\n[server]\nbind = "loopback"\nport = {}\n[daemon]\nauto_start = false\n'.format(daemon_port))
    # Keep the fixture environment intentional, without persisting ambient
    # credentials into test files or transient unit definitions.
    env = {"HOME": os.environ["HOME"], "PATH": os.environ["PATH"],
           "XDG_CONFIG_HOME": str(work / "config"), "XDG_STATE_HOME": str(work / "state"),
           "XDG_RUNTIME_DIR": str(work / "runtime"), "XDG_DATA_HOME": str(work / "data"),
           "DBUS_SESSION_BUS_ADDRESS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", "unix:path=" + os.environ.get("XDG_RUNTIME_DIR", "/run/user/" + str(os.getuid())) + "/bus"),
           "JOBWRAP_BIN": str(CLI)}
    daemon_log = (work / "daemon.log").open("wb")
    daemon = subprocess.Popen([str(DAEMON), "--foreground"], env=env, cwd=str(work), stdout=daemon_log, stderr=subprocess.STDOUT)
    job_ids = []
    launcher_unit = "jobwrap-test-launcher-{}.service".format(work.name)

    def cli(*args, **kwargs):
        return subprocess.run([str(CLI)] + list(args), env=env, cwd=str(work), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, **kwargs)

    def launch(*command, **options):
        args = ["run", "-d", "--profile", "private", "--name", options.get("name", "detached-test")]
        if options.get("no_record"):
            args.append("--no-record")
        result = cli(*(args + ["--"] + list(command)))
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        job_id = re.search(rb"registered detached job (\w+)", result.stdout).group(1).decode()
        job_ids.append(job_id)
        return job_id

    try:
        wait_for(lambda: (work / "runtime/jobwrap/daemon.sock").exists(), "test daemon startup")
        (work / "serve.py").write_text("#!" + sys.executable + "\n" + '''import http.server, json, os, sys
from pathlib import Path
server = http.server.HTTPServer(("127.0.0.1", 0), http.server.SimpleHTTPRequestHandler)
Path("server.json").write_text(json.dumps({"port": server.server_port, "pid": os.getpid(),
    "argv": os.fsencode(sys.argv[1]).hex(), "env": os.environb[b"JOBWRAP_TEST_VALUE"].hex(), "cwd": os.getcwd()}))
print("detached-output-ready", flush=True)
server.serve_forever()
''')
        (work / "serve.py").chmod(0o700)
        (work / "index.html").write_text("server-survived-agent-cleanup")
        raw_value = b"literal spaces $() ; \xff"
        launcher_env = dict(env)
        launcher_env["JOBWRAP_TEST_VALUE"] = os.fsdecode(raw_value)
        fixture = {"env": {os.fsencode(k).hex(): os.fsencode(v).hex() for k, v in launcher_env.items()},
                   "argv": [os.fsencode(a).hex() for a in [str(CLI), "run", "--detach", "--profile", "private", "--name", "detached-server", "--", "./serve.py", os.fsdecode(raw_value)]]}
        (work / "launcher.json").write_text(json.dumps(fixture))
        (work / "launcher.py").write_text('''import json, os, pathlib, subprocess, time
root = pathlib.Path(__file__).parent
fixture = json.loads((root / "launcher.json").read_text())
env = {bytes.fromhex(k): bytes.fromhex(v) for k, v in fixture["env"].items()}
result = subprocess.run([bytes.fromhex(a) for a in fixture["argv"]], env=env, cwd=str(root), stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
(root / "launch-output").write_bytes(result.stdout)
(root / "launch-code").write_text(str(result.returncode))
while True: time.sleep(1)
''')
        subprocess.run(["systemd-run", "--user", "--quiet", "--collect", "--service-type=exec", "--unit=" + launcher_unit,
                        sys.executable, str(work / "launcher.py")], check=True)
        wait_for(lambda: (work / "launch-code").exists(), "agent launch did not finish")
        output = (work / "launch-output").read_bytes()
        assert (work / "launch-code").read_text() == "0", output
        job_id = re.search(rb"registered detached job (\w+)", output).group(1).decode()
        job_ids.append(job_id)
        wait_for(lambda: (work / "server.json").exists(), "server startup")
        server = json.loads((work / "server.json").read_text())
        assert server["argv"] == raw_value.hex(), server
        assert server["env"] == raw_value.hex(), server
        assert server["cwd"] == str(work), server
        subprocess.run(["systemctl", "--user", "kill", "--signal=SIGKILL", launcher_unit], check=True)
        time.sleep(0.15)
        with urllib.request.urlopen("http://127.0.0.1:{}/".format(server["port"]), timeout=3) as response:
            assert response.read() == b"server-survived-agent-cleanup"
        assert b"running" in cli("show", job_id).stdout
        before = cli("list").stdout
        status = cli("status", job_id)
        assert status.returncode == 0 and status.stdout == cli("show", job_id).stdout, status.stderr
        assert cli("list").stdout == before, "status must not create another job"
        assert b"detached-output-ready" in cli("logs", job_id).stdout
        assert cli("stop", job_id).returncode == 0
        wait_for(lambda: b"terminated by SIGTERM" in cli("show", job_id).stdout, "detached server stop was not recorded")

        short = launch(sys.executable, "-c", 'print("exit-seven", flush=True); raise SystemExit(7)')
        wait_for(lambda: b"exited (7)" in cli("show", short).stdout or b"exited with code 7" in cli("show", short).stdout, "exit code 7 was not recorded")
        assert b"exit-seven" in cli("logs", short).stdout
        quiet = launch(sys.executable, "-c", 'print("not-recorded")', no_record=True)
        wait_for(lambda: b"exited" in cli("show", quiet).stdout, "no-record job completion")
        assert cli("logs", quiet).stdout == b""
        before = cli("list").stdout
        assert cli("run", "--detach", "--no-web", "--", "true").returncode != 0
        assert cli("run", "--detach", "--profile", "missing", "--", "true").returncode != 0
        for args in [("python3", "-c", "raise SystemExit(0)"), ("statsu", "anything"), ("wrap", "true"), ("--", "true")]:
            assert cli(*args).returncode == 2, "implicit command launch must be rejected"
        assert cli("list").stdout == before, "invalid requests started a workload"

        survivor = launch(sys.executable, "-c", 'import os, pathlib, time; pathlib.Path("survivor.pid").write_text(str(os.getpid())); time.sleep(120)')
        wait_for(lambda: (work / "survivor.pid").exists(), "survivor startup")
        survivor_pid = int((work / "survivor.pid").read_text())
        daemon.terminate()
        daemon.wait(timeout=5)
        os.kill(survivor_pid, 0)
        # A monitoring daemon restart must not terminate a detached service.
        # Reconnection/adoption is outside the current monitoring protocol.
        broken_bus_env = dict(env, DBUS_SESSION_BUS_ADDRESS="unix:path=" + str(work / "missing-bus"))
        daemon = subprocess.Popen([str(DAEMON), "--foreground"], env=broken_bus_env, cwd=str(work), stdout=daemon_log, stderr=subprocess.STDOUT)
        wait_for(lambda: (work / "runtime/jobwrap/daemon.sock").exists(), "test daemon restart")
        assert b"disconnected" in cli("show", survivor).stdout
        before = cli("list").stdout
        unavailable = cli("run", "-d", "--", sys.executable, "-c", 'raise SystemExit(0)')
        assert unavailable.returncode != 0 and b"working systemd user manager" in unavailable.stderr, unavailable.stderr
        assert cli("list").stdout == before, "unavailable manager started a workload"
        print("PASS: detached HTTP server survived agent cgroup SIGKILL; argv/environment/cwd, logs, signals, exit status, no-record, status alias, and invalid requests verified")
        print("PASS: detached workload survived daemon shutdown; missing manager failed without launching a fallback")
        print("Artifacts: " + str(work))
    finally:
        for job_id in job_ids:
            # Stop only this test's own service, including any failed fixture.
            subprocess.run(["systemctl", "--user", "stop", "jobwrap-job-" + job_id + ".service"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["systemctl", "--user", "stop", launcher_unit], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        daemon.terminate()
        try:
            daemon.wait(timeout=5)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait()
        daemon_log.close()


if __name__ == "__main__":
    main()
