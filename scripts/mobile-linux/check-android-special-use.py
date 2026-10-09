#!/usr/bin/env python3
"""Validate the reviewed FGS declarations, not Google Play approval.

specialUse is a normal FGS permission, not a device-control privilege:
https://developer.android.com/develop/background-work/services/fgs/service-types#special-use
https://support.google.com/googleplay/android-developer/answer/13392821
https://support.google.com/googleplay/android-developer/answer/16559646

ConversationBackgroundExecution starts a user turn from the visible Activity,
posts an ongoing notification with private cancellation, and releases the lease
on terminal state (including child tasks). Redelivery restores that same turn.
This source gate limits declarations and constant locations; it does not prove
runtime lifecycle behavior or replace release device tests / Play declarations.
"""
import argparse
from pathlib import Path
import re
import subprocess
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from product_identity import android_package, branding_constant

ANDROID = "{http://schemas.android.com/apk/res/android}"
TOOLS = "{http://schemas.android.com/tools}"
PERMISSION = "android.permission.FOREGROUND_SERVICE_SPECIAL_USE"
SUBTYPE = "android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE"
CONVERSATION = ".conversation.ConversationTurnService"
CONTROL = ".computeruse.ComputerUseSessionService"
MAIN = Path("apps/android/native/app/src/main/AndroidManifest.xml")
DIRECT = Path("apps/android/native/app/src/direct/AndroidManifest.xml")
TOKEN = re.compile(r"FOREGROUND_SERVICE_(?:TYPE_)?SPECIAL_USE|PROPERTY_SPECIAL_USE_FGS_SUBTYPE|specialUse")
CONSTANT = "ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE"


def reject(path, message):
    raise ValueError(f"{path}: {message}")


def service_name(name):
    # Fully qualified spelling must not bypass overlay/duplicate checks.
    prefix = android_package()
    return name[len(prefix):] if name.startswith(prefix + ".") else name


def validate_manifest(path, repo_root, mode):
    tree = ET.parse(path).getroot()
    if tree.tag != "manifest":
        reject(path, "expected manifest XML")
    parents = {child: parent for parent in tree.iter() for child in parent}
    relative = path.relative_to(repo_root) if path.is_relative_to(repo_root) else None
    expected = {}
    if relative == MAIN:
        expected[CONVERSATION] = ("specialUse", "User-started LLM inference and agent tool execution")
    if mode == "direct" and relative == DIRECT:
        expected[CONTROL] = ("mediaProjection|microphone|specialUse", "User-started Android Computer Use control session")
    seen = set()
    allowed = set()
    permissions = []
    for node in tree.iter():
        name = node.get(ANDROID + "name", "")
        if node.tag == "service":
            if "mediaPlayback" in node.get(ANDROID + "foregroundServiceType", "").split("|"):
                reject(path, "forbidden mediaPlayback foreground service")
            canonical = service_name(name)
            if canonical in (CONVERSATION, CONTROL) or TOKEN.search(node.get(ANDROID + "foregroundServiceType", "")):
                if canonical not in expected or canonical in seen:
                    reject(path, "unreviewed or duplicate specialUse service / manifest override")
                kind, subtype = expected[canonical]
                attributes = {ANDROID + "name": name, ANDROID + "exported": "false",
                              ANDROID + "foregroundServiceType": kind, ANDROID + "stopWithTask": "false"}
                if node.attrib != attributes or parents.get(node) is not tree.find("application"):
                    reject(path, "specialUse service must match the non-exported conversation/control lifecycle declaration")
                properties = list(node)
                if len(properties) != 1 or properties[0].tag != "property" or properties[0].attrib != {
                    ANDROID + "name": SUBTYPE, ANDROID + "value": subtype
                } or len(properties[0]):
                    reject(path, "specialUse service requires the exact reviewed subtype and no intent filters")
                seen.add(canonical)
                allowed.update((node, properties[0]))
        if TOKEN.search(name) and node.tag.startswith("uses-permission"):
            if not expected or node.tag != "uses-permission" or node.attrib != {ANDROID + "name": PERMISSION} or parents.get(node) is not tree:
                reject(path, "unreviewed specialUse permission")
            permissions.append(node)
            allowed.add(node)
    if seen != set(expected) or len(permissions) != (1 if expected else 0):
        reject(path, "missing reviewed specialUse service or permission pair")
    if expected:
        if tree.get("package") not in (None, android_package()):
            reject(path, "unreviewed service package")
        base = [node for node in tree.findall("uses-permission")
                if node.attrib == {ANDROID + "name": "android.permission.FOREGROUND_SERVICE"}]
        if len(base) != 1:
            reject(path, "missing normal FOREGROUND_SERVICE permission")
    for node in tree.iter():
        if node not in allowed and any(TOKEN.search(value) for value in node.attrib.values()):
            reject(path, "specialUse declaration outside the reviewed service / permission")
        # Merger directives on ancestors can silently remove/replace the service.
        if node.tag in ("manifest", "application") and any(key.startswith(TOOLS) for key in node.attrib):
            reject(path, "manifest merger directive requires policy review")
    if mode == "direct" and relative == DIRECT:
        accessibility = ".computeruse." + branding_constant("PRODUCT_NAME") + "AccessibilityService"
        bindings = [node for node in tree.findall("application/service")
                    if service_name(node.get(ANDROID + "name", "")) == accessibility]
        if len(bindings) != 1 or bindings[0].get(ANDROID + "permission") != "android.permission.BIND_ACCESSIBILITY_SERVICE":
            reject(path, "Direct manifest is missing AccessibilityService binding")


def validate_sources(repo_root, mode, scan_paths):
    # Match the shell scan's exclusions; do not exempt an entire manifest/file.
    command = ["rg", "--hidden", "-0"]
    for glob in ("!docs/superpowers/references/**", "!**/build/**", "!**/.build/**", "!**/DerivedData/**", "!**/.git/**"):
        command += ["--glob", glob]
    if mode == "play":
        command += ["--glob", "!**/src/direct/**"]
    result = subprocess.run(command + ["--files"] + list(map(str, scan_paths)), capture_output=True, check=True)
    paths = [Path(item.decode()) for item in result.stdout.split(b"\0") if item]
    manifests = {path for path in paths if path.name == "AndroidManifest.xml"}
    required = {repo_root / MAIN} | ({repo_root / DIRECT} if mode == "direct" else set())
    if not required <= manifests:
        raise ValueError("missing reviewed Android manifest")
    for path in sorted(manifests):
        validate_manifest(path, repo_root, mode)
    package_path = Path(*android_package().split("."))
    constant_paths = {repo_root / "apps/android/native/app/src/main/java" / package_path
                      / "conversation/ConversationBackgroundExecution.kt"}
    if mode == "direct":
        constant_paths.add(repo_root / "apps/android/native/app/src/direct/java" / package_path
                           / "computeruse/ComputerUseSessionService.kt")
    candidates = subprocess.run(command + ["-l", "-e", TOKEN.pattern] + list(map(str, scan_paths)),
                                capture_output=True)
    if candidates.returncode not in (0, 1):
        raise ValueError(f"specialUse source scan failed: {candidates.stderr.decode(errors='replace')}")
    for item in candidates.stdout.split(b"\0"):
        if not item:
            continue
        path = Path(item.decode())
        if path in manifests:
            continue
        content = path.read_bytes().decode("utf-8", errors="replace")
        if path in constant_paths and content.count(CONSTANT) == 1:
            content = content.replace(CONSTANT, "", 1)
        if TOKEN.search(content):
            reject(path, "unreviewed specialUse source reference")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("play", "direct"), required=True)
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("scan_paths", type=Path, nargs="+")
    args = parser.parse_args()
    try:
        validate_sources(args.repo_root, args.mode, args.scan_paths)
    except (ValueError, OSError, ET.ParseError, subprocess.CalledProcessError) as error:
        print(f"forbidden Android specialUse policy: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
