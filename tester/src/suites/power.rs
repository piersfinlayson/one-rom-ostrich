//! A power cut at any point of a save leaves One ROM booting a complete
//! image.
//!
//! The shim records the flash halfway through every erase and program and once
//! each completes.  Halfway through an erase or program of NV storage the boot
//! slot byte is recorded as a value that isn't a ROM slot.  For each record the
//! tester works out the slot One ROM would boot, as the firmware does with the
//! plugin's header bits: the boot slot byte if it identifies a ROM slot,
//! otherwise slot 0.  That slot must hold the image just saved, or still be the
//! slot the save started from with the image it held.

use onerom_plugin_tester::run::Outcome;

use super::commands::layout_or_skip;
use super::save::{SAVE_QUIET_MS, edit, save, save_passes, sectors};
use super::{check, scenario, scenario_booting};
use crate::device::{Device, hash};
use crate::ffi::{Cut, OP_ERASE, PHASE_AFTER, PHASE_MID};
use crate::{Boot, Ctx, Layout, Scenario};

pub const SCENARIOS: &[Scenario] = &[
    scenario(
        "power.first_save",
        "a cut anywhere in a save from slot 0 to slot 1 boots a complete image",
        first_save,
    ),
    scenario(
        "power.second_save",
        "a cut anywhere in the save back to slot 0 boots a complete image",
        second_save,
    ),
    scenario_booting(
        "power.from_slot_1",
        "a cut anywhere in a save from slot 1 to slot 0 boots a complete image",
        from_slot_1,
        Boot { nv: 1 },
    ),
    scenario_booting(
        "power.unknown_boot_slot_byte",
        "a boot slot byte that isn't 0 or 1 boots slot 0, and the save goes to slot 1",
        unknown_boot_slot_byte,
        Boot { nv: 0x42 },
    ),
    scenario(
        "power.abandoned_save",
        "a cut in a save an edit abandoned boots the image it started from",
        abandoned_save,
    ),
];

/// Check every recorded cut boots `new`, or boots `from` holding `old`.
fn check_cuts(cuts: &[Cut], from: u8, old: u64, new: Option<u64>) -> Result<(), String> {
    for c in cuts {
        let boot = if c.nv_boot_slot < 2 {
            c.nv_boot_slot
        } else {
            0
        };
        let when = || {
            format!(
                "a cut {} {} {}",
                match c.phase {
                    PHASE_MID => "halfway through",
                    PHASE_AFTER => "after",
                    _ => "at an unrecorded point of",
                },
                if c.kind == OP_ERASE {
                    "erase"
                } else {
                    "program"
                },
                c.op
            )
        };
        check(boot < 2, || format!("{} boots slot {boot}", when()))?;
        let held = c.slot_hash[boot as usize];
        let complete = Some(held) == new || (boot == from && held == old);
        check(complete, || {
            format!("{} boots slot {boot}, which holds neither image", when())
        })?;
    }
    Ok(())
}

/// Every flash operation was recorded halfway and after.
fn check_recorded(d: &Device, cuts: &[Cut]) -> Result<(), String> {
    let ops = d.flash_state().ops as usize;
    check(ops > 0 && cuts.len() == 2 * ops, || {
        format!("{} cuts recorded for {ops} operations", cuts.len())
    })
}

fn save_and_check(d: &mut Device, layout: &Layout, from: u8) -> Result<(), String> {
    let old = hash(&d.region(u32::from(from)));
    d.clear_flash_logs();
    edit(d, 0x0100, from.wrapping_add(1))?;
    save(d, layout)?;
    let new = hash(&d.served_physical());
    let cuts = d.cuts();
    check_recorded(d, &cuts)?;
    check_cuts(&cuts, from, old, Some(new))?;
    let nv = d.nv_boot_slot();
    check(nv == 1 - from, || {
        format!("boot slot byte {nv} after the save")
    })
}

fn first_save(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    if layout.booted != 0 {
        return Ok(Outcome::Skip(format!("booted slot {}", layout.booted)));
    }
    save_and_check(d, &layout, 0)?;
    Ok(Outcome::Pass)
}

fn second_save(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    if layout.booted != 0 {
        return Ok(Outcome::Skip(format!("booted slot {}", layout.booted)));
    }
    save_and_check(d, &layout, 0)?;
    save_and_check(d, &layout, 1)?;
    Ok(Outcome::Pass)
}

fn from_slot_1(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    check(layout.booted == 1, || {
        format!("booted slot {}", layout.booted)
    })?;
    save_and_check(d, &layout, 1)?;
    Ok(Outcome::Pass)
}

fn unknown_boot_slot_byte(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    check(layout.booted == 0, || {
        format!("booted slot {}", layout.booted)
    })?;
    save_and_check(d, &layout, 0)?;
    Ok(Outcome::Pass)
}

fn abandoned_save(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    if sectors(&layout) < 3 {
        return Ok(Outcome::Skip("a save shorter than 3 windows".to_string()));
    }
    let from = layout.booted;
    let old = hash(&d.region(u32::from(from)));
    edit(d, 0x0100, 1)?;
    d.advance_ms(SAVE_QUIET_MS);
    d.step_n(3)?;
    edit(d, 0x0200, 2)?;
    d.step_n(save_passes(&layout))?;
    let cuts = d.cuts();
    check_recorded(d, &cuts)?;
    check_cuts(&cuts, from, old, None)?;
    Ok(Outcome::Pass)
}
