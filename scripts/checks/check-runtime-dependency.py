#!/usr/bin/env python3
"""Reject divergent identities and local overrides for the extracted runtime."""

import itertools
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from runtime_source import resolve_runtime
from mobile_linux_source import resolve_sdk
from local_app_source import resolve_local_app


def main():
    resolved = resolve_runtime()
    print(f"runtime-dependency: {len(resolved['packages'])} packages at {resolved['revision']}")
    sdk = resolve_sdk()
    local_app = resolve_local_app()
    owners = {"runtime": resolved, "mobile-linux": sdk, "local-app": local_app}
    for (left, a), (right, b) in itertools.combinations(owners.items(), 2):
        overlap = set(a["packages"]) & set(b["packages"])
        if overlap:
            raise ValueError(f"{left} and {right} both claim: {sorted(overlap)}")
    print(f"mobile-linux-dependency: {len(sdk['packages'])} packages at {sdk['revision']}")
    print(f"local-app-dependency: {len(local_app['packages'])} packages at {local_app['revision']}")


if __name__ == "__main__":
    main()
