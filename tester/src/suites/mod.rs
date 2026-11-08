//! Scenario tables.

use crate::{BOOT_DEFAULT, Boot, Scenario, ScenarioFn, Suite};

mod commands;
mod datalog;
mod mapping;
mod power;
mod rejection;
mod save;
mod staging;

pub const SUITES: &[Suite] = &[
    Suite {
        name: "commands",
        blurb: "every command, checksums, framing and timeouts",
        scenarios: commands::SCENARIOS,
    },
    Suite {
        name: "mapping",
        blurb: "Ostrich addresses wrap at the chip size",
        scenarios: mapping::SCENARIOS,
    },
    Suite {
        name: "staging",
        blurb: "writes apply only once their checksum matches",
        scenarios: staging::SCENARIOS,
    },
    Suite {
        name: "save",
        blurb: "edits are saved to the other slot of the pair",
        scenarios: save::SCENARIOS,
    },
    Suite {
        name: "power",
        blurb: "a power cut at any point of a save boots a complete image",
        scenarios: power::SCENARIOS,
    },
    Suite {
        name: "rejection",
        blurb: "every command fails on a device the plugin doesn't support",
        scenarios: rejection::SCENARIOS,
    },
    Suite {
        name: "datalog",
        blurb: "the ECU's serial port is bridged to serial port 2",
        scenarios: datalog::SCENARIOS,
    },
];

/// A scenario booting slot 0 with NV storage erased.
pub const fn scenario(name: &'static str, about: &'static str, run: ScenarioFn) -> Scenario {
    Scenario {
        name,
        about,
        run,
        boot: BOOT_DEFAULT,
        before_start: None,
    }
}

/// A scenario booting with `boot.nv` in NV storage.
pub const fn scenario_booting(
    name: &'static str,
    about: &'static str,
    run: ScenarioFn,
    boot: Boot,
) -> Scenario {
    Scenario {
        name,
        about,
        run,
        boot,
        before_start: None,
    }
}

/// A scenario that arranges the shim before the plugin starts.
pub const fn scenario_arranged(
    name: &'static str,
    about: &'static str,
    run: ScenarioFn,
    before_start: fn(),
) -> Scenario {
    Scenario {
        name,
        about,
        run,
        boot: BOOT_DEFAULT,
        before_start: Some(before_start),
    }
}

/// Fail with `what` unless `cond` holds.
pub fn check(cond: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(what()) }
}
