#!/usr/bin/env python3
"""Regenerate (or verify) the TypeScript bridge envelopes from the Cargo source."""
import argparse
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from bridge_protocol_generator import check_or_write, run_tests
from runtime_source import resolve_runtime

parser = argparse.ArgumentParser(description=__doc__)
group = parser.add_mutually_exclusive_group()
group.add_argument("--check", action="store_true", help="fail if checked-in output is stale")
group.add_argument("--test", action="store_true", help="run the isolated generator regression tests")
args = parser.parse_args()
if args.test:
    run_tests()
else:
    check_or_write(Path(resolve_runtime()["root"]), check=args.check)
