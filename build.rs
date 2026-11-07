//! Builds libghostty-vt from the Ghostty source in vendor/ghostty (the
//! vt-1.0.0 tag, a git submodule) with Zig and links it in statically.
//! Its C API, read into Rust, is kept in src/ghostty_sys.rs. Also tells
//! the build which commit it is of, for `jterm --version`.

use std::path::PathBuf;
use std::process::Command;

use vergen_gitcl::{Emitter, Gitcl};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let git = Gitcl::builder().sha(true).dirty(false).build();
    Emitter::default().add_instructions(&git)?.emit()?;

    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let ghostty = root.join("vendor/ghostty");
    if !ghostty.join("build.zig").exists() {
        panic!("vendor/ghostty is empty: run `git submodule update --init`");
    }
    println!("cargo:rerun-if-changed=vendor/ghostty/include");
    println!("cargo:rerun-if-changed=vendor/ghostty/src");
    println!("cargo:rerun-if-changed=vendor/ghostty/build.zig.zon");

    // Zig's cache is kept under target/, shared by debug and release, so
    // the library is compiled once (about two minutes) and not again until
    // the submodule moves.
    let target_dir = root.join("target");
    let prefix = target_dir.join("ghostty-vt");
    let status = Command::new("zig")
        .args(["build", "-Demit-lib-vt=true", "-Doptimize=ReleaseFast", "-Dapp-runtime=none", "-Demit-xcframework=false"])
        .arg("--prefix")
        .arg(&prefix)
        .arg("--cache-dir")
        .arg(target_dir.join("zig-cache"))
        .current_dir(&ghostty)
        .status()
        .map_err(|e| format!("zig, needed to build libghostty-vt, did not run: {e}"))?;
    if !status.success() {
        panic!("zig build of libghostty-vt failed");
    }

    println!("cargo:rustc-link-search=native={}", prefix.join("lib").display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");

    #[cfg(feature = "bindgen")]
    bindings(&prefix)?;
    Ok(())
}

/// The C API read into Rust, for src/ghostty_sys.rs: only with
/// `--features bindgen`, which needs libclang.
#[cfg(feature = "bindgen")]
fn bindings(prefix: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let bindings = bindgen::Builder::default()
        .header(prefix.join("include/ghostty/vt.h").to_string_lossy())
        .clang_arg(format!("-I{}", prefix.join("include").display()))
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .layout_tests(false)
        .generate()
        .map_err(|e| format!("bindgen on vt.h: {e}"))?;
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    bindings.write_to_file(out.join("ghostty.rs"))?;
    Ok(())
}
