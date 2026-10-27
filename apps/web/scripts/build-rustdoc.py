#!/usr/bin/env python3
"""Build Rustdoc from committed SDK snapshots and their declared dependencies.

Unavailable profiles keep an explicit reason and no URL. Their absence is never
represented by a successful link. Only public/rustdoc is replaced, not source.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "examples"))
from sdk_tools import SdkSnapshot, WEB, cargo_args, cargo_target_lock, command

PROFILES = [
    {"id": "harness-engine", "repo": "harness-runtime", "packages": ["harness-runtime"], "features": "engine", "crate": "harness_runtime", "label": {"en": "Harness · engine", "zh": "Harness · 引擎"}},
    {"id": "harness-desktop", "repo": "harness-runtime", "packages": ["harness-runtime", "client", "tool-api", "media-api", "media-runtime", "tool-media"], "features": "harness-runtime/desktop", "crate": "harness_runtime", "label": {"en": "Harness · desktop", "zh": "Harness · 桌面"}},
    {"id": "harness-mobile-uniffi", "repo": "harness-runtime", "packages": ["harness-runtime", "client"], "features": "harness-runtime/mobile,harness-runtime/uniffi", "crate": "harness_runtime", "label": {"en": "Harness · mobile + UniFFI", "zh": "Harness · 移动端与 UniFFI"}},
    {"id": "llm-core", "repo": "llm-client", "packages": ["lingxi-llm-client"], "features": "", "crate": "lingxi_llm_client", "label": {"en": "LLM · core", "zh": "LLM · 核心"}},
    {"id": "llm-websocket", "repo": "llm-client", "packages": ["lingxi-llm-client"], "features": "responses-websocket,realtime-websocket", "crate": "lingxi_llm_client", "label": {"en": "LLM · WebSocket", "zh": "LLM · WebSocket"}},
    {"id": "llm-tokenizers", "repo": "llm-client", "packages": ["lingxi-llm-client"], "features": "tokenizers-all", "crate": "lingxi_llm_client", "label": {"en": "LLM · all tokenizers", "zh": "LLM · 全部分词器"}},
    {"id": "mobile-api", "repo": "mobile-linux-runtime", "packages": ["mobile-linux-api"], "features": "", "crate": "mobile_linux_api", "label": {"en": "Mobile Linux · API", "zh": "Mobile Linux · 接口"}},
    {"id": "mobile-core", "repo": "mobile-linux-runtime", "packages": ["mobile-linux-core"], "features": "", "crate": "mobile_linux_core", "label": {"en": "Mobile Linux · rootfs core", "zh": "Mobile Linux · rootfs 核心"}},
    {"id": "mobile-ffi", "repo": "mobile-linux-runtime", "packages": ["mobile-linux-ffi"], "features": "", "crate": "mobile_linux_runtime", "label": {"en": "Mobile Linux · host-supported FFI", "zh": "Mobile Linux · 宿主可编译 FFI"}},
    {"id": "mobile-pty", "repo": "mobile-linux-runtime", "packages": ["platform-pty"], "features": "", "crate": "platform_pty", "label": {"en": "Mobile Linux · host PTY", "zh": "Mobile Linux · 宿主 PTY"}},
]


def safe_build_reason(error: Exception | str, snapshot: Path, target: Path) -> str:
    return str(error).replace(str(snapshot), "<snapshot>").replace(str(target), "<cargo-target>").replace(str(Path.home()), "<home>")


def artifact_fingerprint(directory: Path) -> str:
    """Bind a profile to every generated file, independent of workspace drift."""
    if not directory.is_dir() or directory.is_symlink():
        raise RuntimeError(f"Rustdoc profile directory is missing or invalid: {directory}")
    files = []
    for file in directory.rglob("*"):
        if file.is_symlink():
            raise RuntimeError(f"Rustdoc artifacts cannot contain symlinks: {file}")
        if file.is_file():
            files.append(file)
    if not files:
        raise RuntimeError(f"Rustdoc profile is empty: {directory}")
    digest = hashlib.sha256()
    for file in sorted(files, key=lambda file: file.relative_to(directory).as_posix().encode("utf-8")):
        digest.update(file.relative_to(directory).as_posix().encode("utf-8") + b"\0")
        digest.update(hashlib.sha256(file.read_bytes()).digest())
    return "sha256:" + digest.hexdigest()


def retained_profile(saved: dict | None, output: Path) -> dict | None:
    """Retain a verified historical build without claiming it was rebuilt."""
    if not saved or saved.get("state") != "ready" or not saved.get("artifactFingerprint") or not saved.get("sourceFingerprint"):
        return None
    profile_id = saved.get("id", "")
    if not profile_id or not all(char.isalnum() or char == "-" for char in profile_id):
        return None
    url = saved.get("url", "")
    if not url.startswith(f"/rustdoc/{profile_id}/") or ".." in Path(url).parts:
        return None
    directory = output / profile_id
    landing = output.parent / url.lstrip("/")
    if not landing.is_file() or not landing.stat().st_size:
        return None
    try:
        return saved if artifact_fingerprint(directory) == saved["artifactFingerprint"] else None
    except (RuntimeError, OSError):
        return None


def build_profile(snapshot: SdkSnapshot, profile: dict, offline: bool, target: Path, output: Path) -> dict:
    manifest = snapshot.root / profile["repo"] / "Cargo.toml"
    feature_args = ["--no-default-features"]
    if profile["features"]:
        feature_args += ["--features", profile["features"]]
    identities = snapshot.verify_metadata(manifest, offline=offline, feature_args=feature_args, owner_repo=profile["repo"])
    # A private doc directory avoids broken crate-picker/search links left by
    # unrelated earlier profiles. Host dependency artifacts share the existing
    # debug cache through a symlink; Cargo still owns its normal file locks.
    profile_target = target / "handbook-rustdoc" / profile["id"]
    profile_target.mkdir(parents=True, exist_ok=True)
    (target / "debug").mkdir(parents=True, exist_ok=True)
    if not (profile_target / "debug").exists():
        (profile_target / "debug").symlink_to((target / "debug").resolve(), target_is_directory=True)
    if (profile_target / "doc").exists():
        shutil.rmtree(profile_target / "doc")
    args = cargo_args("doc", manifest, offline=offline, target=profile_target) + ["--locked", "--no-deps", "--lib"] + feature_args
    for package in profile["packages"]:
        args += ["-p", package]
    # Shared Cargo cache keeps dependencies inexpensive; copy only this profile's
    # actual output after successful generation and verifying its landing page.
    command(args, cwd=manifest.parent, timeout=2400)
    rustdoc = profile_target / "doc"
    landing = rustdoc / profile["crate"] / "index.html"
    if not landing.is_file():
        raise RuntimeError(f"Cargo succeeded but Rustdoc landing page is absent: {landing}")
    destination = output / profile["id"]
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(rustdoc, destination)
    compiler = command(["rustc", "-vV"], cwd=manifest.parent).stdout
    host = next((line.removeprefix("host: ") for line in compiler.splitlines() if line.startswith("host: ")), "unknown")
    return {"id": profile["id"], "label": profile["label"], "state": "ready",
            "url": f"/rustdoc/{profile['id']}/{profile['crate']}/index.html",
            "packages": profile["packages"], "features": profile["features"].split(",") if profile["features"] else [],
            "identities": identities, "compiler": compiler.splitlines()[0], "hostTarget": host,
            "sourceFingerprint": snapshot.source_fingerprint,
            "sourceRevision": snapshot.source_revisions[profile["repo"]],
            "sourceMode": snapshot.source_mode, "dependencyMode": snapshot.dependency_mode,
            "artifactFingerprint": artifact_fingerprint(destination)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=WEB.parents[2])
    parser.add_argument("--only", help="comma-separated profile ids; retain other profiles only when their saved artifact fingerprints match")
    parser.add_argument("--online", action="store_true", help="allow fetching; offline is default")
    parser.add_argument("--offline", action="store_true", help="explicit default, useful for CI")
    parser.add_argument("--target-dir", type=Path)
    parser.add_argument("--list", action="store_true")
    args = parser.parse_args()
    if args.list:
        print(json.dumps(PROFILES, indent=2))
        return 0
    selected = set(args.only.split(",")) if args.only else {profile["id"] for profile in PROFILES}
    if selected - {profile["id"] for profile in PROFILES}:
        raise RuntimeError("Unknown profile id")
    target = args.target_dir or args.workspace / "harness-runtime" / "target"
    output = WEB / "public" / "rustdoc"
    manifest_path = WEB / "public" / "rustdoc-manifest.json"
    previous = {}
    if manifest_path.exists():
        previous_data = json.loads(manifest_path.read_text())
        previous = {profile["id"]: profile for profile in previous_data.get("profiles", [])}
    lock = cargo_target_lock(target)
    lock.__enter__()
    snapshot = SdkSnapshot(args.workspace, source_mode="committed", dependency_mode="declared")
    results = []
    try:
        for profile in PROFILES:
            if profile["id"] not in selected:
                saved = retained_profile(previous.get(profile["id"]), output)
                if saved:
                    results.append(saved)
                else:
                    if (output / profile["id"]).exists():
                        shutil.rmtree(output / profile["id"])
                    results.append({"id": profile["id"], "label": profile["label"], "state": "unavailable", "reason": "Profile was not selected for this build."})
                continue
            print(f"Building real Rustdoc profile {profile['id']}", flush=True)
            try:
                result = build_profile(snapshot, profile, not args.online, target, output)
                results.append(result)
                print(f"READY {profile['id']}: {result['url']}", flush=True)
            except (RuntimeError, OSError) as error:
                if (output / profile["id"]).exists():
                    shutil.rmtree(output / profile["id"])
                results.append({"id": profile["id"], "label": profile["label"], "state": "unavailable", "reason": safe_build_reason(error, snapshot.root, target)})
                print(f"UNAVAILABLE {profile['id']}: {error}", file=sys.stderr, flush=True)
    finally:
        snapshot.close()
        lock.__exit__(None, None, None)
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(json.dumps({"sourceFingerprint": snapshot.source_fingerprint, "boundary": "Each profile records its own build snapshot and artifact fingerprint. Host Rustdoc compilation; native/device runtime acceptance is separate.", "profiles": results}, ensure_ascii=False, indent=2) + "\n")
    return int(any(profile["state"] == "unavailable" for profile in results if profile["id"] in selected))


if __name__ == "__main__":
    sys.exit(main())
