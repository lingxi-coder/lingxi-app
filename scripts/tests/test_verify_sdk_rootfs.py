#!/usr/bin/env python3
"""Equivalent workspace paths must never widen the image's writable scope."""
import importlib.util
from pathlib import Path
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "lib/verify_sdk_rootfs.py"
spec = importlib.util.spec_from_file_location("verify_sdk_rootfs", SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class WritableScopeTests(unittest.TestCase):
    def setUp(self):
        self.roots = ["/home/lingxi", "/root", "/tmp", "/var/tmp"]

    def test_workspace_move_has_the_same_effective_scope(self):
        before = self.roots + ["/home/lingxi/.lingxi"]
        after = self.roots + ["/home/lingxi/.lingxi-workspaces"]
        self.assertEqual(module.writable_scope(before), module.writable_scope(after))
        self.assertEqual(module.writable_scope(before), set(self.roots))

    def test_parent_or_sibling_expansion_is_rejected(self):
        for extra in ("/", "/home", "/home/lingxi-other", "/etc"):
            with self.subTest(extra=extra):
                self.assertNotEqual(module.writable_scope(self.roots + [extra]), set(self.roots))

    def test_missing_root_is_not_equivalent(self):
        self.assertNotEqual(module.writable_scope(self.roots[1:]), set(self.roots))

    def test_child_cannot_replace_writable_home(self):
        self.assertNotEqual(module.writable_scope(self.roots[1:] + ["/home/lingxi/.lingxi"]), set(self.roots))

    def test_invalid_paths_are_rejected(self):
        for paths in (None, [], {}, [1], ["home/lingxi"], ["/home/lingxi/../root"],
                      ["/home//lingxi"], ["/home/lingxi/"], self.roots + ["/root"]):
            with self.subTest(paths=paths), self.assertRaises(ValueError):
                module.writable_scope(paths)


if __name__ == "__main__":
    unittest.main()
