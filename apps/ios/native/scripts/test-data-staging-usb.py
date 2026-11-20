"""Run report UI tests against local synthetic staging through a paired iPhone."""
import argparse
import asyncio
import base64
import contextlib
import json
import os
import plistlib
import secrets
import socket
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlsplit


def loopback_port(value):
    url = urlsplit(value if "://" in value else "http://" + value)
    if url.scheme != "http" or url.hostname != "127.0.0.1" or not url.port or url.port < 1024:
        raise ValueError("Expected a local staging endpoint")
    return url.port


def endpoints(staging_log, browser_log):
    staging = json.loads(next(line for line in staging_log.read_text().splitlines() if line.startswith('{"web"')))
    if staging["project"] != "demo-lingxi-data":
        raise ValueError("Expected the isolated Data project")
    ports = {"LINGXI_DATA_STAGING_" + key.upper(): loopback_port(staging[key]) for key in ("auth", "firestore", "api")}
    if browser_log:
        browser = json.loads(next(line for line in browser_log.read_text().splitlines() if line.startswith('{"url"')))
        if urlsplit(browser["url"]).path != "/launch":
            raise ValueError("Expected the one-time Local App launcher")
        ports["LINGXI_REPORT_LAUNCH_PORT"] = loopback_port(browser["url"])
        ports["LINGXI_REPORT_GATEWAY_PORT"] = loopback_port(f"127.0.0.1:{browser['gatewayPort']}")
    if len(set(ports.values())) != len(ports):
        raise ValueError("Staging services must have separate ports")
    return ports


async def pump(reader, writer):
    while chunk := await reader.read(65536):
        writer.write(chunk)
        await writer.drain()
    if writer.can_write_eof():
        writer.write_eof()


async def forward(control_port, service_port, secret, counts, ready):
    while True:
        writers = []
        try:
            control, to_phone = await asyncio.open_connection("127.0.0.1", control_port)
            writers.append(to_phone)
            to_phone.write(secret)
            await to_phone.drain()
            if await control.readexactly(1) != b"A":
                raise ValueError("USB control was not authenticated")
            if service_port not in ready:
                ready.add(service_port)
                print("Authenticated staging control", service_port, flush=True)
            if await control.readexactly(1) != b"R":
                raise ValueError("Invalid staging handshake")
            service, to_service = await asyncio.open_connection("127.0.0.1", service_port)
            writers.append(to_service)
            counts[service_port] += 1
            async with asyncio.TaskGroup() as tasks:
                tasks.create_task(pump(control, to_service))
                tasks.create_task(pump(service, to_phone))
        except (OSError, EOFError, ValueError, ExceptionGroup):
            pass
        finally:
            for writer in writers:
                writer.close()
        await asyncio.sleep(0.25)


async def run(args):
    paired = subprocess.check_output(["idevice_id", "-l"], text=True).splitlines()
    if args.device not in paired:
        raise SystemExit("Requested iPhone is not connected over USB")
    ports = endpoints(args.staging_log, args.browser_log)
    secret = secrets.token_bytes(32)
    environment = {key: str(value) for key, value in ports.items()}
    environment["LINGXI_REPORT_USB_SECRET"] = base64.b64encode(secret).decode()
    counts = dict.fromkeys(ports.values(), 0)
    ready = set()
    # Keep the prepared test run next to the original so __TESTROOT__ still resolves.
    runfile = plistlib.loads(args.xctestrun.read_bytes())
    if "TestConfigurations" in runfile:
        targets = [target for config in runfile["TestConfigurations"] for target in config["TestTargets"]]
    else:
        targets = [entry for entry in runfile.values() if isinstance(entry, dict)]
    selected = [target for target in targets if target.get("BlueprintName") == args.target]
    if len(selected) != 1:
        raise ValueError("Expected one Data staging UI test target")
    selected[0].setdefault("EnvironmentVariables", {}).update(environment)
    with tempfile.NamedTemporaryFile(dir=args.xctestrun.parent, suffix=".xctestrun") as private_run:
        private_run.write(plistlib.dumps(runfile))
        private_run.flush()
        controls = []
        with contextlib.ExitStack() as reservations:
            for _ in ports:
                sock = reservations.enter_context(socket.socket())
                sock.bind(("127.0.0.1", 0))
                controls.append(sock.getsockname()[1])
        mappings = [f"{port}:{61231 + i}" for i, port in enumerate(controls)]
        proxy = await asyncio.create_subprocess_exec("iproxy", "--local", "-s", "127.0.0.1", "-u", args.device, *mappings,
                                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        workers = [asyncio.create_task(forward(control, port, secret, counts, ready))
                   for control, port in zip(controls, ports.values()) for _ in range(6)]
        tests = args.tests or (["testReaderOpensPublishedReport", "testDeviceLocalAppOpensReportAndReviews"]
                               if args.browser_log else ["testReaderOpensPublishedReport"])
        selection = [f"-only-testing:{args.target}/LingxiDataStagingUITests/{test}" for test in tests]
        try:
            process = await asyncio.create_subprocess_exec("xcodebuild", "test-without-building", "-xctestrun", private_run.name,
                "-destination", "platform=iOS,id=" + args.device, "-parallel-testing-enabled", "NO", *selection,
                "-resultBundlePath", os.fspath(args.result))
            return await process.wait()
        finally:
            for task in workers:
                task.cancel()
            await asyncio.gather(*workers, return_exceptions=True)
            if proxy.returncode is None:
                proxy.terminate()
            await proxy.wait()
            print("USB staging service connections", {key: counts[value] for key, value in ports.items()}, flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", required=True)
    parser.add_argument("--staging-log", required=True, type=Path)
    parser.add_argument("--browser-log", type=Path)
    parser.add_argument("--xctestrun", required=True, type=Path)
    parser.add_argument("--result", required=True, type=Path)
    parser.add_argument("--target", default="LingxiCodeUITests")
    parser.add_argument("--test", dest="tests", action="append")
    raise SystemExit(asyncio.run(run(parser.parse_args())))
