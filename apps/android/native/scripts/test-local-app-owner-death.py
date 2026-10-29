#!/usr/bin/env python3
"""Run the two-stage owner-death acceptance on the disposable CI AVD.

The instrumented test kills its own Android process after writing each receipt.
This runner only observes the recorded PID/start-time identities; it never
signals a PID and never removes device paths. The isolated configHome remains on
the CI AVD until android-emulator-runner tears that AVD down at job end.
Its one-time capability binds each phase to this runner invocation; it is not
device-side attestation of the GitHub runner or emulator's disposability.
"""
import argparse
import json
import os
from pathlib import Path, PurePosixPath
import re
import secrets
import subprocess
import time
import uuid


PACKAGE = "com.lingxi.code.direct.debug"
COMPONENT = PACKAGE + ".test/androidx.test.runner.AndroidJUnitRunner"
TEST_CLASS = "com.lingxi.code.apps.UnifiedLocalAppProcessDeathTest"
RECEIPT = "files/local-app-owner-death-acceptance.json"
SETUP_RECEIPT = "files/local-app-owner-death-setup.json"
CI_CAPABILITY = "files/local-app-owner-death-ci-capability"
RECEIPT_STATE_PATHS = (
    RECEIPT, RECEIPT + ".bak", RECEIPT + ".new",
    SETUP_RECEIPT, SETUP_RECEIPT + ".bak", SETUP_RECEIPT + ".new",
    CI_CAPABILITY,
)
CONFIG_PREFIX = ".lingxi-owner-death-acceptance-"
OWNER_MARKER = ".owner-death-acceptance-owner"


def instrumentation_args(phase, capability, run_nonce):
    if phase not in ("prepare", "recover"):
        raise ValueError("owner-death phase must be prepare or recover")
    if not re.fullmatch(r"[0-9a-f]{64}", capability):
        raise ValueError("owner-death phase requires a runner-issued capability")
    if not re.fullmatch(r"[0-9a-f]{64}", run_nonce):
        raise ValueError("owner-death phase requires a runner-issued run nonce")
    return ["shell", "am", "instrument", "-w", "-r", "-e", "class", TEST_CLASS,
            "-e", "ownerDeathAcceptance", "true", "-e", "ownerDeathCiCapability", capability,
            "-e", "ownerDeathRunNonce", run_nonce, "-e", "ownerDeathPhase", phase, COMPONENT]


def require_ephemeral_ci_emulator(*, ci, github_actions, runner_environment, serial, qemu_property):
    if ci != "true" or github_actions != "true" or runner_environment != "github-hosted":
        raise RuntimeError("Owner-death self-SIGKILL is restricted to a GitHub-hosted Actions job")
    if not re.fullmatch(r"emulator-[0-9]+", serial):
        raise RuntimeError("Owner-death self-SIGKILL requires an emulator serial")
    if qemu_property.strip() != "1":
        raise RuntimeError("The selected Android device did not identify itself as a QEMU emulator")


def parse_proc_stat(stat):
    close = stat.rfind(")")
    if close < 0:
        raise RuntimeError("Malformed /proc/<pid>/stat command field")
    fields = stat[close + 1:].split()
    if len(fields) <= 19:
        raise RuntimeError("Malformed /proc/<pid>/stat fields")
    return fields[0], fields[19]


def process_is_running(command, package, pid, start_ticks):
    result = command("shell", "run-as", package, "cat", f"/proc/{pid}/stat", check=False)
    if result.returncode != 0:
        detail = (result.stdout + result.stderr).lower()
        if "no such file" in detail or "not found" in detail:
            return False
        raise RuntimeError("Cannot observe the recorded Android process identity: " + result.stderr + result.stdout)
    state, observed_start = parse_proc_stat(result.stdout)
    return state != "Z" and observed_start == start_ticks


def wait_for_process_exit(command, package, pid, start_ticks, *, timeout=20.0):
    deadline = time.monotonic() + timeout
    while process_is_running(command, package, pid, start_ticks):
        if time.monotonic() >= deadline:
            raise RuntimeError(f"Recorded Android process {pid}/{start_ticks} did not exit")
        time.sleep(0.1)


def _canonical_uuid(value):
    if not isinstance(value, str):
        return False
    try:
        return str(uuid.UUID(value)) == value
    except ValueError:
        return False


def validate_receipt(receipt, app_data_root, *, recovered, run_nonce=None):
    if run_nonce is not None and receipt.get("ownerDeathRunNonce") != run_nonce:
        raise RuntimeError("Owner-death receipt does not belong to this runner invocation")
    expected_phase = "recovered" if recovered else "prepared"
    if receipt.get("ownerDeathPhase") != expected_phase:
        raise RuntimeError(f"Owner-death receipt phase is not {expected_phase}")
    process_name_key = "recoveryProcessName" if recovered else "hostProcessName"
    if receipt.get(process_name_key) != PACKAGE:
        raise RuntimeError("Instrumentation was not running inside the target app/service process")
    if recovered:
        for flag in ("recoveryComplete", "appRemoved", "backendPortsClosed", "candidateBuildStopped"):
            if receipt.get(flag) is not True:
                raise RuntimeError(f"Recovery receipt did not certify {flag}")
        if receipt.get("recoverySelfKillRequested") is not True:
            raise RuntimeError("Recovery did not record its in-process self-SIGKILL request")
    elif receipt.get("recoveryComplete") is not False:
        raise RuntimeError("Prepare checkpoint must remain pending until recovery")
    elif receipt.get("prepareSelfKillRequested") is not True:
        raise RuntimeError("Prepare did not record its in-process self-SIGKILL request")

    token = receipt.get("configToken")
    if not _canonical_uuid(token):
        raise RuntimeError("configToken is not a canonical UUID")
    app_root = PurePosixPath(app_data_root)
    persistent = app_root / "files/mobile-linux/android-proot/persistent"
    config_home = PurePosixPath(str(receipt.get("configHome", "")))
    if not config_home.is_absolute() or ".." in config_home.parts:
        raise RuntimeError("configHome is not an absolute normalized path")
    try:
        relative = config_home.relative_to(persistent)
    except ValueError as error:
        raise RuntimeError("configHome is outside this package's Linux persistent home") from error
    if len(relative.parts) != 2 or not re.fullmatch(r"[A-Za-z0-9_-]+", relative.parts[0]):
        raise RuntimeError("configHome is not a direct child of the Linux user home")
    if relative.parts[1] != CONFIG_PREFIX + token:
        raise RuntimeError("configHome basename does not match its UUID marker")

    source = PurePosixPath(str(receipt.get("source", "")))
    if not source.is_absolute() or ".." in source.parts or source.parent != config_home / ".apps":
        raise RuntimeError("source is not contained in this test's isolated .apps root")
    source_token = source.name.removeprefix("owner-death-")
    if not _canonical_uuid(source_token):
        raise RuntimeError("source directory is missing its canonical test UUID")
    marker = PurePosixPath(str(receipt.get("markerPath", "")))
    if marker != source / "build-in-progress":
        raise RuntimeError("build marker is not the exact test source file")

    app_id = receipt.get("appId")
    data_path = PurePosixPath(str(receipt.get("dataPath", "")))
    try:
        data_relative = data_path.relative_to(config_home / "local-apps/v2/apps")
    except ValueError as error:
        raise RuntimeError("app data path is outside the isolated local-app library") from error
    if len(data_relative.parts) != 2 or data_relative.parts[0] != app_id or data_relative.parts[1] != "data":
        raise RuntimeError("app data path does not match the receipt app identity")
    if not isinstance(receipt.get("data"), str):
        raise RuntimeError("receipt does not contain the preserved test data sentinel")

    pid_key = "recoveryHostPid" if recovered else "hostPid"
    ticks_key = "recoveryHostStartTicks" if recovered else "hostStartTicks"
    pid = receipt.get(pid_key)
    ticks = receipt.get(ticks_key)
    if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 100:
        raise RuntimeError(f"{pid_key} is invalid")
    if not isinstance(ticks, str) or not ticks.isdigit():
        raise RuntimeError(f"{ticks_key} is invalid")
    return {
        "configHome": str(config_home), "configToken": token, "source": str(source),
        "dataPath": str(data_path), "data": receipt["data"],
        "hostPid": pid, "hostStartTicks": ticks,
    }


def check_owned_paths(command, package, saved):
    for path in (saved["configHome"], saved["source"], saved["configHome"] + "/" + OWNER_MARKER):
        canonical = command("shell", "run-as", package, "readlink", "-f", path, check=False)
        if canonical.returncode != 0 or canonical.stdout.strip() != path:
            raise RuntimeError("Isolated owner-death path did not canonicalize to its receipt path: " + path)
    marker = command("shell", "run-as", package, "cat", saved["configHome"] + "/" + OWNER_MARKER)
    if marker.stdout.strip() != saved["configToken"]:
        raise RuntimeError("Isolated configHome owner marker does not match the receipt")


def require_no_package_process(command, package):
    result = command("shell", "pidof", package, check=False)
    if result.stdout.strip() or result.returncode not in (0, 1) or result.stderr.strip():
        raise RuntimeError("The app process restarted or another target package process remains")


def stage_ci_capability(command, capability):
    """Stage a one-time runner/test handshake after validating the host gate."""
    if not re.fullmatch(r"[0-9a-f]{64}", capability):
        raise ValueError("CI capability must be 32 random bytes encoded as lowercase hex")
    existing = command("shell", "run-as", PACKAGE, "test", "-e", CI_CAPABILITY, check=False)
    if existing.returncode == 0:
        raise RuntimeError("A previous CI capability remains unconsumed")
    if existing.returncode != 1:
        raise RuntimeError("Could not inspect the one-time CI capability path")
    # The random token is hex-only, so this fixed shell command has no user-controlled syntax.
    command("shell", "run-as", PACKAGE, "sh", "-c",
            f"printf '%s' '{capability}' > '{CI_CAPABILITY}'")
    written = command("shell", "run-as", PACKAGE, "cat", CI_CAPABILITY)
    if written.stdout != capability:
        raise RuntimeError("Could not verify the one-time CI capability")


def require_ci_capability_consumed(command):
    remaining = command("shell", "run-as", PACKAGE, "test", "-e", CI_CAPABILITY, check=False)
    if remaining.returncode == 0:
        raise RuntimeError("The instrumented process did not consume its one-time CI capability")
    if remaining.returncode != 1:
        raise RuntimeError("Could not verify consumption of the one-time CI capability")


def require_no_existing_owner_death_state(command):
    for path in RECEIPT_STATE_PATHS:
        exists = command("shell", "run-as", PACKAGE, "test", "-e", path, check=False)
        if exists.returncode == 0:
            raise RuntimeError("A previous owner-death receipt or AtomicFile sidecar exists; use a fresh CI emulator")
        if exists.returncode != 1:
            raise RuntimeError("Could not inspect prior owner-death receipts")


def is_completed_prepare_receipt(candidate, run_nonce):
    return (isinstance(candidate, dict)
            and candidate.get("ownerDeathPhase") == "prepared"
            and candidate.get("ownerDeathRunNonce") == run_nonce
            and candidate.get("prepareSelfKillRequested") is True)


def read_completed_prepare_receipt(command, run_nonce, *, attempts=6, retry_delay=0.1):
    """Re-read a committed prepare receipt after the instrumentation host exits.

    AtomicFile may briefly leave the base path absent while replacing it. If the
    test process exits during that window, one failed `cat` is not evidence that
    no checkpoint was committed. Retry a small, fixed number of bounded reads;
    accept only the expected prepare phase and its self-kill marker.
    """
    last_error = "receipt path is not readable"
    for attempt in range(attempts):
        result = command("shell", "run-as", PACKAGE, "cat", RECEIPT,
                         check=False, timeout=5)
        if result.returncode == 0:
            try:
                candidate = json.loads(result.stdout)
            except json.JSONDecodeError as error:
                last_error = "receipt is not complete JSON: " + str(error)
            else:
                if is_completed_prepare_receipt(candidate, run_nonce):
                    return candidate
                last_error = "receipt is not the completed prepare checkpoint"
        elif result.returncode == 1:
            last_error = result.stderr.strip() or "receipt path is not readable"
        else:
            raise RuntimeError("Could not re-read the completed prepare checkpoint: "
                               + result.stderr + result.stdout)
        if attempt + 1 < attempts:
            time.sleep(retry_delay)
    raise RuntimeError("Prepare receipt was unavailable or invalid after instrumentation exited: "
                       + last_error)


def wait_for_prepare_checkpoint(command, prepare, run_nonce, *, timeout=120):
    """Observe the prepare checkpoint, including a final read after host exit."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = command("shell", "run-as", PACKAGE, "cat", RECEIPT, check=False)
        candidate = None
        if result.returncode == 0:
            try:
                candidate = json.loads(result.stdout)
            except json.JSONDecodeError:
                if prepare.poll() is not None:
                    return read_completed_prepare_receipt(command, run_nonce)
            if is_completed_prepare_receipt(candidate, run_nonce):
                return candidate
        if result.returncode not in (0, 1):
            raise RuntimeError("Could not read the prepare checkpoint: " + result.stderr)
        if prepare.poll() is not None:
            return read_completed_prepare_receipt(command, run_nonce)
        time.sleep(0.25)
    # The last sleep can cross the deadline after the checkpoint was committed.
    # Sample once more even if the instrumentation process is still exiting;
    # main subsequently waits for it and verifies the recorded process identity.
    result = command("shell", "run-as", PACKAGE, "cat", RECEIPT, check=False, timeout=5)
    if result.returncode == 0:
        try:
            candidate = json.loads(result.stdout)
        except json.JSONDecodeError:
            candidate = None
        if is_completed_prepare_receipt(candidate, run_nonce):
            return candidate
    elif result.returncode != 1:
        raise RuntimeError("Could not read the prepare checkpoint: " + result.stderr)
    if prepare.poll() is not None:
        return read_completed_prepare_receipt(command, run_nonce)
    raise RuntimeError("Prepare did not write its checkpoint before timeout")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--ci-ephemeral-emulator", action="store_true")
    args = parser.parse_args()
    if not args.ci_ephemeral_emulator:
        raise RuntimeError("Pass --ci-ephemeral-emulator only from the disposable CI AVD workflow")

    adb = [args.adb, "-s", args.serial]

    def command(*values, check=True, timeout=20):
        return subprocess.run(adb + list(values), text=True, capture_output=True,
                              check=check, timeout=timeout)

    qemu = command("shell", "getprop", "ro.kernel.qemu").stdout.strip()
    require_ephemeral_ci_emulator(
        ci=os.environ.get("CI", ""),
        github_actions=os.environ.get("GITHUB_ACTIONS", ""),
        runner_environment=os.environ.get("RUNNER_ENVIRONMENT", ""),
        serial=args.serial,
        qemu_property=qemu,
    )
    args.output_dir.mkdir(parents=True, exist_ok=True)
    require_no_existing_owner_death_state(command)

    app_data_root = command("shell", "run-as", PACKAGE, "pwd").stdout.strip()
    app_data_canonical = command("shell", "run-as", PACKAGE, "readlink", "-f", app_data_root, check=False)
    if app_data_canonical.returncode != 0 or app_data_canonical.stdout.strip() != app_data_root:
        raise RuntimeError("Package app data root is not canonical")

    prepare_log = args.output_dir / "owner-death-prepare.log"
    run_nonce = secrets.token_hex(32)
    prepare_capability = secrets.token_hex(32)
    stage_ci_capability(command, prepare_capability)
    with prepare_log.open("w") as prepare_output:
        prepare = subprocess.Popen(adb + instrumentation_args("prepare", prepare_capability, run_nonce),
            stdout=prepare_output, stderr=subprocess.STDOUT)
    prepared = wait_for_prepare_checkpoint(command, prepare, run_nonce)

    checkpoint = validate_receipt(prepared, app_data_root, recovered=False, run_nonce=run_nonce)
    if not prepared.get("configCreated") or not prepared.get("ownerMarkerWritten"):
        raise RuntimeError("Prepare did not certify its unique isolated configHome")
    check_owned_paths(command, PACKAGE, checkpoint)
    build_pid = prepared.get("buildPid")
    build_ticks = prepared.get("buildStartTicks")
    if not isinstance(build_pid, int) or build_pid <= 100 or not isinstance(build_ticks, str) or not build_ticks.isdigit():
        raise RuntimeError("Candidate build process identity is invalid")
    if prepared.get("candidateRevision", 0) <= prepared.get("currentRevision", 0):
        raise RuntimeError("Candidate release did not advance beyond the serving revision")
    (args.output_dir / "owner-death-checkpoint.json").write_text(json.dumps(prepared, indent=2) + "\n")

    try:
        prepare.wait(timeout=30)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("Prepare instrumentation did not return after test-host self-SIGKILL") from error
    require_ci_capability_consumed(command)
    wait_for_process_exit(command, PACKAGE, checkpoint["hostPid"], checkpoint["hostStartTicks"])
    require_no_package_process(command, PACKAGE)
    wait_for_process_exit(command, PACKAGE, build_pid, build_ticks, timeout=8.0)
    marker_result = command("shell", "run-as", PACKAGE, "cat", prepared["markerPath"])
    stopped_marker = json.loads(marker_result.stdout)
    if (stopped_marker.get("pid") != build_pid
            or stopped_marker.get("startTicks") != build_ticks
            or stopped_marker.get("starts") != prepared.get("buildStarts")):
        raise RuntimeError("Candidate build restarted after test-host self-SIGKILL")
    (args.output_dir / "owner-death-stopped-build.json").write_text(json.dumps(stopped_marker, indent=2) + "\n")

    recovery_log = args.output_dir / "owner-death-recover.log"
    recovery_capability = secrets.token_hex(32)
    stage_ci_capability(command, recovery_capability)
    recovery = subprocess.run(adb + instrumentation_args("recover", recovery_capability, run_nonce),
        text=True, capture_output=True, timeout=180)
    recovery_log.write_text(recovery.stdout + recovery.stderr)
    require_ci_capability_consumed(command)
    latest = command("shell", "run-as", PACKAGE, "cat", RECEIPT)
    recovered = json.loads(latest.stdout)
    if not isinstance(recovered, dict) or recovered.get("ownerDeathRunNonce") != run_nonce:
        raise RuntimeError("Recovery receipt does not belong to this runner invocation")
    if recovered.get("recoverySelfKillRequested") is not True:
        raise RuntimeError("Recovery instrumentation returned without its self-SIGKILL request")
    final = validate_receipt(recovered, app_data_root, recovered=True, run_nonce=run_nonce)
    if (final["configHome"] != checkpoint["configHome"]
            or final["configToken"] != checkpoint["configToken"]
            or final["source"] != checkpoint["source"]
            or (final["hostPid"], final["hostStartTicks"]) == (checkpoint["hostPid"], checkpoint["hostStartTicks"])):
        raise RuntimeError("Recovery changed fixture ownership or reused the prepare process")
    check_owned_paths(command, PACKAGE, final)
    wait_for_process_exit(command, PACKAGE, final["hostPid"], final["hostStartTicks"])
    require_no_package_process(command, PACKAGE)
    data_path = final["dataPath"] + "/acceptance.bin"
    data_canonical = command("shell", "run-as", PACKAGE, "readlink", "-f", data_path, check=False)
    if data_canonical.returncode != 0 or data_canonical.stdout.strip() != data_path:
        raise RuntimeError("Preserved test data file is missing or noncanonical after app removal")
    retained_data = command("shell", "run-as", PACKAGE, "cat", data_path)
    if retained_data.stdout != final["data"]:
        raise RuntimeError("Isolated app data changed after remove")
    (args.output_dir / "owner-death-retained-config.json").write_text(json.dumps({
        "configHome": final["configHome"], "configToken": final["configToken"],
        "source": final["source"], "dataPath": final["dataPath"],
        "preparePid": checkpoint["hostPid"], "prepareStartTicks": checkpoint["hostStartTicks"],
        "recoveryPid": final["hostPid"], "recoveryStartTicks": final["hostStartTicks"],
        "deviceDataPolicy": "retained until disposable CI AVD teardown; no app-data deletion performed",
        "instrumentationPrepareExit": prepare.returncode,
        "instrumentationRecoveryExit": recovery.returncode,
    }, indent=2) + "\n")
    print("PASS: self-SIGKILL, cold recovery, service stop, origin/data assertions; isolated configHome retained on disposable CI AVD")


if __name__ == "__main__":
    main()
