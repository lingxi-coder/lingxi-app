#!/usr/bin/env python3
"""Reject divergent identities and local overrides for the extracted runtime."""

from runtime_source import resolve_runtime


def main():
    resolved = resolve_runtime()
    print(f"runtime-dependency: {len(resolved['packages'])} packages at {resolved['revision']}")


if __name__ == "__main__":
    main()
