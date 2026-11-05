#!/usr/bin/env python3
"""Run the feature UI cases in separate processes; never clear application data.

ActivityScenario teardown stalled during shared-device runs; isolate each case
to preserve independent results.
Each invocation checks USB transport again and stops only this debug test app.
Install the product/test APKs from the same build before running this script.
"""
from contextlib import contextmanager
import argparse
import hashlib
import json
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
CLASSES = {
    "com.lingxi.code.bots.BotsTabUiTest": "bots/BotsTabUiTest.kt",
    "com.lingxi.code.tabs.TabShellUiTest": "tabs/TabShellUiTest.kt",
    "com.lingxi.code.settings.SettingsTabUiTest": "settings/SettingsTabUiTest.kt",
    "com.lingxi.code.teams.TeamsTabUiTest": "teams/TeamsTabUiTest.kt",
}
PACKAGE = "com.lingxi.code.debug"
RUNNER = PACKAGE + ".test/androidx.test.runner.AndroidJUnitRunner"


def adb(*args, timeout=20):
    return subprocess.run(["adb", *args], text=True, capture_output=True, timeout=timeout)


def usb_line(serial):
    result = adb("devices", "-l")
    result.check_returncode()
    line = next((line for line in result.stdout.splitlines() if line.split()[:2] == [serial, "device"]), "")
    if not re.search(r"\busb:", line):
        raise RuntimeError("Device must remain attached over USB: " + serial)
    return line


def digest(path):
    sha = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            sha.update(block)
    return sha.hexdigest()


@contextmanager
def orientation(serial, requested, output):
    keys = ("accelerometer_rotation", "user_rotation")
    original = {key: adb("-s", serial, "shell", "settings", "get", "system", key).stdout.strip() for key in keys}
    record = {"original": original, "requested": requested, "restored": False}
    (output / "device-config.json").write_text(json.dumps(record, indent=2) + "\n")
    try:
        if requested:
            adb("-s", serial, "shell", "settings", "put", "system", "accelerometer_rotation", "0").check_returncode()
            adb("-s", serial, "shell", "settings", "put", "system", "user_rotation", "0" if requested == "portrait" else "1").check_returncode()
        yield
    finally:
        if requested:
            for key, value in original.items():
                command = "delete" if value == "null" else "put"
                args = ["-s", serial, "shell", "settings", command, "system", key]
                if command == "put": args.append(value)
                adb(*args).check_returncode()
        record["restored"] = True
        (output / "device-config.json").write_text(json.dumps(record, indent=2) + "\n")


def interrupted(signum, frame):
    raise KeyboardInterrupt()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--test", action="append", help="One class#method from the feature suite; repeatable")
    parser.add_argument("--timeout", type=int, default=60)
    parser.add_argument("--orientation", choices=["portrait", "landscape"])
    args = parser.parse_args()
    if args.output.exists():
        parser.error("Use a new output directory to preserve prior evidence")
    transport = usb_line(args.device)
    manufacturer = adb("-s", args.device, "shell", "getprop", "ro.product.manufacturer").stdout.strip()
    source = ROOT / "apps/android/native/app/src/androidTest/java/com/lingxi/code"
    cases = []
    for name, relative in CLASSES.items():
        cases.extend(name + "#" + method for method in re.findall(r"@Test\s+fun\s+(\w+)", (source / relative).read_text()))
    if args.test:
        unknown = set(args.test) - set(cases)
        if unknown:
            parser.error("Unknown test: " + ", ".join(sorted(unknown)))
        cases = args.test
    args.output.mkdir(parents=True)
    summary = {
        "device": args.device, "manufacturer": manufacturer, "transport": transport,
        "dataCleared": False, "processIsolation": True,
        "testSourceSHA256": {name: digest(source / relative) for name, relative in CLASSES.items()},
        "tests": [],
    }
    signal.signal(signal.SIGTERM, interrupted)
    with orientation(args.device, args.orientation, args.output):
        for index, case in enumerate(cases, 1):
            line = usb_line(args.device)
            adb("-s", args.device, "shell", "am", "force-stop", PACKAGE).check_returncode()
            adb("-s", args.device, "shell", "am", "force-stop", PACKAGE + ".test").check_returncode()
            started = time.monotonic()
            try:
                result = adb("-s", args.device, "shell", "am", "instrument", "-w", "-r", "-e", "class", case, RUNNER, timeout=args.timeout)
                output = result.stdout + result.stderr
                ok = result.returncode == 0 and "OK (" in output and "FAILURES!!!" not in output
                state = "passed" if ok else "failed"
                if ok and re.search(r"INSTRUMENTATION_STATUS_CODE: -(3|4)\b", output):
                    state = "skipped"
            except subprocess.TimeoutExpired as error:
                output = "HOST TIMEOUT\n" + str(error.stdout or "") + str(error.stderr or "")
                state = "timeout"
                adb("-s", args.device, "shell", "am", "force-stop", PACKAGE)
            log = "{:03d}-{}.log".format(index, case.rsplit(".", 1)[-1].replace("#", "-"))
            (args.output / log).write_text(line + "\n" + output)
            summary["tests"].append({"test": case, "result": state, "seconds": round(time.monotonic() - started, 2), "log": log})
            summary["counts"] = {key: sum(test["result"] == key for test in summary["tests"]) for key in ["passed", "skipped", "failed", "timeout"]}
            (args.output / "results.json").write_text(json.dumps(summary, indent=2) + "\n")
            print("{}/{} {} {}".format(index, len(cases), state, case), flush=True)
    return 1 if summary["counts"]["failed"] or summary["counts"]["timeout"] else 0


if __name__ == "__main__":
    sys.exit(main())
