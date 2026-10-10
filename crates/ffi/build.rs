//! Defines the C ABI version and versions the shared library's soname or install name.

/// Major version of the C ABI, incremented on incompatible changes.
const ABI_VERSION: u32 = 1;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=ALAC_ABI_VERSION={ABI_VERSION}");

    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match os.as_str() {
        "macos" | "ios" => println!(
            "cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libalacritty_ffi.{ABI_VERSION}.\
             dylib"
        ),
        "linux" | "android" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" => {
            println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libalacritty_ffi.so.{ABI_VERSION}")
        },
        _ => (),
    }
}
