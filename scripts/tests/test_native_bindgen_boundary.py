"""Keep native binding generation independent of the product execution graph."""
import json
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class NativeBindgenBoundaryTests(unittest.TestCase):
    def test_generator_has_no_runtime_dependency_and_matches_both_ffi_versions(self):
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"],
            cwd=ROOT, text=True,
        ))
        packages = {package["id"]: package for package in metadata["packages"]}
        nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
        generator = next(p for p in packages.values() if p["name"] == "native-bindgen")
        pending = [generator["id"]]
        closure = set()
        while pending:
            package_id = pending.pop()
            if package_id not in closure:
                closure.add(package_id)
                pending.extend(nodes[package_id]["dependencies"])
        names = {packages[package_id]["name"] for package_id in closure}
        self.assertFalse(names & {
            "ios-framework", "android-aar", "harness-runtime", "orchestrator",
            "llm-runtime", "lingxi-llm-client",
        }, names)
        bindgen = next(packages[p] for p in closure if packages[p]["name"] == "uniffi_bindgen")
        for name in ("ios-framework", "android-aar"):
            ffi = next(p for p in packages.values() if p["name"] == name)
            scaffolding = next(
                packages[p] for p in nodes[ffi["id"]]["dependencies"]
                if packages[p]["name"] == "uniffi"
            )
            self.assertEqual(bindgen["version"], scaffolding["version"], name)


if __name__ == "__main__":
    unittest.main()
