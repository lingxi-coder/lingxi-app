#!/usr/bin/env python3
"""Export client namespace values from the Cargo-locked branding package."""

import argparse
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import subprocess
import sys
import tempfile

from product_identity import parse_branding_constant
from runtime_source import PACKAGE_LIST, resolve_runtime


CONSTANTS = {
    "envPrefix": "ENV_PREFIX",
    "configDirEnv": "CONFIG_DIR_ENV",
    "dotDir": "DOT_DIR",
    "globalConfigFile": "GLOBAL_CONFIG_FILE",
    "managedDirMacOS": "MANAGED_DIR_MACOS",
    "managedDirWindows": "MANAGED_DIR_WINDOWS",
    "managedDirUnix": "MANAGED_DIR_UNIX",
    "localAppPreparationMarker": "LOCAL_APP_PREPARATION_MARKER",
    "localAppPreparationDataDirectory": "LOCAL_APP_PREPARATION_DATA_DIRECTORY",
    "remoteAttachmentPrefix": "REMOTE_ATTACHMENT_PREFIX",
}


def namespace_from(resolved):
    revision = resolved["revision"]
    repository = json.loads(PACKAGE_LIST.read_text(encoding="utf-8"))["repository"]
    if (resolved["development"] is not False or not re.fullmatch(r"[0-9a-f]{40}", revision)
            or resolved["source"] != f"git+{repository}?rev={revision}#{revision}"):
        raise ValueError("product namespace requires the canonical immutable Git source; local patches are not supported")
    root = Path(resolved["root"]).resolve()
    branding_manifest = Path(resolved["packages"]["branding"]).resolve()
    if branding_manifest != root / "crates/branding/Cargo.toml":
        raise ValueError("product namespace branding package is outside the locked source")
    head = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"], check=True,
                          capture_output=True, text=True, encoding="utf-8").stdout.strip()
    if head != revision:
        raise ValueError("product namespace checkout does not match its locked Git revision")
    committed = subprocess.run(["git", "-C", str(root), "show", f"{revision}:crates/branding/src/lib.rs"],
                               check=True, capture_output=True, text=True, encoding="utf-8").stdout
    if (branding_manifest.parent / "src/lib.rs").read_text(encoding="utf-8") != committed:
        raise ValueError("product namespace branding source differs from its locked Git revision")
    result = {key: parse_branding_constant(name, committed) for key, name in CONSTANTS.items()}
    if any(not value or len(value) > 4096 or "\0" in value for value in result.values()):
        raise ValueError("branding namespace contains an invalid value")
    if not re.fullmatch(r"[A-Z][A-Z0-9_]*_", result["envPrefix"]):
        raise ValueError("branding environment prefix is invalid")
    if not re.fullmatch(r"[A-Z][A-Z0-9_]*", result["configDirEnv"]):
        raise ValueError("branding config directory environment key is invalid")
    for key in ("dotDir", "globalConfigFile", "localAppPreparationMarker",
                "localAppPreparationDataDirectory", "remoteAttachmentPrefix"):
        value = result[key]
        if not value or value in (".", "..") or any(c in value for c in "/\\\0"):
            raise ValueError(f"branding {key} must be a single path component")
    if not PurePosixPath(result["managedDirMacOS"]).is_absolute() or not PurePosixPath(result["managedDirUnix"]).is_absolute():
        raise ValueError("branding Unix managed directory must be absolute")
    if not PureWindowsPath(result["managedDirWindows"]).is_absolute():
        raise ValueError("branding Windows managed directory must be absolute")
    return {"version": 1, "sourceRevision": revision, **result}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="write a generated JSON build resource")
    args = parser.parse_args()
    try:
        result = json.dumps(namespace_from(resolve_runtime()), ensure_ascii=False, indent=2) + "\n"
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            temporary = None
            try:
                with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=args.output.parent,
                                                 prefix=".product-namespace-", delete=False) as file:
                    temporary = Path(file.name)
                    file.write(result)
                os.replace(temporary, args.output)
            finally:
                if temporary is not None:
                    temporary.unlink(missing_ok=True)
        else:
            sys.stdout.write(result)
    except subprocess.CalledProcessError as error:
        print(f"product namespace: locked source resolution or verification failed: {(error.stderr or str(error)).strip()}", file=sys.stderr)
        return 1
    except (KeyError, OSError, ValueError) as error:
        print(f"product namespace: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
