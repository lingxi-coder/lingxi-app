#!/usr/bin/env python3
"""Validate the product's reviewed iOS background modes and source contracts.

Apple 2.5.4 requires background APIs to serve their intended purposes:
https://developer.apple.com/app-store/review/guidelines/#software-requirements
https://developer.apple.com/documentation/backgroundtasks/bgprocessingtask
https://developer.apple.com/documentation/avfaudio/avaudiosession/category-swift.struct/playback

AVAudioSession.playback is also used for foreground TTS. Background audio needs
an audio UIBackgroundModes declaration, which this product does not authorize.
Source anchors check the reviewed registration/cleanup contract; runtime tests
and App Review remain separate release requirements.
"""
import argparse
from pathlib import Path
import plistlib
import re
import subprocess
import sys
from xml.parsers.expat import ExpatError

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from local_app_branding import ios_bundle_id

INFO = Path("apps/ios/native/Info.plist")
SOURCES = Path("apps/ios/native/Sources")


def reviewed_identifiers():
    prefix = ios_bundle_id()
    return {prefix + ".cron.reconcile", prefix + ".localapps.background",
            prefix + ".conversation.continued.*"}


class UniqueKeys(dict):
    def __setitem__(self, key, value):
        if key in self:
            raise ValueError(f"duplicate plist key: {key}")
        super().__setitem__(key, value)


def code_without_comments(text):
    # Preserve quoted strings, strip line comments and nested block comments.
    # These are source-contract anchors, not a full Swift semantic parser.
    result = []
    index = 0
    while index < len(text):
        if text.startswith("//", index):
            end = text.find("\n", index)
            index = len(text) if end == -1 else end
        elif text.startswith("/*", index):
            depth = 1
            index += 2
            while index < len(text) and depth:
                if text.startswith("/*", index):
                    depth += 1
                    index += 2
                elif text.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            result.append(" ")
        elif text[index] == '"':
            delimiter = '"""' if text.startswith('"""', index) else '"'
            start = index
            index += len(delimiter)
            while index < len(text):
                if text[index] == "\\":
                    index += 2
                elif text.startswith(delimiter, index):
                    index += len(delimiter)
                    break
                else:
                    index += 1
            result.append(text[start:index])
        else:
            result.append(text[index])
            index += 1
    return "".join(result)


def function_body(source, name):
    match = re.search(r"func\s+" + re.escape(name) + r"\(\)\s*\{", source)
    if not match:
        return ""
    depth = 1
    start = index = match.end()
    while index < len(source):
        if source[index] == '"':
            index += 1
            while index < len(source):
                if source[index] == "\\":
                    index += 2
                elif source[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            continue
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start:index]
        index += 1
    return ""


def validate_processing_sources(repo_root):
    # Limit the exception to the actual declared task owners and their reviewed
    # registration, expiration and completion paths. Comments cannot supply an
    # absent identifier or lifecycle anchor.
    prefix = re.escape(ios_bundle_id())
    contracts = {
        "Cron/CronModels.swift": [r'let\s+cronBackgroundTaskIdentifier\s*=\s*"' + prefix + r'\.cron\.reconcile"'],
        "Cron/CronSystemAdapters.swift": [r'BGProcessingTaskRequest\(identifier:\s*taskIdentifier\)',
            r'BGTaskScheduler\.shared\.register\(forTaskWithIdentifier:\s*identifier',
            r'processingTask\.expirationHandler\s*=\s*\{\s*worker\.cancel\(\)',
            r'task\?\.setTaskCompleted\(success:\s*success\)'],
        "App/LocalAppBackgroundTaskBridge.swift": [r'let\s+localAppBackgroundTaskIdentifier\s*=\s*"' + prefix + r'\.localapps\.background"',
            r'BGTaskScheduler\.shared\.register\(\s*forTaskWithIdentifier:\s*localAppBackgroundTaskIdentifier',
            r'BGProcessingTaskRequest\(identifier:\s*localAppBackgroundTaskIdentifier\)',
            r'processing\.expirationHandler\s*=\s*\{\s*worker\.cancel\(\)',
            r'processing\.setTaskCompleted\(success:\s*!Task\.isCancelled\)'],
        "App/AppNotificationDelegate.swift": [r'cronBackgroundBridge\.registerAtLaunch\(\s*taskIdentifier:\s*cronBackgroundTaskIdentifier',
            r'LocalAppBackgroundTaskBridge\.shared\.registerAtLaunch\(\)'],
        "App/ConversationBackgroundActivity.swift": [r'let\s+conversationContinuedProcessingIdentifier\s*=\s*"' + prefix + r'\.conversation\.continued"',
            r'static\s+let\s+wildcard\s*=\s*"\\\(conversationContinuedProcessingIdentifier\)\.\*"',
            r'BGTaskScheduler\.shared\.register\(\s*forTaskWithIdentifier:\s*identifier',
            r'BGContinuedProcessingTaskRequest\(\s*identifier:\s*identifier',
            r'task\.expirationHandler\s*=\s*\{', r'lease\?\.task\.setTaskCompleted\(success:\s*false\)',
            r'lease\?\.task\.setTaskCompleted\(success:\s*success\)'],
        "Voice/VoiceAudioSessionCoordinator.swift": [r'func\s+suspendForBackground\(\)\s*\{'],
        "App/RootView.swift": [r'case\s+\.background:[\s\S]*?VoiceAudioSessionCoordinator\.shared\.suspendForBackground\(\)'],
    }
    for relative, patterns in contracts.items():
        path = repo_root / SOURCES / relative
        source = code_without_comments(path.read_text())
        if relative == "Voice/VoiceAudioSessionCoordinator.swift":
            if "sessionDriver.deactivate()" not in function_body(source, "suspendForBackground"):
                raise ValueError(f"{path}: missing foreground audio deactivation on background entry")
        for pattern in patterns:
            if not re.search(pattern, source):
                raise ValueError(f"{path}: missing reviewed processing identifier / lifecycle source contract: {pattern}")


def validate_plist(path, repo_root):
    with path.open("rb") as stream:
        info = plistlib.load(stream, dict_type=UniqueKeys)
    if not isinstance(info, dict):
        raise ValueError(f"{path}: expected plist dictionary")
    # Detect a misplaced declaration too, rather than exempting the whole plist.
    def declarations(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if key in ("UIBackgroundModes", "BGTaskSchedulerPermittedIdentifiers") and value is not info:
                    raise ValueError(f"{path}: misplaced background declaration")
                declarations(item)
        elif isinstance(value, list):
            for item in value:
                declarations(item)
    declarations(info)
    modes = info.get("UIBackgroundModes", [])
    identifiers = info.get("BGTaskSchedulerPermittedIdentifiers", [])
    for label, values in (("UIBackgroundModes", modes), ("BGTaskSchedulerPermittedIdentifiers", identifiers)):
        if not isinstance(values, list) or any(not isinstance(value, str) for value in values) or len(set(values)) != len(values):
            raise ValueError(f"{path}: invalid or duplicate {label}")
    if "audio" in modes:
        raise ValueError(f"{path}: forbidden iOS audio background mode")
    if modes or identifiers or path == repo_root / INFO:
        if path != repo_root / INFO or modes != ["processing"] or set(identifiers) != reviewed_identifiers():
            raise ValueError(f"{path}: unreviewed processing modes / permitted identifiers")
        validate_processing_sources(repo_root)


def validate_sources(repo_root, mode, scan_paths):
    command = ["rg", "--hidden", "-0"]
    for glob in ("!docs/superpowers/references/**", "!**/build/**", "!**/.build/**", "!**/DerivedData/**", "!**/.git/**"):
        command += ["--glob", glob]
    if mode == "play":
        command += ["--glob", "!**/src/direct/**"]
    inventory = subprocess.run(command + ["--files"] + list(map(str, scan_paths)), capture_output=True, check=True)
    paths = [Path(item.decode()) for item in inventory.stdout.split(b"\0") if item]
    plists = {path for path in paths if path.suffix == ".plist"}
    if repo_root / INFO not in plists:
        raise ValueError("missing reviewed iOS Info.plist")
    for path in sorted(plists):
        validate_plist(path, repo_root)
    candidates = subprocess.run(command + ["-l", "-e", r"UIBackgroundModes|BGTaskSchedulerPermittedIdentifiers|numberOfLoops|\.loops"]
                                + list(map(str, scan_paths)), capture_output=True)
    if candidates.returncode not in (0, 1):
        raise ValueError("iOS background source scan failed: " + candidates.stderr.decode(errors="replace"))
    for item in candidates.stdout.split(b"\0"):
        if not item:
            continue
        path = Path(item.decode())
        if path in plists:
            continue
        source = code_without_comments(path.read_text(errors="replace"))
        if re.search(r"UIBackgroundModes|BGTaskSchedulerPermittedIdentifiers", source):
            raise ValueError(f"{path}: unreviewed iOS background declaration outside plist")
        loop = re.search(r"numberOfLoops\s*=\s*-\s*1\b|options:\s*(?:\[\s*)?\.loops\b", source)
        muted = re.search(r"\.volume\s*=\s*0(?:\.0+)?(?![\d.])", source)
        silent = re.search(r'"[^"\n]*(?:silent|silence|idle|keep.?alive)[^"\n]*\.(?:wav|mp3|caf|aiff)"', source, re.I)
        if loop and (muted or silent):
            raise ValueError(f"{path}: forbidden fake idle background audio loop")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("play", "direct"), required=True)
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("scan_paths", type=Path, nargs="+")
    args = parser.parse_args()
    try:
        validate_sources(args.repo_root, args.mode, args.scan_paths)
    except (ValueError, OSError, ExpatError, plistlib.InvalidFileException, subprocess.CalledProcessError) as error:
        print(f"forbidden iOS background policy: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
