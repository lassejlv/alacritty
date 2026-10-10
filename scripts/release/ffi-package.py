"""Build alacritty_ffi and lay out headers, libraries and a pkg-config file for distribution.

Usage: ffi-package.py [--target TRIPLE] [--out DIR]

The output contains include/alacritty.h, lib/ with the static and shared libraries under their
versioned names, and lib/pkgconfig/alacritty_ffi.pc, which is relocatable.
"""
import argparse
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / "crates/ffi"


def crate_version():
    manifest = (CRATE / "Cargo.toml").read_text()
    return re.search(r'^version = "([^"]+)"', manifest, re.MULTILINE).group(1)


def abi_version():
    build = (CRATE / "build.rs").read_text()
    return int(re.search(r"const ABI_VERSION: u32 = (\d+);", build).group(1))


def native_static_libs(output):
    """Linker flags rustc reported for the static library."""
    match = re.search(r"native-static-libs: (.*)", output)
    return match.group(1).strip() if match else ""


def shared_library(system, abi):
    """Built shared library, its installed name and development symlinks."""
    if system == "macos":
        return "libalacritty_ffi.dylib", f"libalacritty_ffi.{abi}.dylib", ["libalacritty_ffi.dylib"]
    if system == "windows":
        return "alacritty_ffi.dll", "alacritty_ffi.dll", []
    return "libalacritty_ffi.so", f"libalacritty_ffi.so.{abi}", ["libalacritty_ffi.so"]


def static_libraries(system):
    if system == "windows":
        return ["alacritty_ffi.lib", "alacritty_ffi.dll.lib"]
    return ["libalacritty_ffi.a"]


def pkg_config(version, system, static_libs):
    libs = "-L${libdir} -lalacritty_ffi"
    if system != "windows":
        libs += " -Wl,-rpath,${libdir}"
    return "\n".join([
        "prefix=${pcfiledir}/../..",
        "includedir=${prefix}/include",
        "libdir=${prefix}/lib",
        "",
        "Name: alacritty_ffi",
        "Description: Alacritty terminal engine C API",
        f"Version: {version}",
        "Cflags: -I${includedir}",
        f"Libs: {libs}",
        f"Libs.private: {static_libs}",
        "",
    ])


def package(build_dir, out, system, version, abi, static_libs):
    """Copy built artifacts from `build_dir` into the distribution layout at `out`."""
    build_dir, out = Path(build_dir), Path(out)
    if out.exists():
        shutil.rmtree(out)
    (out / "include").mkdir(parents=True)
    (out / "lib/pkgconfig").mkdir(parents=True)
    shutil.copy2(CRATE / "include/alacritty.h", out / "include")

    for name in static_libraries(system):
        if (build_dir / name).exists():
            shutil.copy2(build_dir / name, out / "lib" / name)

    built, installed, links = shared_library(system, abi)
    destination = out / ("bin" if system == "windows" else "lib")
    destination.mkdir(exist_ok=True)
    shutil.copy2(build_dir / built, destination / installed)
    for link in links:
        (out / "lib" / link).symlink_to(installed)

    (out / "lib/pkgconfig/alacritty_ffi.pc").write_text(pkg_config(version, system, static_libs))
    return out


def target_system(target):
    if target is None:
        return {"darwin": "macos", "win32": "windows"}.get(sys.platform, "linux")
    if "apple" in target:
        return "macos"
    return "windows" if "windows" in target else "linux"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--target", help="Rust target triple")
    parser.add_argument("--out", help="output directory")
    args = parser.parse_args()

    command = ["cargo", "rustc", "--locked", "--release", "-p", "alacritty_ffi", "--lib"]
    if args.target:
        command += ["--target", args.target]
    result = subprocess.run(command + ["--", "--print", "native-static-libs"], cwd=ROOT,
                            stderr=subprocess.PIPE, text=True)
    sys.stderr.write(result.stderr)
    if result.returncode != 0:
        sys.exit(result.returncode)

    build_dir = ROOT / "target" / (args.target or "") / "release"
    out = Path(args.out) if args.out else build_dir / "alacritty_ffi"
    system = target_system(args.target)
    package(build_dir, out, system, crate_version(), abi_version(),
            native_static_libs(result.stderr))
    print(out)


if __name__ == "__main__":
    main()
