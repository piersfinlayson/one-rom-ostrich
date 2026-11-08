//! Builds the plugin natively and archives it with the host shim.
//!
//! The plugin objects come from the plugin's own Makefile (`make host`), so
//! the flags that must match the firmware's native test build are defined
//! beside the device build.  The shim is compiled here with the same flags.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

/// The One ROM checkout, relative to this crate.  Cargo.toml's path
/// dependencies point at the same place.
const ONEROM_DIR: &str = "../build/one-rom";

const PLUGIN_OBJS: &[&str] = &["ostrich_main.o", "datalog.o", "flash_write.o"];

/// Must match the plugin Makefile's HOST_CFLAGS and firmware/test.mk.  The
/// objects link together, so -fshort-enums has to agree across all of them.
const SHIM_FLAGS: &[&str] = &[
    "-DORA_HOST_TEST=1",
    "-fshort-enums",
    "-std=c11",
    "-O1",
    "-g",
    "-Wall",
    "-Wextra",
    "-Werror",
];

/// A value the plugin Makefile defines, as `NAME := value`.
fn makefile_value(plugin_dir: &Path, name: &str) -> String {
    let makefile = std::fs::read_to_string(plugin_dir.join("Makefile"))
        .expect("could not read the plugin Makefile");
    for line in makefile.lines() {
        if let Some(rest) = line.strip_prefix(name)
            && let Some(value) = rest.trim_start().strip_prefix(":=")
        {
            return value.trim().to_string();
        }
    }
    panic!("{name} is not defined in the plugin Makefile");
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let plugin_dir = manifest_dir
        .parent()
        .expect("missing repository root")
        .to_path_buf();
    let onerom_dir = std::fs::canonicalize(manifest_dir.join(ONEROM_DIR)).unwrap_or_else(|e| {
        panic!("One ROM checkout not found at {ONEROM_DIR}, which make one-rom creates: {e}")
    });
    let firmware = onerom_dir.join("firmware");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!(
        "cargo:rerun-if-changed={}",
        plugin_dir.join("src").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        plugin_dir.join("Makefile").display()
    );
    println!("cargo:rerun-if-changed={}", firmware.join("ora").display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("csrc").display()
    );
    println!("cargo:rerun-if-env-changed=HOST_CC");

    // Plugin objects
    let host_build_dir = out_dir.join("plugin");
    let status = Command::new("make")
        .arg("-C")
        .arg(&plugin_dir)
        .arg("host")
        .arg(format!("HOST_BUILD_DIR={}", host_build_dir.display()))
        .status()
        .expect("could not run make");
    assert!(
        status.success(),
        "make host failed in {}",
        plugin_dir.display()
    );
    let plugin_objs: Vec<PathBuf> = PLUGIN_OBJS.iter().map(|o| host_build_dir.join(o)).collect();

    // Shim object
    let shim_obj = out_dir.join("ostrich_shim.o");
    let cc = env::var("HOST_CC").unwrap_or_else(|_| "cc".to_string());
    let status = Command::new(&cc)
        .args(SHIM_FLAGS)
        .arg(format!(
            "-DFLASH_WRITE_FN_MAX={}u",
            makefile_value(&plugin_dir, "FLASH_WRITE_FN_MAX")
        ))
        .arg(format!("-I{}", firmware.join("ora").display()))
        .arg(format!("-I{}", plugin_dir.join("src").display()))
        .arg(format!("-I{}", manifest_dir.join("csrc").display()))
        .arg("-c")
        .arg(manifest_dir.join("csrc/ostrich_shim.c"))
        .arg("-o")
        .arg(&shim_obj)
        .status()
        .expect("could not run the host C compiler");
    assert!(status.success(), "compiling ostrich_shim.c failed");

    // Archive.  The macOS ar produces an archive rustc can't use here, so
    // libtool is used there, as firmware/test.mk does.
    let archive = out_dir.join("libonerom-ostrich-host.a");
    let _ = std::fs::remove_file(&archive);
    let status = if cfg!(target_os = "macos") {
        Command::new("libtool")
            .arg("-static")
            .arg("-o")
            .arg(&archive)
            .args(&plugin_objs)
            .arg(&shim_obj)
            .status()
    } else {
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args(&plugin_objs)
            .arg(&shim_obj)
            .status()
    }
    .expect("could not run the archiver");
    assert!(status.success(), "archiving the plugin objects failed");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=onerom-ostrich-host");

    // The plugin and shim call into the firmware test library, so it follows
    // this archive on the link line.  onerom-fw-emulator builds it.
    println!(
        "cargo:rustc-link-search=native={}",
        firmware.join("build-test").display()
    );
    println!("cargo:rustc-link-lib=static=onerom-test");
}
