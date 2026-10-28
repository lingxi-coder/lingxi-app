#!/usr/bin/env python3
"""Mutate only the disposable hosted checkout to verify regression sensitivity."""
from pathlib import Path
import subprocess
import sys

source = Path("packages/remote-control-client/src/isolated_codec.rs")
before = source.read_bytes()
original = before.decode("utf8")
test_command = [
    "cargo", "test", "--locked", "-p", "remote-control-client", "--features",
    "native-media", "--test", "codec_worker",
]


def mutate(replacements):
    value = original
    for old, new in replacements:
        if value.count(old) != 1:
            raise RuntimeError(f"expected exactly one mutation target: {old!r}")
        value = value.replace(old, new, 1)
    source.write_text(value, encoding="utf8")


def run(command):
    result = subprocess.run(command, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=600)
    print(result.stdout, flush=True)
    return result


def expect_test_failure(test, expected_messages):
    result = run(test_command + [
        test, "--", "--exact", "--test-threads=1", "--nocapture", "--color", "never",
    ])
    if (result.returncode == 0 or "1 failed;" not in result.stdout or
            not any(message in result.stdout for message in expected_messages)):
        raise RuntimeError(f"negative control did not fail for the expected reason: {test}")
    print(f"Negative control detected: {test}", flush=True)


try:
    mutate([(".capture(chunk, chunk_at)", ".capture(chunk, Instant::now())")])
    expect_test_failure(
        "delayed_audio_request_retains_first_sample_timestamps",
        ["queued audio timestamp moved", "the delayed first capture must not be discarded"],
    )
    mutate([("Clock::child_at(sampled, midpoint)",
             "Clock::child_at(sampled + Duration::from_millis(800), midpoint)")])
    expect_test_failure(
        "slow_boot_cannot_hide_a_stale_audio_request",
        ["slow bootstrap must not hide stale capture age"],
    )
    mutate([("bool::from(proof[..32].ct_eq(token.as_slice()))", "true")])
    result = run([
        "cargo", "test", "--locked", "-p", "remote-control-client", "--features",
        "native-media", "--lib",
        "isolated_codec::tests::codec_auth_rejects_wrong_token_and_lane_before_accepting_valid_peer",
        "--", "--exact", "--test-threads=1", "--nocapture", "--color", "never",
    ])
    if (result.returncode == 0 or "1 failed;" not in result.stdout or
            "invalid peer must be closed" not in result.stdout):
        raise RuntimeError("codec token bypass was not detected")
    print("Negative control detected: incorrect codec token accepted", flush=True)
    mutate([
        ('        command.env("GIO_MODULE_DIR", &gio_modules);\n', ""),
        ('        command.env("GST_INSTALL_PLUGINS_HELPER", &disabled_installer);\n', ""),
    ])
    if len(sys.argv) != 2:
        raise RuntimeError("expected the absolute Bridge binary path")
    result = run([sys.executable, "scripts/ci/probe_codec_worker_gio.py",
                  "--bridge", sys.argv[1], "--expect", "unsafe"])
    if result.returncode != 0:
        raise RuntimeError("GIO and installer unsafe-default control failed")
    print("Negative control detected: inherited GIO and installer defaults", flush=True)
finally:
    source.write_bytes(before)
    if source.read_bytes() != before:
        raise RuntimeError("failed to restore candidate source after negative controls")
