#!/usr/bin/env python3
"""Reject divergent identities and local overrides for the extracted runtime."""

from runtime_source import resolve_runtime
from mobile_linux_source import resolve_sdk


def main():
    resolved = resolve_runtime()
    print(f"runtime-dependency: {len(resolved['packages'])} packages at {resolved['revision']}")
    sdk = resolve_sdk()
    overlap = set(resolved["packages"]) & set(sdk["packages"])
    if overlap:
        raise ValueError(f"runtime ownership overlaps: {sorted(overlap)}")
    print(f"mobile-linux-dependency: {len(sdk['packages'])} packages at {sdk['revision']}")


if __name__ == "__main__":
    main()
