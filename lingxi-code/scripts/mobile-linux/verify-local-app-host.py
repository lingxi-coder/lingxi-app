#!/usr/bin/env python3
"""Verify LingXi native launchers and iSH integration against host-owned pins."""
import argparse
import hashlib
import json
import pathlib
import re
import sys

def fail(message):
    raise SystemExit(message)

def valid_sha256(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None

def validate_host_policy(repo):
    policy = json.loads((repo / "docs/mobile-linux/local-app-native-policy.json").read_text())
    launcher = policy.get("android_network_policy_launcher")
    expected_source = "clients/android/app/src/main/cpp/mobile_linux_policy_launcher.c"
    expected_overlay_source = "clients/android/app/src/main/cpp/proot_lingxi_network_policy.c"
    expected_artifact = "libmobile_linux_policy_launcher.so"
    if not isinstance(launcher, dict):
        fail("local-app runtime policy is missing the Android network-policy launcher")
    if (
        launcher.get("source") != expected_source
        or launcher.get("proot_overlay_source") != expected_overlay_source
        or launcher.get("artifact") != expected_artifact
        or launcher.get("supported_network_policies") != ["disabled", "loopback_only"]
        or launcher.get("loopback_only_ready") is not True
        or launcher.get("abis") != ["arm64-v8a", "x86_64"]
        or launcher.get("variants") != ["play", "direct"]
        or not valid_sha256(launcher.get("source_sha256"))
        or not valid_sha256(launcher.get("proot_overlay_source_sha256"))
    ):
        fail("Android network-policy launcher policy diverged")
    source = repo / expected_source
    try:
        actual_source_sha256 = hashlib.sha256(source.read_bytes()).hexdigest()
    except OSError as exc:
        fail(f"missing Android network-policy launcher source: {exc}")
    if actual_source_sha256 != launcher["source_sha256"]:
        fail("Android network-policy launcher source SHA-256 diverged")
    overlay_source = repo / expected_overlay_source
    try:
        actual_overlay_sha256 = hashlib.sha256(overlay_source.read_bytes()).hexdigest()
    except OSError as exc:
        fail(f"missing Android PRoot network-policy overlay source: {exc}")
    if actual_overlay_sha256 != launcher["proot_overlay_source_sha256"]:
        fail("Android PRoot network-policy overlay source SHA-256 diverged")

    build_script = repo / "clients/android/scripts/build-mobile-linux-native.sh"
    verify_script = repo / "clients/android/scripts/verify-mobile-linux-native.sh"
    try:
        build_text = build_script.read_text(encoding="utf-8")
        verify_text = verify_script.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing Android native launcher packaging script: {exc}")
    build_tokens = {
        "app/src/main/cpp/mobile_linux_policy_launcher.c",
        expected_artifact,
        'build_policy_launcher "arm64-v8a"',
        'build_policy_launcher "x86_64"',
    }
    if any(token not in build_text for token in build_tokens):
        fail("Android build script does not package the network-policy launcher for both ABIs")
    if expected_artifact not in verify_text or not all(
        abi in verify_text for abi in launcher["abis"]
    ):
        fail("Android native verifier does not require the network-policy launcher for both ABIs")

    ish_policy = policy.get("ios_ish_execution_policy")
    ish_sources = {
        "registry_source": (
            "registry_source_sha256",
            "clients/ios/Sources/LinuxRuntimeNative/LXISHExecutionPolicy.c",
        ),
        "header_source": (
            "header_source_sha256",
            "clients/ios/Sources/LinuxRuntimeNative/LXISHExecutionPolicy.h",
        ),
        "ish_patch_source": (
            "ish_patch_source_sha256",
            "clients/ios/Sources/LinuxRuntimeNative/patches/ish-socket-network-policy.patch",
        ),
    }
    if not isinstance(ish_policy, dict):
        fail("local-app runtime policy is missing the iOS iSH execution policy")
    if (
        ish_policy.get("hook_version") != 1
        or ish_policy.get("supported_network_policies") != ["disabled", "loopback_only"]
        or ish_policy.get("loopback_only_ready") is not True
        or ish_policy.get("runtime_memory_limit_bytes") != 800 * 1024 * 1024
        or "memory_limit_bytes" in ish_policy
        or ish_policy.get("watchdog_interval_ms") != 250
        or ish_policy.get("memory_accounting")
        != "guest_backed_pages_by_execution_context"
        or ish_policy.get("local_app_build_mount_layout")
        != "single_root_materialized_snapshot"
        or ish_policy.get("nested_bind_mount_resolution") != "longest_guest_prefix"
        or ish_policy.get("platform") != "iphoneos"
    ):
        fail("iOS iSH execution-policy contract diverged")
    for source_key, (digest_key, expected_path) in ish_sources.items():
        digest = ish_policy.get(digest_key)
        if ish_policy.get(source_key) != expected_path or not valid_sha256(digest):
            fail(f"iOS iSH execution-policy source declaration diverged: {source_key}")
        try:
            actual_digest = hashlib.sha256((repo / expected_path).read_bytes()).hexdigest()
        except OSError as exc:
            fail(f"missing iOS iSH execution-policy source: {exc}")
        if actual_digest != digest:
            fail(f"iOS iSH execution-policy source SHA-256 diverged: {expected_path}")

    ish_patch_path = repo / ish_sources["ish_patch_source"][1]
    try:
        ish_patch_text = ish_patch_path.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing iOS iSH execution-policy patch: {exc}")
    nested_bind_tokens = {
        "best_guest_match",
        "best_guest_path_len",
        "g_bind_mounts[best_guest_match].host_path",
    }
    if any(token not in ish_patch_text for token in nested_bind_tokens):
        fail("iOS iSH patch does not make nested bind mounts prefer the longest guest prefix")

    ios_build_script = repo / "clients/ios/scripts/build-linux-runtime.sh"
    try:
        ios_build_text = ios_build_script.read_text(encoding="utf-8")
    except OSError as exc:
        fail(f"missing iOS iSH runtime build script: {exc}")
    required_ish_build_tokens = {
        "ish-socket-network-policy.patch",
        'git -C "${ISH_SOURCE}" apply --unidiff-zero --check "${ISH_NETWORK_POLICY_PATCH}"',
        'git -C "${ISH_SOURCE}" apply --unidiff-zero "${ISH_NETWORK_POLICY_PATCH}"',
        "restore_ish_policy_source",
    }
    if any(token not in ios_build_text for token in required_ish_build_tokens):
        fail("iOS build script does not apply and restore the pinned iSH policy patch")

    # The bundled dependency seed only reaches the engine if the launch config
    # actually carries its path. `LocalAppsRuntimeDistribution.runtimeRoot` was
    # computed and then discarded -- BOTH call sites passed
    # `localAppsRuntimeRoot: nil`, so `configured_runtime_root()` reported the
    # seed as unconfigured and every device installed dependencies over the
    # network with a complete tree sitting in its own bundle. A Rust test
    # cannot see Swift, so assert the Swift spelling here.
    for relative in (
        "clients/ios/Sources/Conversation/ConversationSource.swift",
        "clients/ios/Sources/Cron/CronFFIBridge.swift",
    ):
        source_path = repo / relative
        try:
            source_text = source_path.read_text(encoding="utf-8")
        except OSError as exc:
            fail(f"missing iOS engine launch source: {exc}")
        if "localAppsRuntimeRoot: LocalAppsRuntimeDistribution.runtimeRoot" not in source_text:
            fail(f"{relative} must pass the bundled local-app runtime root to the engine")
        if "localAppsRuntimeRoot: nil" in source_text:
            fail(f"{relative} still discards the bundled local-app runtime root")

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", required=True)
    args = parser.parse_args()
    validate_host_policy(pathlib.Path(args.repo_root).resolve())
    print("LingXi native local-app integration verified")
