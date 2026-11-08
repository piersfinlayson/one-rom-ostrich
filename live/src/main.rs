//! Ostrich plugin live tests
//!
//! Drives a One ROM running the Ostrich plugin through its USB serial port,
//! and checks what the plugin did with the One ROM CLI and probe-rs.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

mod check;
mod consts;
mod debug;
mod lab;
mod onerom;
mod picoboot;
mod port;
mod probe;
#[path = "../../tester/src/proto.rs"]
mod proto;
mod suites;

use suites::{Ctx, Outcome};

/// Runs the Ostrich plugin's tests against a One ROM over USB.
#[derive(Parser)]
struct Args {
    /// USB serial number of the One ROM to test. Other devices' serial ports
    /// aren't opened.
    #[arg(long, value_name = "SERIAL", value_parser = parse_serial)]
    serial: Option<String>,

    /// The One ROM's board type, for example fire-28-c.
    #[arg(long, value_name = "BOARD")]
    board: Option<String>,

    /// Path to the One ROM CLI.
    #[arg(long, value_name = "PATH", default_value = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../build/one-rom/rust/target/debug/onerom"
    ))]
    onerom: PathBuf,

    /// Path to probe-rs.
    #[arg(long, value_name = "PATH", default_value = "probe-rs")]
    probe_rs: PathBuf,

    /// probe-rs's --probe selector, VID:PID or VID:PID:SERIAL, where more
    /// than one debug probe is attached.
    #[arg(long, value_name = "PROBE")]
    probe: Option<String>,

    /// USB serial number of the One ROM Lab acting as the ECU. The datalog
    /// tests are skipped without it.
    #[arg(long, value_name = "SERIAL", value_parser = parse_serial)]
    lab: Option<String>,

    /// A test or suite to run. Repeat for more. Every test runs by default.
    #[arg(long, value_name = "NAME")]
    test: Vec<String>,

    /// List the tests and exit.
    #[arg(long)]
    list: bool,
}

/// A serial number matched exactly, so `onerom --serial` can't select
/// another device with a wildcard.
fn parse_serial(s: &str) -> Result<String, String> {
    if s.is_empty() || s.contains(['*', '?']) {
        return Err("a serial number without wildcards is required".to_string());
    }
    Ok(s.to_string())
}

fn main() -> ExitCode {
    let args = Args::parse();
    let tests = match suites::select(&args.test) {
        Ok(tests) => tests,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if args.list {
        for t in tests {
            println!("{:<36} {}", t.name, t.about);
        }
        return ExitCode::SUCCESS;
    }
    let (Some(serial), Some(board)) = (&args.serial, &args.board) else {
        eprintln!("--serial and --board are required to run tests");
        return ExitCode::FAILURE;
    };

    let probe = probe::Probe {
        path: args.probe_rs.clone(),
        selector: args.probe.clone(),
    };
    let ctx = onerom::Onerom::new(args.onerom.clone(), serial, board).and_then(|onerom| {
        let lab = args.lab.as_deref().map(lab::Lab::connect).transpose()?;
        Ctx::connect(serial, onerom, probe, lab)
    });
    let mut ctx = match ctx {
        Ok(ctx) => ctx,
        Err(e) => {
            println!("FAIL setup: {e}");
            return ExitCode::FAILURE;
        }
    };

    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    for t in tests {
        if let Some(why) = t.opt_in.filter(|_| !args.test.iter().any(|n| n == t.name)) {
            skipped += 1;
            println!("SKIP {}: {why}. Use --test {} to run it.", t.name, t.name);
            continue;
        }
        match (t.run)(&mut ctx) {
            Ok(Outcome::Pass) => {
                passed += 1;
                println!("PASS {}", t.name);
            }
            Ok(Outcome::Measured(value)) => {
                passed += 1;
                println!("PASS {}: {value}", t.name);
            }
            Ok(Outcome::Skip(why)) => {
                skipped += 1;
                println!("SKIP {}: {why}", t.name);
            }
            Err(evidence) => {
                failed += 1;
                println!("FAIL {}: {} - {evidence}", t.name, t.about);
            }
        }
    }
    println!("\n{passed} passed, {failed} failed, {skipped} skipped");
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
