"""Keep Local App FGS policy exceptions exact instead of exempting its manifest."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("special_use", ROOT / "scripts/mobile-linux/check-android-special-use.py")
POLICY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(POLICY)


class LocalAppForegroundPolicyTests(unittest.TestCase):
    def validate(self, mutate=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / POLICY.MAIN
            path.parent.mkdir(parents=True)
            tree = ET.parse(ROOT / POLICY.MAIN)
            if mutate:
                mutate(tree.getroot())
            tree.write(path)
            POLICY.validate_manifest(path, root, "play")

    def local(self, root):
        return next(node for node in root.findall("application/service")
                    if node.get(POLICY.ANDROID + "name") == POLICY.LOCAL_APP)

    def test_current_declarations_pass(self):
        self.validate()

    def test_local_app_cannot_be_exported(self):
        with self.assertRaises(ValueError):
            self.validate(lambda root: self.local(root).set(POLICY.ANDROID + "exported", "true"))

    def test_local_app_cannot_claim_other_subtype(self):
        with self.assertRaises(ValueError):
            self.validate(lambda root: self.local(root).find("property").set(POLICY.ANDROID + "value", "unreviewed"))

    def test_local_app_cannot_add_an_intent_filter(self):
        with self.assertRaises(ValueError):
            self.validate(lambda root: ET.SubElement(self.local(root), "intent-filter"))

    def test_extra_special_use_service_is_rejected(self):
        def mutate(root):
            ET.SubElement(root.find("application"), "service", {
                POLICY.ANDROID + "name": ".apps.Unreviewed",
                POLICY.ANDROID + "foregroundServiceType": "specialUse",
            })
        with self.assertRaises(ValueError):
            self.validate(mutate)


if __name__ == "__main__":
    unittest.main()
