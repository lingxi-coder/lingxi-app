#!/usr/bin/env python3
"""Invoke the supported tool from the Local App source this product pins."""
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "lib"))
from local_app_delegate import run

raise SystemExit(run("verify-local-app-supply-chain.py", sys.argv[1:]))
