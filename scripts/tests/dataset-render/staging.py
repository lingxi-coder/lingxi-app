"""Run the renderer feasibility probe on one disposable, credential-free staging VM."""

import argparse
import base64
import gzip
import hashlib
import io
import json
import secrets
import subprocess
import tarfile
import tempfile
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen

PROJECT = "lingxi-dataset-v2-stg-261119"
ZONE = "us-central1-a"
REGION = "us-central1"
SUBNET = "dataset-offline-central"
ROOT = Path(__file__).resolve().parent


def gcloud(*args, read=False):
    cmd = [
        "gcloud",
        *args,
        "--project=" + PROJECT,
        "--quiet",
        "--format=" + ("json" if read else "none"),
    ]
    if read:
        return json.loads(subprocess.check_output(cmd, text=True))
    subprocess.run(cmd, check=True)


def package(work, filenames):
    archive = io.BytesIO()
    sources = {}
    with gzip.GzipFile(fileobj=archive, mode="wb", mtime=0) as gz, tarfile.open(
        fileobj=gz, mode="w|"
    ) as files:
        for filename in filenames:
            data = (ROOT / filename).read_bytes()
            sources[filename] = hashlib.sha256(data).hexdigest()
            info = tarfile.TarInfo(filename)
            info.size, info.mode = len(data), 0o644
            files.addfile(info, io.BytesIO(data))
    digest = hashlib.sha256(archive.getvalue()).hexdigest()
    payload = work / "fixture"
    payload.write_bytes(base64.b64encode(archive.getvalue()))
    return payload, digest, sources


def probe(evidence, work, benchmark):
    subnet = gcloud(
        "compute",
        "networks",
        "subnets",
        "describe",
        SUBNET,
        "--region=" + REGION,
        read=True,
    )
    rules = gcloud(
        "compute",
        "firewall-rules",
        "list",
        "--filter=network=" + subnet["network"],
        read=True,
    )
    if any(
        row["direction"] == "INGRESS" and not row.get("disabled") and row.get("allowed")
        for row in rules
    ):
        raise RuntimeError("Unexpected ingress rule on isolated staging network")
    receipt = json.loads((evidence / "regional-services.json").read_text())
    if receipt["project"] != PROJECT or receipt["region"] != REGION:
        raise ValueError("Wrong staging evidence location")
    tag = secrets.token_hex(6)
    name = "dataset-render-" + tag
    payload, digest, sources = package(
        work, ("observe.py", "Dockerfile", "browser.mjs", "fixture.html")
    )
    token = subprocess.check_output(
        ["gcloud", "auth", "print-access-token"], text=True
    ).strip()
    created = time.monotonic()
    gcloud(
        "compute",
        "instances",
        "create",
        name,
        "--zone=" + ZONE,
        "--subnet=" + SUBNET,
        "--image-family=debian-12",
        "--image-project=debian-cloud",
        "--machine-type=e2-standard-2",
        "--boot-disk-size=20GB",
        "--boot-disk-type=pd-balanced",
        "--network-tier=STANDARD",
        "--no-service-account",
        "--no-scopes",
        "--max-run-duration=25m",
        "--instance-termination-action=DELETE",
        "--labels=use=dataset-render-probe,case=" + tag,
        "--metadata=enable-guest-attributes=TRUE,block-project-ssh-keys=TRUE,fixture-sha256="
        + digest
        + ",workload="
        + ("benchmark" if benchmark else "isolation"),
        "--metadata-from-file=startup-script="
        + str(ROOT / "bootstrap.sh")
        + ",fixture="
        + str(payload),
    )
    try:
        machine = gcloud(
            "compute", "instances", "describe", name, "--zone=" + ZONE, read=True
        )
        if machine["labels"].get("case") != tag or machine.get("serviceAccounts"):
            raise RuntimeError("Unexpected renderer VM identity")
        endpoint = (
            "https://compute.googleapis.com/compute/v1/projects/"
            + PROJECT
            + "/zones/"
            + ZONE
            + "/instances/"
            + name
            + "/getGuestAttributes?queryPath=render%2F"
        )
        request = Request(endpoint, headers={"Authorization": "Bearer " + token})
        phase = None
        while time.monotonic() - created < 1200:
            try:
                with urlopen(request, timeout=20) as response:
                    attrs = {
                        i["key"]: i["value"]
                        for i in json.load(response)
                        .get("queryValue", {})
                        .get("items", [])
                    }
            except HTTPError as exc:
                if exc.code not in (404, 429, 503):
                    raise
                attrs = {}
            if attrs.get("phase") and attrs["phase"] != phase:
                phase = attrs["phase"]
                print("Renderer VM:", phase, flush=True)
            if "failure" in attrs:
                failed = {
                    k: attrs[k]
                    for k in ("failure", "partial", "diagnostic")
                    if k in attrs
                }
                (evidence / ("renderer-" + tag + "-incomplete.json")).write_text(
                    json.dumps(failed, indent=2)
                )
                raise RuntimeError(
                    attrs["failure"] + " " + attrs.get("diagnostic", "")[-650:]
                )
            if "result" in attrs:
                report = json.loads(attrs["result"])
                expected = 1 if benchmark else 4
                if (
                    len(report["measurements"]) != expected
                    or report["businessCredentials"]
                    or ("benchmark" in report) != benchmark
                ):
                    raise ValueError("Incomplete renderer probe")
                break
            time.sleep(8)
        else:
            raise TimeoutError("The renderer VM exceeded the test deadline")
    finally:
        mine = gcloud(
            "compute", "instances", "describe", name, "--zone=" + ZONE, read=True
        )
        if (
            mine["labels"].get("case") != tag
            or mine["labels"].get("use") != "dataset-render-probe"
        ):
            raise RuntimeError("Renderer VM ownership changed")
        gcloud("compute", "instances", "delete", name, "--zone=" + ZONE)
    seconds = round(time.monotonic() - created)
    if benchmark:
        active = report["benchmark"]["activeViewer"]
        rate = 0.06701142 + 20 * 0.000136986 + 0.005
        report["costScenario"] = {
            "currency": "USD",
            "rates": {
                "e2Standard2PerHour": 0.06701142,
                "pdBalancedGiBHour": 0.000136986,
                "externalIPv4PerHour": 0.005,
                "standardEgressPerGiB": 0.085,
            },
            "activeComputeDiskIp": round(active["seconds"] * rate / 3600, 6),
            "allOtherVmTimeAssignedToThisSession": round(
                (seconds - active["seconds"]) * rate / 3600, 6
            ),
            "equivalentInternetEgress": round(
                active["pipeBytes"] * 0.085 / (1024**3), 6
            ),
            "wholeVmPlusEquivalentEgressUsd": round(
                seconds * rate / 3600 + active["pipeBytes"] * 0.085 / (1024**3), 6
            ),
            "basis": "Iowa on-demand list price, full creation-to-deletion window assigned to one active session; actual host pipe bytes priced as standard internet egress before free tiers. User-network transport is measured in product acceptance.",
            "sources": [
                "https://cloud.google.com/products/compute/pricing/general-purpose?hl=en",
                "https://cloud.google.com/compute/disks-image-pricing",
                "https://cloud.google.com/vpc/network-pricing",
            ],
        }
    return {
        "project": PROJECT,
        "zone": ZONE,
        "machineType": "e2-standard-2",
        "vmDeleted": True,
        "provisionToDeletionSeconds": seconds,
        "fixtureSha256": digest,
        "sourceSha256": {
            **sources,
            "bootstrap.sh": hashlib.sha256(
                (ROOT / "bootstrap.sh").read_bytes()
            ).hexdigest(),
        },
        **report,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--benchmark", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="lingxi-render-probe-") as tmp:
        result = probe(args.evidence, Path(tmp), args.benchmark)
    output = "renderer-benchmark.json" if args.benchmark else "isolated-chromium.json"
    (args.evidence / output).write_text(json.dumps(result, indent=2) + "\n")
    print(
        json.dumps(
            {
                "gvisor": result["gvisor"],
                "samples": len(result["measurements"]),
                "vmDeleted": result["vmDeleted"],
            }
        )
    )
