//! Write data is staged in the spare RAM slot and applied only once its
//! checksum matches.

use onerom_plugin_tester::run::Outcome;

use super::commands::layout_or_skip;
use super::{check, scenario};
use crate::device::Device;
use crate::proto::{self, FAIL, OK};
use crate::{Ctx, Scenario};

pub const SCENARIOS: &[Scenario] = &[
    scenario(
        "staging.bad_checksum_write",
        "W with a bad checksum fails and leaves the served image unchanged",
        bad_checksum_write,
    ),
    scenario(
        "staging.bad_checksum_bulk_write",
        "ZW with a bad checksum fails and leaves the served image unchanged",
        bad_checksum_bulk_write,
    ),
    scenario(
        "staging.stale_data",
        "a write applies its own bytes and none a failed write staged",
        stale_data,
    ),
    scenario(
        "staging.no_save_after_failed_write",
        "a failed write doesn't start a save",
        no_save_after_failed_write,
    ),
];

fn bad_checksum_write(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let before = d.served(0, layout.chip_size as usize)?;
    let request = proto::bad_checksum(&proto::write_unchecked(b'W', 0x0100, &[0x5A; 64]));
    d.expect("W", &request, &[FAIL])?;
    let after = d.served(0, layout.chip_size as usize)?;
    check(after == before, || {
        "a failed W changed the served image".to_string()
    })?;
    Ok(Outcome::Pass)
}

fn bad_checksum_bulk_write(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let before = d.served(0, layout.chip_size as usize)?;
    let data = proto::pattern(0x1000, 13);
    let request = proto::bad_checksum(&proto::bulk_write_unchecked(0, 0, &data));
    d.expect("ZW", &request, &[FAIL])?;
    let after = d.served(0, layout.chip_size as usize)?;
    check(after == before, || {
        "a failed ZW changed the served image".to_string()
    })?;
    Ok(Outcome::Pass)
}

/// A failed 64KB ZW fills the staging slot.  A later W must apply only its
/// own range.
fn stale_data(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let size = layout.chip_size as usize;
    let mut want = d.served(0, size)?;

    let junk = proto::pattern(0x10000, 17);
    let request = proto::bad_checksum(&proto::bulk_write_unchecked(0, 0, &junk));
    d.expect("failed ZW", &request, &[FAIL])?;

    let data = proto::pattern(16, 19);
    d.expect("W", &proto::write(b'W', 0x0100, &data), &[OK])?;
    want[0x100..0x110].copy_from_slice(&data);
    let got = d.served(0, size)?;
    check(got == want, || {
        let n = got.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(0);
        format!("offset 0x{n:X} changed outside the W")
    })?;
    Ok(Outcome::Pass)
}

fn no_save_after_failed_write(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    let request = proto::bad_checksum(&proto::write_unchecked(b'W', 0x0100, &[0xA5; 16]));
    d.expect("W", &request, &[FAIL])?;
    d.advance_ms(4000);
    d.step_n(20)?;
    let ops = d.flash_state().ops;
    check(ops == 0, || {
        format!("{ops} flash operations after a failed write")
    })?;
    Ok(Outcome::Pass)
}
