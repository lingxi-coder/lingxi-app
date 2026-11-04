#!/usr/bin/env python3
"""Check the reviewed, immutable third-party cargo-vet attestations.

The pinned manifests describe other people's attestations; this check neither
performs a source audit nor grants publisher-wide trust. It constrains the three
curated imports; legacy peers retain their existing lock data, including any
publisher or wildcard records, which this guard does not certify or eliminate.
Updating the pins or adding an import requires a new review of the sources and
their provenance.
"""
from collections import Counter
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib


SNAPSHOT_COMMIT = "f5ab635ad9b093ded46f53b744ea71bf695b21dc"
SNAPSHOT_DIR = Path("supply-chain/curated-imports/2026-11-03")
PROVENANCE_SHA256 = "d168f68a29035f4d4896e0cade13cf198c753cfcfb879239f8ff18edec122228"
SNAPSHOT_HASHES = {
    "embark-studios.toml": "bce8784ffdf4a50724f040acfb6bac8b2149a6c106bb0f2e2bc516beaf13ca11",
    "isrg.toml": "b1da7a859c732a6f59d682db4020087a0666bee7e71736613712f8968f39dd48",
    "zcash.toml": "ae9c4047791a18b1bac77ef8c8d96a9ae2ef020670d39542d99641ad1aab6bf4",
}
RECORD_COUNTS = {"embark-studios.toml": 3, "isrg.toml": 6, "zcash.toml": 4}
CURATED_IMPORTS = {
    "curated-embark-studios": "embark-studios.toml",
    "curated-isrg": "isrg.toml",
    "curated-zcash": "zcash.toml",
}
EXISTING_IMPORTS = {
    "bytecode-alliance": "https://raw.githubusercontent.com/bytecodealliance/wasmtime/main/supply-chain/audits.toml",
    "google": "https://raw.githubusercontent.com/google/supply-chain/main/audits.toml",
    "mozilla": "https://raw.githubusercontent.com/mozilla/supply-chain/main/audits.toml",
}
RAW_REPOSITORY = "https://raw.githubusercontent.com/lingxi-coder/lingxi-app"
VERSION = r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:[-+][0-9A-Za-z.-]+)?"


class ValidationError(ValueError):
    """A curated import does not match the reviewed policy."""


@dataclass(frozen=True)
class Anchors:
    commit: str
    provenance_sha256: str
    snapshot_hashes: dict[str, str]


DEFAULT_ANCHORS = Anchors(SNAPSHOT_COMMIT, PROVENANCE_SHA256, SNAPSHOT_HASHES)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValidationError(message)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def read_file(path: Path) -> bytes:
    require(path.is_file() and not path.is_symlink(), f"missing or symlinked file: {path}")
    return path.read_bytes()


def unique_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def record_key(peer: str, crate: str, record: dict) -> tuple[str, str, str]:
    return peer, crate, json.dumps(record, sort_keys=True, separators=(",", ":"))


def validate_record(peer: str, crate: str, record: object) -> dict:
    label = f"{peer}/{crate}"
    require(isinstance(record, dict), f"{label}: audit must be a table")
    require(re.fullmatch(r"[A-Za-z0-9_-]+", crate) is not None, f"{label}: invalid crate name")
    allowed = {"who", "criteria", "version", "delta", "violation", "notes", "aggregated-from"}
    require(set(record) <= allowed, f"{label}: unexpected audit field")
    require(record.get("criteria") == "safe-to-deploy", f"{label}: only explicit safe-to-deploy is allowed")
    require(isinstance(record.get("who"), str) and bool(record["who"].strip()), f"{label}: missing auditor")
    selectors = [field for field in ("version", "delta", "violation") if field in record]
    require(len(selectors) == 1, f"{label}: expected exactly one version, delta, or violation")
    selector = selectors[0]
    value = record[selector]
    require(isinstance(value, str) and bool(value), f"{label}: invalid {selector}")
    if selector == "version":
        require(re.fullmatch(VERSION, value) is not None, f"{label}: invalid exact version")
    elif selector == "delta":
        require(re.fullmatch(f"{VERSION} -> {VERSION}", value) is not None, f"{label}: invalid exact delta")
    for field in ("notes", "aggregated-from"):
        require(field not in record or isinstance(record[field], str), f"{label}: invalid {field}")
    return record


def validate_snapshots(root: Path, anchors: Anchors) -> dict[str, Counter]:
    curated_root = root / "supply-chain/curated-imports"
    require(curated_root.is_dir() and not curated_root.is_symlink(), "curated-imports directory is missing or symlinked")
    for path in curated_root.rglob("*"):
        require(not path.is_symlink(), f"symlink in curated imports: {path}")
    discovered = {path.relative_to(root) for path in curated_root.rglob("*") if path.is_file() and path.suffix.lower() == ".toml"}
    expected_paths = {SNAPSHOT_DIR / name for name in SNAPSHOT_HASHES}
    require(discovered == expected_paths, "curated TOML files differ from the reviewed file set")
    require(set(anchors.snapshot_hashes) == set(SNAPSHOT_HASHES), "incomplete snapshot trust anchors")

    directory = root / SNAPSHOT_DIR
    provenance_bytes = read_file(directory / "provenance.json")
    require(digest(provenance_bytes) == anchors.provenance_sha256, "provenance SHA-256 mismatch")
    provenance = json.loads(provenance_bytes, object_pairs_hook=unique_object)
    require(isinstance(provenance, dict), "provenance must be an object")
    files = provenance.get("files")
    entries = provenance.get("records")
    require(isinstance(files, dict) and set(files) == set(SNAPSHOT_HASHES), "provenance file set mismatch")
    require(isinstance(entries, list) and len(entries) == 13, "provenance must contain exactly 13 records")

    snapshots: dict[str, Counter] = {}
    for name in SNAPSHOT_HASHES:
        peer = name.removesuffix(".toml")
        content = read_file(directory / name)
        actual_hash = digest(content)
        require(actual_hash == anchors.snapshot_hashes[name], f"{name}: snapshot SHA-256 mismatch")
        require(files[name] == {"sha256": actual_hash, "records": RECORD_COUNTS[name]}, f"{name}: provenance file metadata mismatch")
        document = tomllib.loads(content.decode("utf-8"))
        require(set(document) == {"audits"} and isinstance(document["audits"], dict), f"{name}: only the audits table is allowed")
        observed: Counter = Counter()
        for crate, rows in document["audits"].items():
            require(isinstance(rows, list) and bool(rows), f"{peer}/{crate}: expected audit array")
            for record in rows:
                observed[record_key(peer, crate, validate_record(peer, crate, record))] += 1
        require(sum(observed.values()) == RECORD_COUNTS[name], f"{name}: incorrect record count")
        require(all(count == 1 for count in observed.values()), f"{name}: duplicate audit record")
        snapshots[peer] = observed

    from_provenance = Counter()
    violation_count = 0
    for entry in entries:
        require(isinstance(entry, dict), "provenance record must be an object")
        peer, crate, record = entry.get("peer"), entry.get("crate"), entry.get("record")
        require(peer in snapshots and isinstance(crate, str), "provenance has an unknown peer or crate")
        validate_record(peer, crate, record)
        from_provenance[record_key(peer, crate, record)] += 1
        violation_count += "violation" in record
    expected_records = sum(snapshots.values(), Counter())
    require(from_provenance == expected_records, "snapshot audit fields differ from reviewed provenance")
    require(violation_count == 1 and sum(expected_records.values()) == 13, "expected 12 attestations and one violation")
    return snapshots


def safe_criteria(value: object) -> bool:
    return value == "safe-to-deploy" or value == ["safe-to-deploy"]


def validate_config(root: Path, anchors: Anchors) -> None:
    require(re.fullmatch(r"[0-9a-f]{40}", anchors.commit) is not None, "snapshot commit must be a full Git SHA")
    config = tomllib.loads(read_file(root / "supply-chain/config.toml").decode("utf-8"))
    require(set(config) <= {"cargo-vet", "imports", "policy", "default-criteria"}, "unsupported config section or exemption")
    if "default-criteria" in config:
        require(safe_criteria(config["default-criteria"]), "default criterion cannot be lowered")
    imports = config.get("imports")
    require(isinstance(imports, dict), "missing imports table")
    require(set(imports) == set(EXISTING_IMPORTS) | set(CURATED_IMPORTS), "unexpected or missing import peer")
    for peer, expected_url in EXISTING_IMPORTS.items():
        require(imports[peer] == {"url": expected_url}, f"{peer}: existing import changed or uses a criteria map")
    for peer, name in CURATED_IMPORTS.items():
        url = f"{RAW_REPOSITORY}/{anchors.commit}/{SNAPSHOT_DIR.as_posix()}/{name}"
        require(imports[peer] == {"url": url}, f"{peer}: expected the pinned immutable snapshot URL only")
    policies = config.get("policy", {})
    require(isinstance(policies, dict), "policy must be a table")
    for name, policy in policies.items():
        require(isinstance(policy, dict), f"{name}: policy must be a table")
        require(set(policy) <= {"audit-as-crates-io", "criteria", "dependency-criteria", "notes"}, f"{name}: unsupported policy field")
        if "audit-as-crates-io" in policy:
            require(policy["audit-as-crates-io"] is True, f"{name}: audit-as-crates-io cannot be disabled")
        if "criteria" in policy:
            require(safe_criteria(policy["criteria"]), f"{name}: criteria cannot be lowered")
        if "dependency-criteria" in policy:
            criteria = policy["dependency-criteria"]
            require(isinstance(criteria, dict) and all(safe_criteria(v) for v in criteria.values()), f"{name}: dependency criteria cannot be lowered")


def validate_lock(root: Path, snapshots: dict[str, Counter]) -> None:
    lock = tomllib.loads(read_file(root / "supply-chain/imports.lock").decode("utf-8"))
    peers = lock.get("audits", {})
    require(isinstance(peers, dict), "imports.lock audits must be a table")
    require(set(peers) <= set(EXISTING_IMPORTS) | set(CURATED_IMPORTS), "imports.lock has an unknown peer")
    cargo_lock = tomllib.loads(read_file(root / "Cargo.lock").decode("utf-8"))
    packages = cargo_lock.get("package", [])
    require(isinstance(packages, list), "Cargo.lock package entries must be an array")
    package_names = {package.get("name") for package in packages if isinstance(package, dict)}
    for alias, name in CURATED_IMPORTS.items():
        peer = name.removesuffix(".toml")
        section = peers.get(alias, {})
        require(isinstance(section, dict) and set(section) <= {"audits"}, f"{alias}: wildcard, trust, or unexpected lock section")
        audits = section.get("audits", {})
        require(isinstance(audits, dict), f"{alias}: lock audits must be a table")
        locked = Counter()
        for crate, rows in audits.items():
            require(isinstance(rows, list), f"{alias}/{crate}: lock audits must be an array")
            for record in rows:
                locked[record_key(peer, crate, validate_record(peer, crate, record))] += 1
        require(not (locked - snapshots[peer]), f"{alias}: lock contains an unreviewed or duplicate record")
        # A negative audit cannot be silently lost when that crate enters the
        # locked dependency set. This intentionally treats even optional
        # Cargo.lock packages conservatively until the import is regenerated.
        for key in snapshots[peer]:
            _source_peer, crate, encoded = key
            if crate in package_names and "violation" in json.loads(encoded):
                require(locked[key] == 1, f"{alias}/{crate}: applicable violation missing from imports.lock")


def validate(root: Path, anchors: Anchors = DEFAULT_ANCHORS) -> None:
    snapshots = validate_snapshots(root, anchors)
    validate_config(root, anchors)
    validate_lock(root, snapshots)


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    try:
        validate(root)
    except (ValidationError, OSError, UnicodeError, json.JSONDecodeError, tomllib.TOMLDecodeError, TypeError) as error:
        print(f"curated audit validation failed: {error}", file=sys.stderr)
        return 1
    print("curated audit snapshots, provenance, immutable imports and lock verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
