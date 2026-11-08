//! Edits are saved to the other slot of the pair once the host stops writing,
//! one 4KB sector per exclusive mode window, and the boot slot byte changes
//! last.

use onerom_plugin_tester::run::Outcome;

use super::commands::layout_or_skip;
use super::{check, scenario, scenario_arranged, scenario_booting};
use crate::device::Device;
use crate::ffi::{self, FlashOp, OP_ERASE, OP_PROGRAM};
use crate::proto::{self, OK};
use crate::{Boot, Ctx, Layout, Scenario};

/// NV storage's offset from the start of flash: the last 4KB of the user plugin
/// region, as the plugin's linker script places it.
pub const NV_FLASH_OFFSET: u32 = 0x2_F000;

pub const SAVE_QUIET_MS: u64 = 3000;

pub const SCENARIOS: &[Scenario] = &[
    scenario(
        "save.quiet_period",
        "a save starts 3s after the last edit and writes the served image to the other slot",
        quiet_period,
    ),
    scenario(
        "save.quiet_restarts",
        "each edit restarts the 3s",
        quiet_restarts,
    ),
    scenario(
        "save.windows",
        "one sector is erased and programmed per exclusive window, the boot slot byte last",
        windows,
    ),
    scenario(
        "save.reads_between_windows",
        "reads are answered between windows and don't stop the save",
        reads_between_windows,
    ),
    scenario(
        "save.edit_abandons",
        "an edit during a save abandons it, and the next save starts again",
        edit_abandons,
    ),
    scenario(
        "save.second_save",
        "the save after a save goes to the first slot",
        second_save,
    ),
    scenario_booting(
        "save.booted_slot_1",
        "a device booted from slot 1 saves to slot 0",
        booted_slot_1,
        Boot { nv: 1 },
    ),
    scenario_arranged(
        "save.verify_failure",
        "a slot that reads back wrong leaves the boot slot byte unchanged until the next edit",
        verify_failure,
        corrupt_first_program,
    ),
    scenario_arranged(
        "save.boot_slot_byte_fails",
        "after the boot slot byte fails to program, the next save doesn't write the slot One ROM boots",
        boot_slot_byte_fails,
        fail_nv_program,
    ),
    scenario_arranged(
        "save.exclusive_refused",
        "a save that can't enter exclusive mode is tried again 3s later",
        exclusive_refused,
        refuse_one_exclusive,
    ),
];

fn corrupt_first_program() {
    // SAFETY: called before the plugin starts.
    unsafe { ffi::ostrich_host_test_corrupt_program(1) };
}

fn fail_nv_program() {
    // SAFETY: called before the plugin starts.
    unsafe { ffi::ostrich_host_test_fail_nv_program(1) };
}

fn refuse_one_exclusive() {
    // SAFETY: called before the plugin starts.
    unsafe { ffi::ostrich_host_test_fail_exclusive(1) };
}

pub fn sectors(layout: &Layout) -> u32 {
    layout.region_size / 4096
}

/// Passes enough for a whole save: one to start, one per sector, one to verify
/// and write the boot slot byte, and two spare.
pub fn save_passes(layout: &Layout) -> u32 {
    sectors(layout) + 4
}

pub fn edit(d: &mut Device, addr: u16, seed: u8) -> Result<(), String> {
    let data = proto::pattern(16, seed);
    d.expect("W", &proto::write(b'W', addr, &data), &[OK])
}

/// Wait out the quiet period and run the save to its end.
pub fn save(d: &mut Device, layout: &Layout) -> Result<(), String> {
    d.advance_ms(SAVE_QUIET_MS);
    d.step_n(save_passes(layout))
}

/// The slot the save wrote holds the served image and is the boot slot.
pub fn check_saved(d: &Device, target: u8) -> Result<(), String> {
    let nv = d.nv_boot_slot();
    check(nv == target, || {
        format!("boot slot byte {nv}, want {target}")
    })?;
    let served = d.served_physical();
    check(d.region(u32::from(target)) == served, || {
        format!("slot {target} doesn't hold the served image")
    })?;
    let state = d.flash_state();
    check(
        state.bad_outside_exclusive == 0
            && state.bad_unmasked == 0
            && state.bad_xip_active == 0
            && state.bad_range == 0
            && state.bad_staged_copy == 0,
        || format!("flash model faults: {state:?}"),
    )
}

fn quiet_period(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let target = 1 - layout.booted;
    edit(d, 0x0100, 1)?;
    d.advance_ms(SAVE_QUIET_MS - 1);
    d.step_n(save_passes(&layout))?;
    let ops = d.flash_state().ops;
    check(ops == 0, || format!("{ops} flash operations before 3s"))?;
    d.advance_ms(1);
    d.step_n(save_passes(&layout))?;
    check_saved(d, target)?;
    Ok(Outcome::Pass)
}

fn quiet_restarts(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    edit(d, 0x0100, 1)?;
    d.advance_ms(2000);
    d.step_n(4)?;
    edit(d, 0x0200, 2)?;
    d.advance_ms(2000);
    d.step_n(save_passes(&layout))?;
    let ops = d.flash_state().ops;
    check(ops == 0, || {
        format!("{ops} flash operations 2s after an edit")
    })?;
    d.advance_ms(1000);
    d.step_n(save_passes(&layout))?;
    check_saved(d, 1 - layout.booted)?;
    Ok(Outcome::Pass)
}

fn windows(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let target = 1 - layout.booted;
    edit(d, 0x0100, 1)?;
    save(d, &layout)?;
    check_saved(d, target)?;

    let n = sectors(&layout);
    let mut want = Vec::new();
    for k in 0..n {
        let offs = layout.slot_offs[target as usize] + k * 4096;
        let window = (k + 1) as u16;
        want.push(FlashOp {
            kind: OP_ERASE,
            region: target,
            window,
            offs,
            count: 4096,
        });
        want.push(FlashOp {
            kind: OP_PROGRAM,
            region: target,
            window,
            offs,
            count: 4096,
        });
    }
    let window = (n + 1) as u16;
    let nv = ffi::REGION_NV as u8;
    want.push(FlashOp {
        kind: OP_ERASE,
        region: nv,
        window,
        offs: NV_FLASH_OFFSET,
        count: 4096,
    });
    want.push(FlashOp {
        kind: OP_PROGRAM,
        region: nv,
        window,
        offs: NV_FLASH_OFFSET,
        count: 256,
    });
    let got = d.flash_ops();
    check(got == want, || {
        let i = got
            .iter()
            .zip(&want)
            .position(|(a, b)| a != b)
            .unwrap_or(got.len().min(want.len()));
        format!(
            "flash operation {i} of {}: got {:?}, want {:?}",
            want.len(),
            got.get(i),
            want.get(i)
        )
    })?;

    let state = d.flash_state();
    let windows = n + 1;
    check(
        state.windows == windows
            && state.connect_calls == windows
            && state.select_xip_calls == windows
            && state.flush_calls == windows
            && state.last_clkdiv == 4
            && state.last_xip_mode == 3
            && state.xip_active == 1
            && state.exclusive_depth == 0,
        || format!("{windows} windows, each restoring XIP with the divisor read: {state:?}"),
    )?;
    let page = d.region(ffi::REGION_NV);
    check(page[1..].iter().all(|&b| b == 0xFF), || {
        "NV storage holds more than the boot slot byte".to_string()
    })?;
    Ok(Outcome::Pass)
}

fn reads_between_windows(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    edit(d, 0x0100, 1)?;
    d.advance_ms(SAVE_QUIET_MS);
    d.step_n(3)?;
    let windows = d.flash_state().windows;
    check(windows > 0, || "the save hadn't started".to_string())?;
    let data = d.served(0x0100, 16)?;
    d.expect(
        "R during save",
        &proto::read(b'R', 0x0100, 16),
        &proto::read_reply(&data),
    )?;
    d.step_n(save_passes(&layout))?;
    check_saved(d, 1 - layout.booted)?;
    Ok(Outcome::Pass)
}

fn edit_abandons(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    if sectors(&layout) < 3 {
        return Ok(Outcome::Skip("a save shorter than 3 windows".to_string()));
    }
    let target = 1 - layout.booted;
    let nv_before = d.nv_boot_slot();
    edit(d, 0x0100, 1)?;
    d.advance_ms(SAVE_QUIET_MS);
    d.step_n(3)?;
    let windows = d.flash_state().windows;
    check(windows == 2, || {
        format!("{windows} windows after 3 passes, want 2")
    })?;

    edit(d, 0x0200, 2)?;
    d.step_n(save_passes(&layout))?;
    let state = d.flash_state();
    check(state.windows == windows, || {
        "the save went on after an edit".to_string()
    })?;
    let nv = d.nv_boot_slot();
    check(nv == nv_before, || {
        "an abandoned save changed the boot slot byte".to_string()
    })?;

    d.clear_flash_logs();
    save(d, &layout)?;
    let first = d.flash_ops().first().copied();
    let want = FlashOp {
        kind: OP_ERASE,
        region: target,
        window: 1,
        offs: layout.slot_offs[target as usize],
        count: 4096,
    };
    check(first == Some(want), || {
        format!("the next save began with {first:?}")
    })?;
    check_saved(d, target)?;
    Ok(Outcome::Pass)
}

fn second_save(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let first = 1 - layout.booted;
    edit(d, 0x0100, 1)?;
    save(d, &layout)?;
    check_saved(d, first)?;
    let first_image = d.served_physical();

    edit(d, 0x0200, 2)?;
    save(d, &layout)?;
    check_saved(d, layout.booted)?;
    check(d.region(u32::from(first)) == first_image, || {
        format!("the second save changed slot {first}")
    })?;
    Ok(Outcome::Pass)
}

fn booted_slot_1(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    check(layout.booted == 1, || {
        format!("booted slot {}", layout.booted)
    })?;
    edit(d, 0x0100, 1)?;
    save(d, &layout)?;
    check_saved(d, 0)?;
    Ok(Outcome::Pass)
}

fn verify_failure(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let target = 1 - layout.booted;
    let nv_before = d.nv_boot_slot();
    edit(d, 0x0100, 1)?;
    save(d, &layout)?;
    let nv = d.nv_boot_slot();
    check(nv == nv_before, || {
        "a slot that read back wrong became the boot slot".to_string()
    })?;
    let ops = d.flash_state().ops;
    check(ops == 2 * sectors(&layout), || {
        format!("{ops} flash operations, want the slot's only")
    })?;

    d.advance_ms(2 * SAVE_QUIET_MS);
    d.step_n(save_passes(&layout))?;
    let again = d.flash_state().ops;
    check(again == ops, || {
        "the failed save was retried without an edit".to_string()
    })?;

    edit(d, 0x0200, 2)?;
    save(d, &layout)?;
    check_saved(d, target)?;
    Ok(Outcome::Pass)
}

fn boot_slot_byte_fails(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let booting = layout.booted;
    let other = 1 - booting;
    let booting_image = d.region(u32::from(booting));
    edit(d, 0x0100, 1)?;
    save(d, &layout)?;
    let nv = d.nv_boot_slot();
    check(nv == 0xFF, || {
        format!("boot slot byte {nv}, want it left erased")
    })?;

    // The erased byte boots slot 0
    edit(d, 0x0200, 2)?;
    save(d, &layout)?;
    let on_booting = d
        .flash_ops()
        .iter()
        .filter(|op| op.region == booting)
        .count();
    check(on_booting == 0, || {
        format!("{on_booting} operations on slot {booting}, which One ROM boots")
    })?;
    check(d.region(u32::from(booting)) == booting_image, || {
        format!("slot {booting} changed")
    })?;
    check(d.region(u32::from(other)) == d.served_physical(), || {
        format!("slot {other} doesn't hold the served image")
    })?;
    Ok(Outcome::Pass)
}

fn exclusive_refused(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    edit(d, 0x0100, 1)?;
    save(d, &layout)?;
    let ops = d.flash_state().ops;
    check(ops == 0, || {
        format!("{ops} flash operations without exclusive mode")
    })?;
    d.advance_ms(SAVE_QUIET_MS - 1);
    d.step_n(save_passes(&layout))?;
    let ops = d.flash_state().ops;
    check(ops == 0, || "the save was retried within 3s".to_string())?;
    save(d, &layout)?;
    check_saved(d, 1 - layout.booted)?;
    Ok(Outcome::Pass)
}
