"""Trusted product identities for the release policy gates.

Sources are relative to this module's Host checkout, never the scanned
--repo-root. Native IDs use the existing literal Gradle/XcodeGen config forms;
unsupported or ambiguous forms fail closed. Rust names come from the branding
package validated by the canonical locked Cargo resolver. Release env keys
come from the actual Host authorization/smoke entrypoints.
"""
from functools import lru_cache
import json
from pathlib import Path
import re

from runtime_source import resolve_runtime

HOST = Path(__file__).resolve().parents[2]


def literal(pattern, source, label):
    matches = re.findall(pattern, source, re.M)
    if len(matches) != 1 or not matches[0]:
        raise ValueError(f"trusted {label}: expected one literal definition")
    return matches[0]


def native_id(value, label):
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+", value):
        raise ValueError(f"trusted {label}: invalid native identity")
    return value


@lru_cache(maxsize=1)
def android_package():
    source = (HOST / "apps/android/native/app/build.gradle.kts").read_text()
    namespace = literal(r'^\s*namespace = "([^"\n]+)"\s*$', source, "Android namespace")
    application = literal(r'^\s*applicationId = "([^"\n]+)"\s*$', source, "Android applicationId")
    if namespace != application:
        raise ValueError("trusted Android namespace/applicationId contract diverged")
    return native_id(namespace, "Android namespace")


@lru_cache(maxsize=1)
def ios_bundle_id():
    source = (HOST / "apps/ios/native/project.yml").read_text()
    # Read only the main application's settings.base, excluding flavor,
    # widget and test identities. These anchors intentionally require the
    # checked-in XcodeGen block/scalar shape instead of guessing YAML values.
    target = literal(r"^name: ([A-Za-z][A-Za-z0-9_]*)$", source, "XcodeGen project name")
    targets = literal(r"^targets:\n([\s\S]*?)(?=^\S|\Z)", source, "XcodeGen targets")
    application = literal(r"^  " + re.escape(target) + r":\n([\s\S]*?)(?=^  \S|\Z)",
                          targets, "main application target")
    if literal(r"^    type: ([^\n]+)$", application, "main target type") != "application":
        raise ValueError("trusted XcodeGen main target is not an application")
    base = literal(r"^    settings:\n      base:\n([\s\S]*?)(?=^ {0,6}\S|\Z)",
                   application, "main application settings.base")
    return native_id(literal(r"^        PRODUCT_BUNDLE_IDENTIFIER: ([A-Za-z0-9_.]+)$",
                             base, "main application bundle ID"), "iOS bundle ID")


@lru_cache(maxsize=1)
def locked_branding_source():
    resolved = resolve_runtime()
    return Path(resolved["packages"]["branding"]).parent / "src/lib.rs"


def branding_constant(name, resolved=None):
    source = (Path(resolved["packages"]["branding"]).parent / "src/lib.rs"
              if resolved is not None else locked_branding_source())
    value = literal(r"^pub const " + re.escape(name) + r': &str = ("[^"\n]*");$',
                    source.read_text(), f"branding::{name}")
    return json.loads(value)


@lru_cache(maxsize=1)
def authorization_enabled_env():
    return literal(r'^enabled="\$\{([A-Z][A-Z0-9_]*):-0\}"$',
                   (HOST / "scripts/mobile-linux/check-authorizations.sh").read_text(),
                   "authorization enabled environment key")
