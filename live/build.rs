//! Copies values the harness checks against out of the plugin's source, the
//! plugin API's headers and the One ROM CLI's source into `OUT_DIR`, one file
//! per value, so the harness follows a change to any of them.  `src/consts.rs`
//! reads them with `include!`.

use std::{env, fs, path::PathBuf};

/// The One ROM checkout, relative to this crate.  The tester's build.rs uses
/// the same location.
const ONEROM_DIR: &str = "../build/one-rom";

/// `#define`s in the plugin's `src/ostrich_main.c`.
const PLUGIN_DEFINES: &[&str] = &[
    "CMD_TIMEOUT_MS",
    "REPLY_TIMEOUT_MS",
    "SAVE_QUIET_MS",
    "NV_STORAGE_SIZE",
    "NV_BOOT_SLOT",
    "FLASH_SECTOR_SIZE",
];

/// `#define`s in the plugin's `src/datalog.c`.
const DATALOG_DEFINES: &[&str] = &["DATALOG_BAUD"];

/// String constants in the plugin's `src/ostrich_main.c`.
const PLUGIN_STRINGS: &[&str] = &["datalog_port_desc"];

/// `#define`s in the plugin API's constants header, which the plugin builds
/// against.
const ORA_HEADER: &str = "firmware/ora/onerom_constants_generated.h";
const ORA_DEFINES: &[&str] = &[
    "ORA_FLASH_CS0_BASE_ADDR",
    "ORA_USER_PLUGIN_OFFSET",
    "ORA_USER_PLUGIN_SIZE",
    "ORA_USB_PLUGIN_VID",
    "ORA_USB_PLUGIN_PID",
];

/// `Duration::from_millis` constants in the One ROM CLI, by file.
const CLI_DURATIONS: &[(&str, &str)] = &[
    ("rust/cli/src/lib/usb.rs", "OPEN_RETRY_LIMIT"),
    ("rust/cli/src/lib/ram_log.rs", "POLL"),
];

/// The number in `#define NAME value` in C source, without a cast, brackets
/// or a `u` suffix.
fn c_define(source: &str, name: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        if words.next() != Some("#define") || words.next() != Some(name) {
            return None;
        }
        let value = words.next()?.split(['(', ')']).rfind(|s| !s.is_empty())?;
        Some(value.trim_end_matches(['u', 'U']).to_string())
    })
}

/// The string literal, quotes included, in `const char name[] = "...";`.
fn c_string(source: &str, name: &str) -> Option<String> {
    let prefix = format!("const char {name}[] = ");
    source.lines().find_map(|line| {
        let literal = line.trim_start().strip_prefix(&prefix)?.strip_suffix(';')?;
        (literal.starts_with('"') && literal.ends_with('"')).then(|| literal.to_string())
    })
}

/// The milliseconds in the `Duration::from_millis` constant `name`.
fn rust_millis(source: &str, name: &str) -> Option<String> {
    let prefix = format!("const {name}: Duration = Duration::from_millis(");
    source.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(&prefix)?;
        Some(rest.strip_suffix(");")?.to_string())
    })
}

fn read(path: &PathBuf) -> String {
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read_to_string(path).unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()))
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let write = |name: &str, value: String| {
        fs::write(out_dir.join(name), value).expect("could not write to OUT_DIR");
    };

    let onerom_dir = manifest_dir.join(ONEROM_DIR);
    let sources = [
        (manifest_dir.join("../src/ostrich_main.c"), PLUGIN_DEFINES),
        (manifest_dir.join("../src/datalog.c"), DATALOG_DEFINES),
        (onerom_dir.join(ORA_HEADER), ORA_DEFINES),
    ];
    for (path, names) in sources {
        let source = read(&path);
        for &name in names {
            let value = c_define(&source, name)
                .unwrap_or_else(|| panic!("{name} isn't defined in {}", path.display()));
            write(name, value);
        }
    }

    let path = manifest_dir.join("../src/ostrich_main.c");
    let source = read(&path);
    for &name in PLUGIN_STRINGS {
        let value = c_string(&source, name)
            .unwrap_or_else(|| panic!("{name} isn't a string constant in {}", path.display()));
        write(name, value);
    }

    for &(file, name) in CLI_DURATIONS {
        let path = onerom_dir.join(file);
        let value = rust_millis(&read(&path), name).unwrap_or_else(|| {
            panic!(
                "{name} isn't a Duration::from_millis constant in {}",
                path.display()
            )
        });
        write(name, value);
    }
}
