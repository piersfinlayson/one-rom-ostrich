//! On a device the plugin doesn't support, every command fails, the version
//! included, and nothing is written to flash.

use onerom_fw_emulator::ffi as fw;
use onerom_plugin_tester::run::Outcome;

use super::save::{check_saved, edit, save};
use super::{check, scenario, scenario_arranged};
use crate::device::Device;
use crate::ffi;
use crate::proto::{self, FAIL, VERSION};
use crate::{Ctx, Scenario};

pub const SCENARIOS: &[Scenario] = &[
    scenario(
        "rejection.config",
        "the plugin supports the configuration exactly when the tester's own checks pass",
        config,
    ),
    scenario_arranged(
        "rejection.no_exclusive_serial",
        "without exclusive serial IN every command fails",
        rule_fails,
        no_exclusive_serial,
    ),
    scenario_arranged(
        "rejection.one_ram_slot",
        "without a spare RAM slot every command fails",
        rule_fails,
        one_ram_slot,
    ),
    scenario_arranged(
        "rejection.unaligned_slot",
        "with a slot not on a 4KB sector boundary every command fails",
        rule_fails,
        unaligned_slot,
    ),
    scenario_arranged(
        "rejection.slot_size",
        "with a flash slot a different size from the RAM slot every command fails",
        rule_fails,
        slot_size,
    ),
    scenario_arranged(
        "rejection.missing_api",
        "on firmware without the flash slot address call every command fails",
        rule_fails,
        missing_api,
    ),
];

/// Every command fails on a configuration the plugin otherwise supports.
fn rule_fails(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    if let Err(skip) = ctx.layout() {
        return Ok(skip);
    }
    every_command_fails(d, ctx)
}

/// One of each command, then a check that no save follows.
fn every_command_fails(d: &mut Device, _ctx: &Ctx) -> Result<Outcome, String> {
    let requests: Vec<Vec<u8>> = vec![
        b"VV".to_vec(),
        proto::checked(b"NS"),
        proto::read(b'R', 0x0000, 16),
        proto::read(b'r', 0x0000, 16),
        proto::write(b'W', 0x0100, &[0x5A; 16]),
        proto::write(b'w', 0x0100, &[0x5A; 16]),
        proto::write(b'X', 0x0100, &[0x5A; 16]),
        proto::bulk_read(1, 0, 0),
        proto::bulk_write(0, 0, &[0xA5; 256]),
        proto::checked(b"BRR"),
        proto::checked(&[b'B', b'R', 0]),
    ];
    // Each request fails once, whatever its length
    let request: Vec<u8> = requests.concat();
    d.expect("every command", &request, &vec![FAIL; requests.len()])?;

    d.advance_ms(2 * super::save::SAVE_QUIET_MS);
    d.step_n(20)?;
    let ops = d.flash_state().ops;
    check(ops == 0, || {
        format!("{ops} flash operations on an unsupported device")
    })?;
    Ok(Outcome::Pass)
}

fn config(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    match &ctx.expect {
        Ok(layout) => {
            d.expect("VV", b"VV", &VERSION)?;
            edit(d, 0x0100, 1)?;
            save(d, layout)?;
            check_saved(d, 1 - layout.booted)?;
            Ok(Outcome::Pass)
        }
        Err(_) => every_command_fails(d, ctx),
    }
}

fn no_exclusive_serial() {
    // SAFETY: called before the plugin starts.
    unsafe { ffi::ostrich_host_test_set_entry_flags(0) };
}

fn one_ram_slot() {
    // SAFETY: called before the plugin starts.
    unsafe { ffi::ostrich_host_test_override_ram_slot_count(1) };
}

fn slot_1() -> Option<(u32, u32)> {
    let (mut addr, mut size) = (0u32, 0u32);
    // SAFETY: the outputs outlive the call.
    let found = unsafe { ffi::ostrich_host_test_slot_addr(1, &mut addr, &mut size) } != 0;
    found.then_some((addr, size))
}

fn unaligned_slot() {
    if let Some((addr, size)) = slot_1() {
        // SAFETY: called before the plugin starts.
        unsafe { ffi::ostrich_host_test_override_slot(1, addr + 0x100, size) };
    }
}

fn slot_size() {
    if let Some((addr, size)) = slot_1() {
        // SAFETY: called before the plugin starts.
        unsafe { ffi::ostrich_host_test_override_slot(1, addr, size * 2) };
    }
}

fn missing_api() {
    let ids = [fw::api_id_t_ORA_ID_GET_FLASH_SLOT_ADDR];
    // SAFETY: `ids` outlives the call, which copies it.
    unsafe {
        onerom_plugin_tester::ffi::ora_host_test_withhold_api(ids.as_ptr(), ids.len() as u32)
    };
}
