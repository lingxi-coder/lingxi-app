#!/usr/bin/env python3
"""Verify product policies and SDK integration without owning native sources."""
import argparse
import json
from pathlib import Path


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_host_policy(repo):
    policy = json.loads((repo / "docs/mobile-linux/local-app-native-policy.json").read_text())
    require(policy.get("schema_version") == 2, "unsupported host native-policy schema")
    require(policy.get("sdk_integration") == {
        "source_resolver": "lingxi-code/scripts/mobile_linux_source.py",
        "android_artifact_kind": "native-support-only",
        "ios_framework": "MobileLinuxNativeSupport.xcframework",
        "contains_rust_core": False,
    }, "host must consume Cargo-locked native support without a second Rust core")
    android = policy["android_network_policy_launcher"]
    require(android == {
        "artifact": "libmobile_linux_policy_launcher.so",
        "supported_network_policies": ["disabled", "loopback_only"],
        "loopback_only_ready": True,
        "abis": ["arm64-v8a", "x86_64"],
        "variants": ["play", "direct"],
    }, "Android product network-policy contract diverged")
    require(policy["ios_ish_execution_policy"] == {
        "hook_version": 1,
        "supported_network_policies": ["disabled", "loopback_only"],
        "loopback_only_ready": True,
        "runtime_memory_limit_bytes": 800 * 1024 * 1024,
        "watchdog_interval_ms": 250,
        "memory_accounting": "guest_backed_pages_by_execution_context",
        "local_app_build_mount_layout": "single_root_materialized_snapshot",
        "nested_bind_mount_resolution": "longest_guest_prefix",
        "platform": "iphoneos",
    }, "iOS product execution-policy contract diverged")
    for relative in (
        "clients/android/scripts/build-mobile-linux-native.sh",
        "clients/ios/scripts/build-linux-runtime.sh",
    ):
        text = (repo / relative).read_text()
        require("mobile_linux_source.py" in text, f"{relative} must resolve the Cargo-locked SDK")
        require("docs/superpowers/references/OpenMinis" not in text,
                f"{relative} still requires host-owned OpenMinis sources")
    project = (repo / "clients/ios/project.yml").read_text()
    require("Frameworks/MobileLinuxNativeSupport.xcframework" in project,
            "iOS must link the SDK native-support framework")
    require("Frameworks/MobileLinuxRuntime.xcframework" not in project,
            "iOS must not link full SDK Rust FFI beside LingxiCodeFFI")
    require("- LinuxRuntimeNative/**" in project,
            "iOS must not compile a second native implementation from the old source directory")
    require("-lish_emu" not in project and "references/OpenMinis" not in project,
            "iOS must link SDK native support rather than host iSH archives")
    gradle = (repo / "clients/android/app/build.gradle.kts").read_text()
    require("useLegacyPackaging = true" in gradle,
            "Android native executables must be extracted into nativeLibraryDir")
    require("verify-mobile-linux-apk.py" in gradle,
            "Android must validate native payloads in the final APK")
    for relative in (
        "clients/ios/Sources/Conversation/ConversationSource.swift",
        "clients/ios/Sources/Cron/CronFFIBridge.swift",
    ):
        text = (repo / relative).read_text()
        require("localAppsRuntimeRoot: LocalAppsRuntimeDistribution.runtimeRoot" in text,
                f"{relative} must pass the bundled local-app runtime root")
        require("localAppsRuntimeRoot: nil" not in text,
                f"{relative} discards the bundled local-app runtime root")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        validate_host_policy(args.repo_root.resolve())
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"local-app-host: {error}\n")
    print("LingXi local-app policy and SDK integration verified")


if __name__ == "__main__":
    main()
