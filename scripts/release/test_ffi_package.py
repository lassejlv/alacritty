import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("ffi_package", Path(__file__).with_name("ffi-package.py"))
ffi = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ffi)


class FfiPackageTests(unittest.TestCase):
    def test_versions_come_from_the_crate(self):
        self.assertRegex(ffi.crate_version(), r"^\d+\.\d+\.\d+")
        self.assertGreaterEqual(ffi.abi_version(), 1)

    def test_native_static_libs(self):
        output = "warning: x\nnote: native-static-libs: -liconv -lSystem -lc -lm\n"
        self.assertEqual(ffi.native_static_libs(output), "-liconv -lSystem -lc -lm")
        self.assertEqual(ffi.native_static_libs(""), "")

    def test_pkg_config_is_relocatable(self):
        pc = ffi.pkg_config("0.1.0", "linux", "-lc")
        self.assertIn("prefix=${pcfiledir}/../..", pc)
        self.assertIn("Version: 0.1.0", pc)
        self.assertIn("Libs: -L${libdir} -lalacritty_ffi -Wl,-rpath,${libdir}", pc)
        self.assertIn("Libs.private: -lc", pc)
        self.assertNotIn("rpath", ffi.pkg_config("0.1.0", "windows", ""))

    def test_targets(self):
        self.assertEqual(ffi.target_system("aarch64-apple-darwin"), "macos")
        self.assertEqual(ffi.target_system("x86_64-pc-windows-msvc"), "windows")
        self.assertEqual(ffi.target_system("x86_64-unknown-linux-gnu"), "linux")

    def test_layout_uses_versioned_shared_libraries(self):
        for system, built, installed in [
            ("linux", "libalacritty_ffi.so", "lib/libalacritty_ffi.so.3"),
            ("macos", "libalacritty_ffi.dylib", "lib/libalacritty_ffi.3.dylib"),
            ("windows", "alacritty_ffi.dll", "bin/alacritty_ffi.dll"),
        ]:
            with self.subTest(system=system), tempfile.TemporaryDirectory() as directory:
                build = Path(directory) / "build"
                build.mkdir()
                (build / built).write_text("shared")
                for name in ffi.static_libraries(system):
                    (build / name).write_text("static")

                out = ffi.package(build, Path(directory) / "out", system, "1.2.3", 3, "-lm")
                self.assertTrue((out / "include/alacritty.h").is_file())
                self.assertEqual((out / installed).read_text(), "shared")
                for name in ffi.static_libraries(system):
                    self.assertEqual((out / "lib" / name).read_text(), "static")
                for link in ffi.shared_library(system, 3)[2]:
                    self.assertEqual((out / "lib" / link).read_text(), "shared")
                self.assertIn("Version: 1.2.3", (out / "lib/pkgconfig/alacritty_ffi.pc").read_text())


if __name__ == "__main__":
    unittest.main()
