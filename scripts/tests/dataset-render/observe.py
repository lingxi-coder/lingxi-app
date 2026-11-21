"""Run an identical synthetic page on a private control network and in an isolated gVisor sandbox."""

import base64
import collections
import contextlib
import http.server
import hashlib
import json
import math
import os
import secrets
import socket
import socketserver
import ssl
import struct
import subprocess
import sys
import threading
import time
from pathlib import Path
from urllib.request import urlopen

IP = "172.29.76.2"
NETWORK = "dataset-observer"
IMAGE = "lingxi-render-probe:fixture"
OBSERVER = "observer-peer"
TALLY = Path("/tmp/observer.json")
counts = collections.Counter()
lock = threading.Lock()


def count(protocol):
    with lock:
        counts[protocol] += 1
        scratch = TALLY.with_suffix(".tmp")
        scratch.write_text(json.dumps(counts))
        scratch.replace(TALLY)


class HttpObserver(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/app-"):
            count("documents")
            page = Path("/opt/lingxi/fixture.html").read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(page)))
            self.end_headers()
            self.wfile.write(page)
            return
        count("https" if isinstance(self.connection, ssl.SSLSocket) else "http")
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Cache-Control", "no-store")
        self.end_headers()

    def log_message(self, fmt, *args):
        pass


class DnsObserver(socketserver.BaseRequestHandler):
    def handle(self):
        packet, connection = self.request
        if len(packet) < 17:
            return
        count("dns")
        offset = 12
        while offset < len(packet) and 0 < packet[offset] <= 63:
            offset += packet[offset] + 1
        if offset + 5 > len(packet) or packet[offset] != 0:
            return
        question = packet[12 : offset + 5]
        answer = b""
        if question[-4:] == b"\x00\x01\x00\x01":
            answer = bytes.fromhex("c00c00010001000000000004") + socket.inet_aton(IP)
        header = packet[:2] + struct.pack("!HHHHH", 0x8180, 1, int(bool(answer)), 0, 0)
        connection.sendto(header + question + answer, self.client_address)


class StunObserver(socketserver.BaseRequestHandler):
    def handle(self):
        packet, connection = self.request
        if (
            len(packet) < 20
            or packet[:2] != b"\x00\x01"
            or packet[4:8] != bytes.fromhex("2112a442")
        ):
            return
        count("stun")
        host, port = self.client_address
        address = int.from_bytes(socket.inet_aton(host), "big") ^ 0x2112A442
        mapping = struct.pack("!HHBBHI", 0x0020, 8, 0, 1, port ^ 0x2112, address)
        connection.sendto(
            struct.pack("!HH", 0x0101, len(mapping)) + packet[4:20] + mapping,
            self.client_address,
        )


class TurnObserver(socketserver.BaseRequestHandler):
    def handle(self):
        packet, _ = self.request
        if (
            len(packet) >= 20
            and packet[:2] == bytes.fromhex("0003")
            and packet[4:8] == bytes.fromhex("2112a442")
        ):
            count("turn")


def observer_counts():
    return json.loads(
        subprocess.check_output(
            ["docker", "exec", OBSERVER, "cat", str(TALLY)], text=True
        )
    )


def serve_observer():
    with contextlib.ExitStack() as stack:
        https = http.server.ThreadingHTTPServer((IP, 18443), HttpObserver)
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain("/tmp/server.crt", "/tmp/server.key")
        https.socket = tls.wrap_socket(https.socket, server_side=True)
        for server in (
            http.server.ThreadingHTTPServer((IP, 18080), HttpObserver),
            https,
            socketserver.UDPServer((IP, 53), DnsObserver),
            socketserver.UDPServer((IP, 3478), StunObserver),
            socketserver.UDPServer((IP, 3479), TurnObserver),
        ):
            stack.enter_context(server)
            threading.Thread(target=server.serve_forever, daemon=True).start()
        TALLY.write_text("{}")
        threading.Event().wait()


def check_route():
    assert socket.gethostbyname("route.fixture.test") == IP
    with urlopen("http://" + IP + ":18080/route", timeout=4) as r:
        assert r.status == 204
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        udp.settimeout(4)
        transaction = secrets.token_bytes(12)
        udp.sendto(bytes.fromhex("000100002112a442") + transaction, (IP, 3478))
        packet, _ = udp.recvfrom(128)
        assert packet[8:20] == transaction


def drive_active_viewer(child):
    start = time.monotonic() + 0.5
    latencies, frame_bytes, wire_bytes = [], 0, 0
    for n in range(1, 121):
        time.sleep(max(0, start + (n - 1) * 5 - time.monotonic()))
        command = json.dumps({"type": "tap", "n": n}) + "\n"
        before = time.monotonic()
        child.stdin.write(command)
        child.stdin.flush()
        line = child.stdout.readline()
        message = json.loads(line)
        png = base64.b64decode(message["png"], validate=True)
        assert (
            message["phase"] == "screen"
            and message["n"] == n
            and message["quantity"] == 8 + n
        )
        assert png[:8] == bytes.fromhex("89504e470d0a1a0a") and struct.unpack(
            "!II", png[16:24]
        ) == (480, 640)
        assert hashlib.sha256(png).hexdigest() == message["sha256"]
        latencies.append(round((time.monotonic() - before) * 1000))
        frame_bytes += len(png)
        wire_bytes += len(line.encode()) + len(command.encode())
    time.sleep(max(0, start + 600 - time.monotonic()))
    child.stdin.write('{"type":"close"}\n')
    child.stdin.flush()
    return {
        "seconds": round(time.monotonic() - start, 3),
        "inputEverySeconds": 5,
        "frames": len(latencies),
        "hostInputToVerifiedFrameMs": latencies,
        "pngBytesReceived": frame_bytes,
        "pipeBytes": wire_bytes,
    }


def run_sandbox(mode, nonce, resolver, interact=drive_active_viewer):
    closed = mode != "control"
    runtime = "lingxi-closed" if closed else "runc"
    network = "none" if closed else NETWORK
    name = "render-" + nonce
    args = [
        "docker",
        "run",
        "--name",
        name,
        "--runtime",
        runtime,
        "--network",
        network,
        "--mount",
        resolver,
        "--mount",
        f"type=bind,src={Path.cwd()/'tls/ca.crt'},dst=/opt/lingxi/fixture-ca.crt,readonly",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=256m,mode=1777",
        "--shm-size=128m",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--cpus=2",
        "--memory=1g",
        "--pids-limit=180",
        "--user=65532:65532",
        IMAGE,
        nonce,
        mode,
    ]
    before = observer_counts()
    started = time.monotonic()
    if mode == "active":
        args.insert(2, "-i")
    child = subprocess.Popen(
        args,
        stdout=subprocess.PIPE,
        stdin=subprocess.PIPE if mode == "active" else subprocess.DEVNULL,
        text=True,
    )
    rows = []
    active = None
    try:
        for line in child.stdout:
            item = json.loads(line)
            if item["phase"] == "interactive":
                assert mode == "active" and active is None
                active = interact(child)
                continue
            if item["phase"] == "frame":
                item["containerStartToFrameMs"] = round(
                    (time.monotonic() - started) * 1000
                )
            rows.append(item)
        if child.wait(timeout=12):
            raise RuntimeError(mode + " Chromium fixture failed")
        state = json.loads(
            subprocess.check_output(["docker", "inspect", name], text=True)
        )[0]
        host = state["HostConfig"]
        if (
            host["Runtime"] != runtime
            or host["NetworkMode"] != network
            or not host["ReadonlyRootfs"]
            or host["Privileged"]
        ):
            raise ValueError("Unexpected sandbox configuration")
        assert len(rows) == 2 and [r["phase"] for r in rows] == ["frame", "result"]
    finally:
        subprocess.run(
            ["docker", "rm", "-f", name], stdout=subprocess.DEVNULL, check=True
        )
    after = observer_counts()
    observed = {
        key: after.get(key, 0) - before.get(key, 0)
        for key in ("http", "https", "dns", "stun", "turn", "documents")
    }
    return {
        "mode": mode,
        "runtime": runtime,
        "network": network,
        "observer": observed,
        "browser": rows,
        "active": active,
    }


def distribution(values):
    ordered = sorted(values)
    return {
        "n": len(values),
        "min": ordered[0],
        "p50": ordered[math.ceil(len(values) * 0.5) - 1],
        "p95": ordered[math.ceil(len(values) * 0.95) - 1],
        "max": ordered[-1],
    }


def main(mode):
    if mode not in ("isolation", "benchmark", "wire"):
        raise ValueError("Unexpected renderer measurement")
    if os.geteuid() != 0:
        raise RuntimeError("Expected a disposable staging VM")
    subprocess.run(
        [
            "docker",
            "network",
            "create",
            "--internal",
            "--subnet=172.29.76.0/24",
            "--gateway=172.29.76.1",
            NETWORK,
        ],
        check=True,
        stdout=subprocess.DEVNULL,
    )
    with contextlib.ExitStack() as stack:
        subprocess.run(
            [
                "docker",
                "run",
                "-d",
                "--name",
                OBSERVER,
                "--ip",
                IP,
                "--network",
                NETWORK,
                "--read-only",
                "--tmpfs",
                "/tmp:rw,nosuid,nodev,mode=1777",
                "--mount",
                f"type=bind,src={Path.cwd()/'tls/server.crt'},dst=/tmp/server.crt,readonly",
                "--mount",
                f"type=bind,src={Path.cwd()/'tls/server.key'},dst=/tmp/server.key,readonly",
                "--cap-drop=ALL",
                "--cap-add=NET_BIND_SERVICE",
                "--security-opt=no-new-privileges",
                "--user=0:0",
                "--entrypoint=/usr/bin/python3",
                IMAGE,
                "/opt/lingxi/observe.py",
                "serve",
            ],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        stack.callback(
            subprocess.run,
            ["docker", "rm", "-f", OBSERVER],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        for _ in range(50):
            if (
                subprocess.run(
                    ["docker", "exec", OBSERVER, "test", "-f", str(TALLY)]
                ).returncode
                == 0
            ):
                break
            time.sleep(0.2)
        resolver_file = Path.cwd() / "fixture-resolver"
        resolver_file.write_text(
            "nameserver " + IP + "\noptions timeout:1 attempts:1\n"
        )
        resolver_file.chmod(0o444)
        resolver = f"type=bind,src={resolver_file},dst=/etc/resolv.conf,readonly"
        subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "--runtime=runc",
                "--network",
                NETWORK,
                "--mount",
                resolver,
                "--cap-drop=ALL",
                "--read-only",
                "--security-opt=no-new-privileges",
                "--entrypoint=/usr/bin/python3",
                IMAGE,
                "/opt/lingxi/observe.py",
                "route",
            ],
            check=True,
        )
        results = []
        modes = (
            ("control",)
            if mode != "isolation"
            else ("control", "isolated", "isolated", "isolated")
        )
        for sample_mode in modes:
            result = run_sandbox(sample_mode, secrets.token_hex(6), resolver)
            results.append(result)
            Path("measurements.json").write_text(json.dumps(results))
        control = results[0]
        assert all(
            control["observer"].values()
        ), "The control must reach every observer"
        assert control["browser"][1]["httpStatuses"] == [204, 204]
        for sandbox in results[1:]:
            assert not any(
                sandbox["observer"].values()
            ), "Unexpected outbound connection"
            assert sandbox["browser"][1]["httpStatuses"] == [0, 0]
            assert sandbox["browser"][1]["iceOfferCreated"] is True
            assert sandbox["browser"][1]["turnOfferCreated"] is True
            assert (
                sandbox["browser"][0]["htmlSha256"]
                == control["browser"][0]["htmlSha256"]
            )
        report = {
            "gvisor": subprocess.check_output(
                ["runsc", "--version"], text=True
            ).splitlines()[0],
            "image": subprocess.check_output(
                ["docker", "image", "inspect", "-f", "{{.Id}}", IMAGE], text=True
            ).strip(),
            "businessCredentials": False,
            "publicIngress": False,
            "measurements": results,
        }
        if mode == "wire":
            from wire_worker import serve

            interact = serve()
            active = run_sandbox("active", secrets.token_hex(6), resolver, interact)
            assert not any(active["observer"].values())
            report["wire"] = active
        elif mode == "benchmark":
            samples = [
                run_sandbox("timing", secrets.token_hex(6), resolver) for _ in range(20)
            ]
            active = run_sandbox("active", secrets.token_hex(6), resolver)
            if any(any(sample["observer"].values()) for sample in [*samples, active]):
                raise RuntimeError("Benchmark viewer generated network traffic")
            frames = active["active"]
            assert frames["seconds"] >= 600 and frames["frames"] == 120
            cold = distribution(
                [s["browser"][0]["containerStartToFrameMs"] for s in samples]
            )
            warm = distribution(
                [s["browser"][0]["warmMountToFrameMs"] for s in samples]
            )
            inputs = distribution(frames["hostInputToVerifiedFrameMs"])
            report["benchmark"] = {
                "coldStartMs": cold,
                "pristineWarmMountMs": warm,
                "inputToVerifiedFrameMs": inputs,
                "activeViewer": frames,
                "observer": active["observer"],
                "scope": "fresh gVisor containers on a provisioned regional worker; trusted host-to-viewer pipe; no end-user Web network",
                "passedServerTiming": cold["p95"] <= 5000
                and warm["p95"] <= 2000
                and inputs["p95"] <= 250,
            }
        return report


if __name__ == "__main__":
    if sys.argv[1:] == ["serve"]:
        serve_observer()
    elif sys.argv[1:] == ["route"]:
        check_route()
    else:
        Path("result.json").write_text(
            json.dumps(main(sys.argv[1] if len(sys.argv) > 1 else "isolation"))
        )
