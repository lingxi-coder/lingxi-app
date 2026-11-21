"""Measure trusted browser input through the displayed frame, across a real regional network."""

import ipaddress
import json
import math
import os
import ssl
import time
from urllib.error import URLError
from urllib.request import urlopen

from selenium import webdriver
from selenium.webdriver.chrome.service import Service
from selenium.webdriver.common.by import By
from selenium.webdriver.support.ui import WebDriverWait

from wire_worker import PORT, metadata


def run():
    peer = str(ipaddress.IPv4Address(metadata("attributes/worker-ip")))
    assert ipaddress.ip_address(peer).is_private
    origin = f"https://{peer}:{PORT}"
    opts = webdriver.ChromeOptions()
    opts.binary_location = "/usr/bin/chromium"
    for flag in (
        "--headless",
        "--no-sandbox",
        "--disable-dev-shm-usage",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-sync",
        "--no-first-run",
        "--window-size=800,900",
    ):
        opts.add_argument(flag)
    context = ssl.create_default_context(cafile="/tmp/reader/ca.crt")
    with webdriver.Chrome(
        service=Service("/usr/bin/chromedriver"), options=opts
    ) as browser:
        metadata("guest-attributes/render/ready", b"true")
        deadline = time.monotonic() + 100
        while time.monotonic() < deadline:
            try:
                with urlopen(
                    origin + "/viewer", context=context, timeout=2
                ) as response:
                    if response.status == 200:
                        break
            except (OSError, URLError):
                time.sleep(0.3)
        else:
            raise TimeoutError("The isolated screen did not start")
        browser.get(origin + "/viewer")
        button = browser.find_element(By.TAG_NAME, "button")
        assert button.accessible_name == "数量加一"
        ready = WebDriverWait(browser, 8, poll_frequency=0.04)
        start = time.monotonic()
        for n in range(1, 121):
            time.sleep(max(0, start + (n - 1) * 5 - time.monotonic()))
            button.click()
            ready.until(
                lambda b: b.execute_script(
                    "return wire.position === arguments[0] || Boolean(wire.error)", n
                )
            )
            error = browser.execute_script("return wire.error")
            if error:
                raise RuntimeError(error)
        time.sleep(max(0, start + 600 - time.monotonic()))
        elapsed = time.monotonic() - start
        values = browser.execute_script(
            "return {samples:wire.samples,transfer:wire.transfer,frames:wire.frames.size,pixels:wire.pixels.size,quantity:document.querySelector('output').value}"
        )
        assert (
            values["frames"] == values["pixels"] == len(values["samples"]) == 120
            and values["quantity"] == "128"
        )
        done = browser.execute_async_script(
            """const cb=arguments[0]; fetch('/finish', {method:'POST',
          headers:{'Content-Type':'application/json'},body:JSON.stringify({frames:120})}).then(r=>cb(r.status),()=>cb(0))"""
        )
        assert done == 200
        samples = sorted(values["samples"])
        return {
            "browser": browser.capabilities["browserVersion"],
            "seconds": round(elapsed, 3),
            "trustedClicks": len(samples),
            "distinctDecodedCanvases": values["pixels"],
            "finalQty": int(values["quantity"]),
            "inputThroughBrowserPaintMs": {
                "min": round(samples[0], 2),
                "p50": round(samples[59], 2),
                "p95": round(samples[math.ceil(0.95 * len(samples)) - 1], 2),
                "max": round(samples[-1], 2),
                "samples": values["samples"],
            },
            "pngPayloadBytes": sum(row["payload"] for row in values["transfer"]),
            "resourceTimingTransferBytes": sum(
                row["browserTransfer"] for row in values["transfer"]
            ),
        }


if __name__ == "__main__":
    os.environ["NO_PROXY"] = "*"
    print(json.dumps(run()), flush=True)
