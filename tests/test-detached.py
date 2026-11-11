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
import sqlite3
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
    runtime = ROOT / "target" / ("r" + str(len(list((ROOT / "target").glob("r[0-9]*")))))
    runtime.mkdir()
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
           "XDG_RUNTIME_DIR": str(runtime), "XDG_DATA_HOME": str(work / "data"),
           "DBUS_SESSION_BUS_ADDRESS": os.environ.get("DBUS_SESSION_BUS_ADDRESS", "unix:path=" + os.environ.get("XDG_RUNTIME_DIR", "/run/user/" + str(os.getuid())) + "/bus"),
           "JOBWRAP_BIN": str(CLI)}
    daemon_log = (work / "daemon.log").open("wb")
    daemon = subprocess.Popen([str(DAEMON), "--foreground"], env=env, cwd=str(work), stdout=daemon_log, stderr=subprocess.STDOUT)
    job_ids = []
    launcher_unit = "jobwrap-test-launcher-{}.service".format(work.name)
    daemon_launcher_unit = "jobwrap-test-daemon-agent-{}.service".format(work.name)

    def cli(*args, **kwargs):
        return subprocess.run([str(CLI)] + list(args), env=env, cwd=str(work), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30, **kwargs)

    def launch(*command, **options):
        args = ["run", "-d", "--profile", options.get("profile", "private"), "--name", options.get("name", "detached-test")]
        if options.get("no_record"):
            args.append("--no-record")
        result = cli(*(args + ["--"] + list(command)))
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        job_id = re.search(rb"registered detached job (\w+)", result.stdout).group(1).decode()
        job_ids.append(job_id)
        return job_id

    try:
        wait_for(lambda: (runtime / "jobwrap/daemon.sock").exists(), "test daemon startup")
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

        assert cli("--json", "wait", job_id, "--timeout", "2").returncode == 143
        assert cli("wait", "invalid-job-id", "--timeout", "10").returncode == 1

        short = launch(sys.executable, "-c", 'print("exit-seven", flush=True); raise SystemExit(7)')
        wait_for(lambda: b"exited (7)" in cli("show", short).stdout or b"exited with code 7" in cli("show", short).stdout, "exit code 7 was not recorded")
        assert b"exit-seven" in cli("logs", short).stdout
        observation = json.loads(cli("--json", "show", short).stdout)["job"]["monitoring"]
        assert observation["last_contact_at"] and observation["last_output_at"], observation
        waited = cli("--json", "wait", short, "--timeout", "2")
        assert waited.returncode == 7 and json.loads(waited.stdout)["job"]["state"]["code"] == 7, waited.stderr
        assert isinstance(json.loads(cli("list", "--json").stdout), list)
        assert json.loads(cli("doctor", short, "--json").stdout)["control_available"] is False
        pipe = subprocess.Popen([str(CLI), "list"], env=env, cwd=str(work), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        pipe.stdout.close()
        assert pipe.wait(timeout=5) == 0 and b"panicked" not in pipe.stderr.read()
        structured = cli("--json", "run", "-d", "--", sys.executable, "-c", "pass")
        launched = json.loads(structured.stdout)
        assert structured.returncode == 0 and launched["registered"] and launched["ready"] is None
        job_ids.append(launched["job_id"])
        quiet = launch(sys.executable, "-c", 'print("not-recorded")', no_record=True)
        wait_for(lambda: b"exited" in cli("show", quiet).stdout, "no-record job completion")
        assert cli("logs", quiet).stdout == b""
        before = cli("list").stdout
        assert cli("run", "--detach", "--no-web", "--", "true").returncode != 0
        assert cli("run", "--detach", "--profile", "missing", "--", "true").returncode != 0
        for args in [("python3", "-c", "raise SystemExit(0)"), ("statsu", "anything"), ("wrap", "true"), ("--", "true")]:
            assert cli(*args).returncode == 2, "implicit command launch must be rejected"
        assert cli("list").stdout == before, "invalid requests started a workload"

        def details(job_id):
            url = "http://127.0.0.1:{}/api/v1/jobs/{}/details".format(daemon_port, job_id)
            with urllib.request.urlopen(url, timeout=10) as response:
                return json.loads(response.read())

        def start_daemon(run_env):
            started = subprocess.Popen([str(DAEMON), "--foreground"], env=run_env, cwd=str(work), stdout=daemon_log, stderr=subprocess.STDOUT)
            wait_for(lambda: cli("daemon", "status").returncode == 0 and (runtime / "jobwrap/daemon.sock").exists(), "test daemon restart")
            return started

        # Exercise recording and a child exit during complete daemon absence.
        replay = launch(sys.executable, "-c", 'import pathlib,time; print("before-outage",flush=True); pathlib.Path("replay.ready").touch(); time.sleep(2); print("during-outage",flush=True); raise SystemExit(19)', profile="standard")
        wait_for(lambda: (work / "replay.ready").exists(), "replay child startup")
        daemon.kill(); daemon.wait(timeout=5)
        time.sleep(7)
        # final result remains durably queued in the independent worker
        daemon = start_daemon(env)
        wait_for(lambda: b"exited (19)" in cli("show", replay).stdout or b"exited with code 19" in cli("show", replay).stdout, "outage final result was lost")
        replay_log = cli("logs", replay).stdout
        assert replay_log.count(b"before-outage") == 1 and replay_log.count(b"during-outage") == 1, replay_log
        print("PASS: output replay has no duplicate chunks and final exit during daemon outage is recovered")

        # An explicitly selected venv is detected without activation.
        venv = work / "science-venv"
        subprocess.run([sys.executable, "-m", "venv", "--without-pip", str(venv)], check=True)
        environment_job = launch(str(venv / "bin/python"), "-c", "print('environment-ready')", profile="standard")
        metadata = json.loads(cli("--json", "show", environment_job).stdout)["job"]["execution"]
        assert metadata["python_environment"]["path"] == str(venv), metadata
        assert metadata["python_environment"]["kind"] == "venv", metadata
        assert metadata["worker_build"]["commit"] and metadata["daemon_build"]["version"], metadata
        hidden = json.loads(urllib.request.urlopen("http://127.0.0.1:{}/api/v1/jobs?environment=science-venv".format(daemon_port), timeout=10).read())
        assert hidden == [], "status-only readers must not filter on private environment paths"

        # A public detached job: systemd reports on its service while connected.
        watched = launch(sys.executable, "-c", 'import os, pathlib, time; pathlib.Path("watched.pid").write_text(str(os.getpid())); time.sleep(120)', profile="standard", name="watched")
        wait_for(lambda: (work / "watched.pid").exists(), "watched startup")
        watched_pid = int((work / "watched.pid").read_text())
        assert cli("wait", watched, "--timeout", "0").returncode == 124
        os.kill(watched_pid, 0)
        unit = "jobwrap-job-{}.service".format(watched)
        shown = cli("show", watched).stdout
        assert b"launch:             detached (systemd user service " + unit.encode() in shown, shown
        assert b"service:            active (running)" in shown, shown
        assert re.search(rb"service main pid:   [0-9]+ \(jobwrap worker\)", shown), shown
        # Usage needs delegated cgroup v2 controllers; cgroup v1 hosts omit it.
        data = details(watched)
        assert data["service_unit"] == unit and data["service"]["active_state"] == "active", data
        assert data["process"]["pid"] == watched_pid, data
        listed = json.loads(urllib.request.urlopen("http://127.0.0.1:{}/api/v1/jobs?q={}".format(daemon_port, unit), timeout=10).read())
        assert [job["id"] for job in listed] == [watched], listed

        survivor = launch(sys.executable, "-c", 'import os, pathlib, time; pathlib.Path("survivor.pid").write_text(str(os.getpid())); time.sleep(120)')
        wait_for(lambda: (work / "survivor.pid").exists(), "survivor startup")
        survivor_pid = int((work / "survivor.pid").read_text())
        daemon.terminate()
        daemon.wait(timeout=5)
        os.kill(survivor_pid, 0)
        os.kill(watched_pid, 0)

        # Simulate a record written before service tracking existed.
        db = sqlite3.connect(str(work / "state/jobwrap/jobwrap.db"))
        record = json.loads(db.execute("SELECT record_json FROM jobs WHERE id = ?", (watched,)).fetchone()[0])
        assert record.pop("service_unit") == unit
        record.pop("worker_identity", None)
        db.execute("UPDATE jobs SET record_json = ? WHERE id = ?", (json.dumps(record), watched))
        db.commit()
        db.close()

        # After a restart the workers cannot reconnect, but systemd still
        # vouches for them: the service is found again by its unit name and
        # the process snapshot is trusted because the worker is the service's
        # main process.
        daemon = start_daemon(env)
        shown = cli("show", watched).stdout
        assert b"disconnected" in shown and b"service:            active (running)" in shown, shown
        assert b"systemctl --user stop " + unit.encode() in shown, shown
        data = details(watched)
        assert data["state"]["type"] == "disconnected" and data["service_unit"] == unit, data
        assert data["process"]["pid"] == watched_pid, data
        subprocess.run(["systemctl", "--user", "kill", "--signal=SIGKILL", unit], check=True)
        wait_for(lambda: b"state:              lost" in cli("show", watched).stdout, "ended detached service was not settled")
        events = json.loads(urllib.request.urlopen("http://127.0.0.1:{}/api/v1/jobs/{}/events".format(daemon_port, watched), timeout=10).read())
        assert any("is no longer running" in json.dumps(event["kind"]) for event in events), events
        assert b"finished:" in cli("show", watched).stdout
        daemon.terminate()
        daemon.wait(timeout=5)
        os.kill(survivor_pid, 0)
        # A monitoring daemon restart must not terminate a detached service.
        # Reconnection/adoption is outside the current monitoring protocol.
        broken_bus_env = dict(env, DBUS_SESSION_BUS_ADDRESS="unix:path=" + str(work / "missing-bus"))
        daemon = start_daemon(broken_bus_env)
        wait_for(lambda: b"state:              running" in cli("show", survivor).stdout, "worker did not reconnect without a service manager")
        shown = cli("show", survivor).stdout
        assert b"service:            unavailable" in shown, shown
        assert cli("stop", survivor).returncode == 0
        wait_for(lambda: b"terminated by SIGTERM" in cli("show", survivor).stdout, "restored signal control failed")
        before = cli("list").stdout
        unavailable = cli("run", "-d", "--", sys.executable, "-c", 'raise SystemExit(0)')
        assert unavailable.returncode != 0 and b"working systemd user manager" in unavailable.stderr, unavailable.stderr
        assert cli("list").stdout == before, "unavailable manager started a workload"
        # Automatic daemon startup must escape the initiating agent too.
        daemon.terminate(); daemon.wait(timeout=5)
        daemon_fixture = dict(fixture)
        daemon_fixture["argv"] = [os.fsencode(a).hex() for a in [str(CLI), "daemon", "start"]]
        (work / "daemon-agent.json").write_text(json.dumps(daemon_fixture))
        daemon_agent = (work / "launcher.py").read_text().replace("launcher.json", "daemon-agent.json").replace("launch-output", "daemon-agent-output").replace("launch-code", "daemon-agent-code")
        (work / "daemon-agent.py").write_text(daemon_agent)
        subprocess.run(["systemd-run", "--user", "--quiet", "--collect", "--service-type=exec", "--unit=" + daemon_launcher_unit, sys.executable, str(work / "daemon-agent.py")], check=True)
        wait_for(lambda: (work / "daemon-agent-code").exists(), "independent daemon startup")
        assert (work / "daemon-agent-code").read_text() == "0", (work / "daemon-agent-output").read_bytes()
        subprocess.run(["systemctl", "--user", "kill", "--signal=SIGKILL", daemon_launcher_unit], check=True)
        status = cli("daemon", "status").stdout
        daemon_pid = int(re.search(rb"pid (\d+)", status).group(1))
        assert "jobwrapd-" in Path("/proc/{}/cgroup".format(daemon_pid)).read_text()
        assert cli("--json", "list").returncode == 0
        print("PASS: automatically started daemon survives initiating agent cgroup SIGKILL")
        print("PASS: detached HTTP server survived agent cgroup SIGKILL; argv/environment/cwd, logs, signals, exit status, no-record, status alias, and invalid requests verified")
        print("PASS: detached workload survived daemon shutdown; missing manager failed without launching a fallback")
        print("PASS: service state, usage, and process snapshot reported across a daemon restart; older records adopted; an ended service settled as lost")
        print("Artifacts: " + str(work))
    finally:
        cli("daemon", "stop")
        subprocess.run(["systemctl", "--user", "stop", daemon_launcher_unit], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
