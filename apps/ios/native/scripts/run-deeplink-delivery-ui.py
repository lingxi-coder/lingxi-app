#!/usr/bin/env python3
"""Run deeplink UI acceptance with a genuine warm URL from outside the app.

The warm test publishes a marker in its runner sandbox after asserting its draft.
This driver sends that URL through simctl, preserving the target process. Other
tests exercise cold XCUIApplication.open delivery. Requires an existing build.
"""
import argparse
import json
import pathlib
import plistlib
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--simulator", required=True)
    parser.add_argument("--xctestrun", type=pathlib.Path, required=True)
    parser.add_argument("--result", type=pathlib.Path, required=True)
    parser.add_argument("--log", type=pathlib.Path, required=True)
    parser.add_argument("--only-warm", action="store_true", help="Rerun only the external warm-delivery case")
    args = parser.parse_args()
    configuration = plistlib.loads(args.xctestrun.read_bytes())
    tests = configuration.get("TestConfigurations", [])
    targets = [target for config in tests for target in config.get("TestTargets", [])]
    if not targets:
        targets = [value for value in configuration.values() if isinstance(value, dict)]
    matching = [target for target in targets if target.get("BlueprintName") == "LingxiCodeUITests"]
    if len(matching) != 1:
        raise RuntimeError("Expected exactly one LingxiCodeUITests target")
    matching[0].setdefault("EnvironmentVariables", {})["LINGXI_EXTERNAL_URL_DRIVER"] = "1"
    def resolve_testroot(value):
        if isinstance(value, str):
            return value.replace("__TESTROOT__", str(args.xctestrun.resolve().parent))
        if isinstance(value, list):
            return [resolve_testroot(item) for item in value]
        if isinstance(value, dict):
            return {key: resolve_testroot(item) for key, item in value.items()}
        return value

    configuration = resolve_testroot(configuration)
    runner = "com.lingxi.code.uitests.xctrunner"
    def runner_marker():
        container = subprocess.run(["xcrun", "simctl", "get_app_container", args.simulator,
                                    runner, "data"], capture_output=True, text=True)
        if container.returncode == 0:
            return pathlib.Path(container.stdout.strip()) / "tmp/lingxi-deeplink-warm-url"
        return None

    stale_marker = runner_marker()
    if stale_marker is not None:
        stale_marker.unlink(missing_ok=True)
        stale_marker.with_suffix(".sent").unlink(missing_ok=True)
    sent = False
    with tempfile.TemporaryDirectory(prefix="lingxi-deeplink-uitest-") as scratch:
        runfile = pathlib.Path(scratch) / "WarmDelivery.xctestrun"
        runfile.write_bytes(plistlib.dumps(configuration))
        selector = "LingxiCodeUITests/DeepLinkDeliveryUITests"
        if args.only_warm:
            selector += "/testExternalNavigationRetainsDraftUntilConfirmed"
        command = ["xcodebuild", "test-without-building", "-xctestrun", str(runfile),
                   "-destination", "platform=iOS Simulator,id=" + args.simulator,
                   "-parallel-testing-enabled", "NO", "-maximum-concurrent-test-simulator-destinations", "1",
                   "-only-testing:" + selector,
                   "-resultBundlePath", str(args.result)]
        with args.log.open("w") as output:
            process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT)
            try:
                while process.poll() is None:
                    marker = runner_marker() if not sent else None
                    if not sent and marker is not None and marker.exists():
                        url = marker.read_text().strip()
                        if url != "lingxi://settings/general":
                            raise RuntimeError("Unexpected warm test URL")
                        subprocess.run(["xcrun", "simctl", "openurl", args.simulator, url], check=True)
                        marker.with_suffix(".sent").write_text("sent")
                        sent = True
                        print(json.dumps({"warm_os_delivery": url, "simulator": args.simulator}), flush=True)
                    time.sleep(0.25)
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait()
            if not sent:
                raise RuntimeError("Warm delivery test never requested its external URL")
            return process.returncode


if __name__ == "__main__":
    raise SystemExit(main())
