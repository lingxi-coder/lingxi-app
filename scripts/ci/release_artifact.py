#!/usr/bin/env python3
"""Keep native release modes and links intact across Actions artifact transfer."""

import argparse
from pathlib import Path
import tarfile


def pack(source: Path, archive: Path) -> None:
    if source.is_symlink() or not source.is_dir():
        raise ValueError(f"release artifact source is not a directory: {source}")
    source = source.resolve()
    archive = archive.resolve()
    if archive.is_relative_to(source):
        raise ValueError("release archive must be outside its source directory")
    archive.parent.mkdir(parents=True, exist_ok=True)
    # Exclusive creation also prevents an existing archive from being silently
    # reused if this step runs twice in an unexpected workspace.
    with archive.open("xb") as output:
        with tarfile.open(fileobj=output, mode="w", format=tarfile.PAX_FORMAT) as tar:
            tar.add(source, arcname=".", recursive=True)


def unpack(archive: Path, destination: Path) -> None:
    if not archive.is_file():
        raise ValueError(f"release artifact archive is missing: {archive}")
    if destination.is_symlink():
        raise ValueError("release artifact destination must not be a symlink")
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError(f"release artifact destination is not empty: {destination}")
    with tarfile.open(archive, mode="r:") as tar:
        # The data filter keeps normal file execute bits and safe in-tree
        # symlinks, while rejecting paths or links that escape the destination.
        tar.extractall(destination, filter="data")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("pack")
    create.add_argument("--source", type=Path, required=True)
    create.add_argument("--archive", type=Path, required=True)
    extract = commands.add_parser("unpack")
    extract.add_argument("--archive", type=Path, required=True)
    extract.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "pack":
            pack(args.source, args.archive)
        else:
            unpack(args.archive, args.destination)
    except (OSError, ValueError, tarfile.TarError) as error:
        parser.exit(1, f"release artifact: {error}\n")


if __name__ == "__main__":
    main()
