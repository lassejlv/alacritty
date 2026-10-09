import importlib.util
from pathlib import Path
import plistlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("macos_version", Path(__file__).with_name("macos-version.py"))
version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(version)


class BundleVersionTests(unittest.TestCase):
    def test_tags(self):
        self.assertEqual(version.release_version("v1.2.3"), "1.2.3")
        self.assertEqual(version.release_version("0.18.0"), "0.18.0")
        for tag in ("latest", "v01.2.3", "v1.2", "v1.2.3-beta.1", "v1.2.3\nOTHER=value"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                version.release_version(tag)

    def test_stamp_before_signing_preserves_update_contract(self):
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory)
            (bundle / "Contents").mkdir()
            path = bundle / "Contents/Info.plist"
            original = {"CFBundleVersion": "1", "CFBundleShortVersionString": "0.18.0-dev",
                        "SUPublicEDKey": "test", "SUFeedURL": "https://example.com/appcast.xml"}
            path.write_bytes(plistlib.dumps(original))
            version.stamp(bundle, "v2.3.4")
            actual = plistlib.loads(path.read_bytes())
            self.assertEqual(actual, dict(original, CFBundleVersion="2.3.4", CFBundleShortVersionString="2.3.4"))
            version.stamp(bundle, "")
            self.assertEqual(plistlib.loads(path.read_bytes())["CFBundleVersion"], "0")


if __name__ == "__main__":
    unittest.main()
