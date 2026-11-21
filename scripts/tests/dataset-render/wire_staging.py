"""Observe a real HTTPS viewer on a separate same-zone browser VM, without public ingress."""

import argparse
import hashlib
import ipaddress
import json
import secrets
import subprocess
import tempfile
import time
from contextlib import ExitStack
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import HTTPRedirectHandler, Request, build_opener

from staging import PROJECT, REGION, ROOT, SUBNET, ZONE, gcloud, package

PORT = "18444"


class DirectEndpoint(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def run(evidence, work, stack):
    services = json.loads((evidence / "regional-services.json").read_text())
    if services["project"] != PROJECT or services["region"] != REGION:
        raise ValueError("Expected the dedicated staging project")
    subnet = gcloud(
        "compute",
        "networks",
        "subnets",
        "describe",
        SUBNET,
        "--region=" + REGION,
        read=True,
    )
    network = subnet["network"]
    if not network.endswith("/networks/dataset-offline"):
        raise ValueError("Unexpected renderer network")
    rules = gcloud(
        "compute", "firewall-rules", "list", "--filter=network=" + network, read=True
    )
    if any(
        rule["direction"] == "INGRESS"
        and not rule.get("disabled")
        and rule.get("allowed")
        for rule in rules
    ):
        raise ValueError("The renderer test requires an isolated network")
    case = secrets.token_hex(6)
    renderer, browser = [
        "dataset-wire-" + role + "-" + case for role in ("worker", "client")
    ]
    firewall = "dataset-wire-" + case
    files = (
        "observe.py",
        "Dockerfile",
        "browser.mjs",
        "fixture.html",
        "wire_worker.py",
        "wire_client.py",
        "wire-viewer.html",
    )
    archive, digest, sources = package(work, files)
    for filename in ("bootstrap.sh", "wire-client.sh"):
        sources[filename] = hashlib.sha256((ROOT / filename).read_bytes()).hexdigest()
    gcloud(
        "compute",
        "firewall-rules",
        "create",
        firewall,
        "--network=" + network.rsplit("/", 1)[1],
        "--direction=INGRESS",
        "--action=allow",
        "--rules=tcp:" + PORT,
        "--source-tags=" + browser,
        "--target-tags=" + renderer,
    )
    stack.callback(gcloud, "compute", "firewall-rules", "delete", firewall)
    rule = gcloud("compute", "firewall-rules", "describe", firewall, read=True)
    if (
        rule.get("sourceRanges")
        or rule["sourceTags"] != [browser]
        or rule["targetTags"] != [renderer]
        or rule["allowed"] != [{"IPProtocol": "tcp", "ports": [PORT]}]
    ):
        raise ValueError("Unexpected browser-to-renderer network scope")
    lifetimes, ids = {}, {}

    def create(name, startup, extra):
        began = time.monotonic()
        gcloud(
            "compute",
            "instances",
            "create",
            name,
            "--zone=" + ZONE,
            "--image-family=debian-12",
            "--image-project=debian-cloud",
            "--machine-type=e2-standard-2",
            "--boot-disk-size=20GB",
            "--boot-disk-type=pd-balanced",
            "--subnet=" + SUBNET,
            "--network-tier=STANDARD",
            "--no-service-account",
            "--no-scopes",
            "--max-run-duration=30m",
            "--instance-termination-action=DELETE",
            "--tags=" + name,
            "--labels=use=dataset-render-wire,case=" + case,
            "--metadata=enable-guest-attributes=TRUE,block-project-ssh-keys=TRUE,fixture-sha256="
            + digest
            + ","
            + extra,
            f"--metadata-from-file=startup-script={ROOT/startup},fixture={archive}",
        )

        def remove():
            found = gcloud(
                "compute", "instances", "list", "--filter=name=" + name, read=True
            )
            if found:
                if (
                    len(found) != 1
                    or found[0]["labels"].get("case") != case
                    or found[0]["labels"].get("use") != "dataset-render-wire"
                ):
                    raise ValueError("Renderer resource ownership changed")
                gcloud("compute", "instances", "delete", name, "--zone=" + ZONE)
            lifetimes[name] = round(time.monotonic() - began)

        stack.callback(remove)
        machine = gcloud(
            "compute", "instances", "describe", name, "--zone=" + ZONE, read=True
        )
        interface = machine["networkInterfaces"][0]
        if machine.get("serviceAccounts") or ipaddress.ip_address(
            interface["networkIP"]
        ) not in ipaddress.ip_network(subnet["ipCidrRange"]):
            raise ValueError("Unexpected observer identity or network")
        ids[name] = machine["id"]
        return interface["networkIP"]

    host = create(renderer, "bootstrap.sh", "workload=wire")
    peer = create(browser, "wire-client.sh", "worker-ip=" + host)
    gcloud(
        "compute",
        "instances",
        "add-metadata",
        renderer,
        "--zone=" + ZONE,
        "--metadata=client-ip=" + peer,
    )
    assert ids[renderer] != ids[browser] and host != peer
    token = subprocess.check_output(
        ["gcloud", "auth", "print-access-token"], text=True
    ).strip()
    direct = build_opener(DirectEndpoint())

    def attributes(name):
        url = f"https://compute.googleapis.com/compute/v1/projects/{PROJECT}/zones/{ZONE}/instances/{name}/getGuestAttributes?queryPath=render%2F"
        try:
            with direct.open(
                Request(url, headers={"Authorization": "Bearer " + token}), timeout=15
            ) as resp:
                pairs = json.load(resp).get("queryValue", {}).get("items", [])
        except HTTPError as exc:
            if exc.code in (404, 429, 503):
                return {}
            raise
        attrs = {x["key"]: x["value"] for x in pairs}
        if attrs.get("failure"):
            why = attrs["failure"] + " " + attrs.get("diagnostic", "")[-1400:]
            (evidence / ("wire-" + case + "-incomplete.json")).write_text(
                json.dumps({"guest": name, "diagnostic": why}, indent=2)
            )
            raise RuntimeError(why)
        return attrs

    ca_sent = activated = False
    phase = None
    deadline = time.monotonic() + 1450
    while time.monotonic() < deadline:
        worker, client = attributes(renderer), attributes(browser)
        current = (worker.get("phase"), client.get("phase"), activated)
        if current != phase:
            print("Regional renderer and browser", *current, flush=True)
            phase = current
        if worker.get("wire-ca") and not ca_sent:
            ca = work / "wire-ca.pem"
            ca.write_text(worker["wire-ca"] + "\n")
            subprocess.run(["openssl", "x509", "-in", str(ca), "-noout"], check=True)
            gcloud(
                "compute",
                "instances",
                "add-metadata",
                browser,
                "--zone=" + ZONE,
                "--metadata-from-file=worker-ca=" + str(ca),
            )
            ca_sent = True
        if ca_sent and client.get("ready") == "true" and not activated:
            gcloud(
                "compute",
                "instances",
                "add-metadata",
                renderer,
                "--zone=" + ZONE,
                "--metadata=client-ready=true",
            )
            activated = True
        if "result" in worker and "result" in client:
            break
        time.sleep(5)
    else:
        raise TimeoutError("No matching result from both independent VMs")
    served, viewed = json.loads(worker["result"]), json.loads(client["result"])
    isolated = served["wire"]
    if (
        not activated
        or served["businessCredentials"]
        or served["publicIngress"]
        or any(isolated["observer"].values())
        or isolated["runtime"] != "lingxi-closed"
        or isolated["network"] != "none"
        or viewed["seconds"] < 600
        or viewed["distinctDecodedCanvases"] != 120
        or viewed["finalQty"] != 128
        or isolated["active"]["frames"] != viewed["trustedClicks"]
        or isolated["active"]["pngBytes"] != viewed["pngPayloadBytes"]
        or isolated["active"]["peer"] != peer
    ):
        raise ValueError("Private browser and renderer measurements disagree")
    return {
        "project": PROJECT,
        "zone": ZONE,
        "renderer": renderer,
        "browser": browser,
        "instanceIds": ids,
        "lifetimesSeconds": lifetimes,
        "sources": sources,
        "fixtureSha256": digest,
        "gvisor": served["gvisor"],
        "renderingImageId": served["image"],
        "observedExternalRequests": isolated["observer"],
        "positiveControl": served["measurements"][0]["observer"],
        "reader": viewed,
        "rendererTransfer": isolated["active"],
        "publicIngress": False,
        "budgetMet": viewed["inputThroughBrowserPaintMs"]["p95"] <= 250,
    }


if __name__ == "__main__":
    cli = argparse.ArgumentParser()
    cli.add_argument("--evidence", type=Path, required=True)
    args = cli.parse_args()
    with tempfile.TemporaryDirectory(
        prefix="dataset-wire-"
    ) as tmp, ExitStack() as stack:
        result = run(args.evidence, Path(tmp), stack)
    # Charge the entire disposable renderer lifecycle to this one 10-minute synthetic session.
    machine_hour = 0.06701142 + 20 * 0.000136986 + 0.005
    times = result["lifetimesSeconds"]
    external_scenario = (
        result["reader"]["ipTransport"]["inbound"]["bytes"] * 0.085 / (1024**3)
    )
    renderer_cost = times[result["renderer"]] * machine_hour / 3600
    result["costScenarioUsd"] = {
        "rendererComputeDiskIpIncludingIdle": round(renderer_cost, 6),
        "equivalentInternetEgressAtStandardListPrice": round(external_scenario, 6),
        "rendererPlusEquivalentEgress": round(renderer_cost + external_scenario, 6),
        "testBrowserMachine": round(times[result["browser"]] * machine_hour / 3600, 6),
        "basis": "Same-zone private HTTPS. Egress scenario prices measured inbound IP bytes on the browser at the Standard internet list rate before allowances.",
    }
    result["budgetMet"] &= renderer_cost + external_scenario <= 0.05
    result["temporaryInfrastructureRemoved"] = True
    (args.evidence / "regional-browser-render.json").write_text(
        json.dumps(result, indent=2) + "\n"
    )
    print(
        json.dumps(
            {
                "budgetMet": result["budgetMet"],
                "p95": result["reader"]["inputThroughBrowserPaintMs"]["p95"],
                "chargedRendererScenario": result["costScenarioUsd"][
                    "rendererPlusEquivalentEgress"
                ],
            }
        )
    )
    raise SystemExit(0 if result["budgetMet"] else 2)
