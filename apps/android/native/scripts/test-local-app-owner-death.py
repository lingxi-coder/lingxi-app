#!/usr/bin/env python3
"""Run real host SIGKILL/cold-recovery acceptance on an explicitly selected device.

Install matching direct-debug main/test APKs and stage fixtures/local-apps/v2 in
files/local-app-acceptance-fixtures first. Only the PID recorded by the fixture
is killed. `am force-stop` would also kill children and invalidate this test.
"""
import argparse
import json
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--recover-only", action="store_true")
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    adb = [args.adb, "-s", args.serial]
    package = "com.lingxi.code.direct.debug"
    component = package + ".test/androidx.test.runner.AndroidJUnitRunner"
    test_class = "com.lingxi.code.apps.UnifiedLocalAppProcessDeathTest"
    receipt = "files/local-app-owner-death-acceptance.json"

    def command(*values, check=True):
        return subprocess.run(adb + list(values), text=True, capture_output=True,
                              check=check, timeout=20)

    def process_is_running(pid, start_ticks):
        result = command("shell", "run-as", package, "cat", f"/proc/{pid}/stat", check=False)
        if result.returncode != 0:
            if "No such file" in result.stderr + result.stdout:
                return False
            raise RuntimeError("Cannot verify the candidate process identity: " + result.stderr + result.stdout)
        fields = result.stdout.rsplit(")", 1)[1].split()
        return fields[0] != "Z" and fields[19] == start_ticks

    existing = command("shell", "run-as", package, "cat", receipt, check=False)
    if args.recover_only:
        if existing.returncode != 0:
            raise RuntimeError("There is no acceptance receipt to recover")
        saved = json.loads(existing.stdout)
    else:
        if existing.returncode == 0:
            raise RuntimeError("Recover the existing receipt with --recover-only first")
        log = args.output_dir / "owner-death-prepare.log"
        with log.open("w") as output:
            prepare = subprocess.Popen(adb + ["shell", "am", "instrument", "-w", "-r",
                "-e", "class", test_class, "-e", "ownerDeathPhase", "prepare", component],
                stdout=output, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 100
            while time.monotonic() < deadline:
                if prepare.poll() is not None:
                    raise RuntimeError("Prepare ended before the update checkpoint: " + log.read_text()[-4000:])
                result = command("shell", "run-as", package, "cat", receipt, check=False)
                if result.returncode == 0:
                    saved = json.loads(result.stdout)
                    break
                time.sleep(0.5)
            else:
                raise RuntimeError("Candidate build did not reach its checkpoint; fixture has a bounded cleanup timeout")
            pid = saved["hostPid"]
            if (not isinstance(pid, int) or pid <= 100 or "/.apps/owner-death-" not in saved["source"]
                    or saved["candidateRevision"] <= saved["currentRevision"]):
                raise RuntimeError("Invalid acceptance checkpoint; no process was killed")
            if (not isinstance(saved.get("buildPid"), int) or saved["buildPid"] <= 100
                    or not isinstance(saved.get("buildStartTicks"), str)
                    or not saved["buildStartTicks"].isdigit()
                    or saved.get("buildStarts") != 1
                    or not process_is_running(saved["buildPid"], saved["buildStartTicks"])):
                raise RuntimeError("The exact first candidate build is not running; no process was killed")
            running = command("shell", "pidof", package).stdout.split()
            if running != [str(pid)]:
                raise RuntimeError("Target PID changed; no process was killed")
            (args.output_dir / "owner-death-checkpoint.json").write_text(json.dumps(saved, indent=2) + "\n")
            command("shell", "run-as", package, "kill", "-9", str(pid))
            prepare.wait(timeout=20)
    # Both first-run and interrupted-run recovery must pass the same checkpoint.
    if (not isinstance(saved.get("buildPid"), int) or saved["buildPid"] <= 100
            or not isinstance(saved.get("buildStartTicks"), str)
            or not saved["buildStartTicks"].isdigit() or saved.get("buildStarts") != 1
            or "/.apps/owner-death-" not in saved.get("source", "")
            or not saved.get("markerPath", "").startswith(saved["source"] + "/")):
        raise RuntimeError("Invalid acceptance checkpoint")
    processes = command("shell", "pidof", package, check=False)
    if processes.stdout.strip() or processes.returncode not in (0, 1) or processes.stderr.strip():
        raise RuntimeError("A package process is already present; pre-recovery cleanup cannot be certified")
    # Check before starting any new instrumentation, which must not
    # accidentally clean up the leaked candidate on this test's behalf.
    deadline = time.monotonic() + 5
    while process_is_running(saved["buildPid"], saved["buildStartTicks"]):
        if time.monotonic() >= deadline:
            raise RuntimeError("Candidate build survived host SIGKILL")
        time.sleep(0.1)
    marker = command("shell", "run-as", package, "cat", saved["markerPath"])
    stopped = json.loads(marker.stdout)
    if (stopped["pid"] != saved["buildPid"] or stopped["startTicks"] != saved["buildStartTicks"]
            or stopped["starts"] != saved["buildStarts"]):
        raise RuntimeError("Candidate build restarted before cold recovery")
    (args.output_dir / "owner-death-stopped-build.json").write_text(json.dumps(stopped, indent=2) + "\n")
    result = subprocess.run(adb + ["shell", "am", "instrument", "-w", "-r", "-e", "class", test_class,
        "-e", "ownerDeathPhase", "recover", component], text=True, capture_output=True, timeout=120)
    (args.output_dir / "owner-death-recover.log").write_text(result.stdout + result.stderr)
    if result.returncode != 0 or "OK (1 test)" not in result.stdout:
        raise RuntimeError("Cold recovery failed: " + result.stdout[-6000:] + result.stderr[-1000:])
    print("PASS: actual candidate build, exact host PID SIGKILL, cold recovery, retained old release and data")


if __name__ == "__main__":
    main()
