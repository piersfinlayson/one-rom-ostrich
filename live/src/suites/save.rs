//! Edits are saved to the other ROM slot of the pair, and One ROM boots it.
//!
//! The plugin logs a save's start and end, and an abandoned save, only in
//! firmware built with `PLUGIN_LOGGING=1`. The log is the only sign of when a
//! save starts and whether an edit abandoned one.

use std::thread;
use std::time::Instant;

use super::{Ctx, Outcome, Test, test};
use crate::check::{self, check};
use crate::consts::{
    LATENCY, LOG_LATENCY, MID_SAVE_DELAY, ORSS_REPLY_TIMEOUT, SAVE_MAX, SAVE_QUIET,
};
use crate::onerom::LogCapture;
use crate::probe::boot_slot;
use crate::proto::{self, OK};

pub const TESTS: &[Test] = &[
    test(
        "save.quiet_period",
        "3s after the last edit the served image is saved to the other slot, and the boot slot byte selects it",
        quiet_period,
    ),
    test(
        "save.alternate",
        "successive saves alternate between the slots",
        alternate,
    ),
    test(
        "save.edit_abandons",
        "an edit during a save is answered within 400ms and abandons the save, and the next save has the edit",
        edit_abandons,
    ),
    test(
        "save.reboot",
        "after onerom reboot One ROM serves the saved slot with the edits",
        reboot,
    ),
];

fn saving(slot: u8) -> String {
    format!("Ostrich: saving to slot {slot}")
}

fn saved(slot: u8) -> String {
    format!("Ostrich: saved to slot {slot}")
}

const ABANDONED: &str = "Ostrich: edit during save, save abandoned";

/// Wait out any save in progress, then return the slot the next save writes:
/// the slot of the pair One ROM doesn't boot.
fn next_target(ctx: &mut Ctx) -> Result<u8, String> {
    ctx.quiesce();
    Ok(1 - boot_slot(ctx.probe.boot_slot_byte()?))
}

fn edit(ctx: &mut Ctx, addr: u16, seed: u8) -> Result<Instant, String> {
    let data = proto::pattern(16, seed);
    let reply = ctx.edit("W", &proto::write(b'W', addr, &data), &[OK])?;
    Ok(reply.last)
}

/// The boot slot byte selects `target`, and `target`'s flash holds the served
/// image.
fn check_saved(ctx: &mut Ctx, target: u8) -> Result<(), String> {
    let byte = ctx.probe.boot_slot_byte()?;
    check(byte == target, || {
        format!("the boot slot byte is 0x{byte:02X}, want {target}")
    })?;
    let served = ctx.image()?;
    let flash = ctx.onerom.save_slot(target)?;
    check::image(&format!("slot {target}'s flash"), 0, &served, &flash)
}

/// Wait for `text` in the log after line `from`, failing at `deadline`.
fn logged(
    log: &LogCapture,
    from: usize,
    text: &str,
    deadline: Instant,
) -> Result<(usize, Instant), String> {
    log.wait_for(from, text, deadline)
        .map(|(i, line)| (i, line.at))
        .ok_or_else(|| {
            format!(
                "the log doesn't contain \"{text}\". Is the firmware built with PLUGIN_LOGGING=1?"
            )
        })
}

fn quiet_period(ctx: &mut Ctx) -> Result<Outcome, String> {
    let target = next_target(ctx)?;
    let mut log = ctx.onerom.log()?;
    let edited = edit(ctx, 0x0100, 21)?;

    let (line, started) = logged(
        &log,
        log.start,
        &saving(target),
        edited + SAVE_QUIET + LOG_LATENCY,
    )?;
    check::between(
        "the save started",
        started - edited,
        SAVE_QUIET - LATENCY,
        SAVE_QUIET + LOG_LATENCY,
    )?;
    logged(&log, line, &saved(target), started + SAVE_MAX + LOG_LATENCY)?;
    log.stop();
    check_saved(ctx, target)?;
    Ok(Outcome::Pass)
}

fn alternate(ctx: &mut Ctx) -> Result<Outcome, String> {
    let mut targets = Vec::new();
    for seed in [22, 23] {
        let target = next_target(ctx)?;
        edit(ctx, 0x0110, seed)?;
        thread::sleep(SAVE_QUIET + SAVE_MAX + LATENCY);
        check_saved(ctx, target)?;
        targets.push(target);
    }
    check(targets[0] != targets[1], || {
        format!("both saves went to slot {}", targets[0])
    })?;
    Ok(Outcome::Pass)
}

fn edit_abandons(ctx: &mut Ctx) -> Result<Outcome, String> {
    let target = next_target(ctx)?;
    let mut log = ctx.onerom.log()?;
    let edited = edit(ctx, 0x0120, 24)?;

    thread::sleep((edited + SAVE_QUIET + MID_SAVE_DELAY).saturating_duration_since(Instant::now()));
    let data = proto::pattern(16, 25);
    let reply = ctx.edit(
        "W during the save",
        &proto::write(b'W', 0x0130, &data),
        &[OK],
    )?;

    let (first, _) = logged(
        &log,
        log.start,
        &saving(target),
        edited + SAVE_QUIET + LOG_LATENCY,
    )?;
    let abandoned = match logged(&log, first, ABANDONED, reply.last + LOG_LATENCY) {
        Ok((i, _)) => i,
        Err(_)
            if log
                .wait_for(first, &saved(target), Instant::now())
                .is_some() =>
        {
            return Err("the save finished before the edit sent during it arrived".to_string());
        }
        Err(e) => return Err(e),
    };
    let (second, started) = logged(
        &log,
        abandoned,
        &saving(target),
        reply.last + SAVE_QUIET + LOG_LATENCY,
    )?;
    check::between(
        "the next save started",
        started - reply.last,
        SAVE_QUIET - LATENCY,
        SAVE_QUIET + LOG_LATENCY,
    )?;
    logged(
        &log,
        second,
        &saved(target),
        started + SAVE_MAX + LOG_LATENCY,
    )?;
    log.stop();

    check_saved(ctx, target)?;
    check::image(
        "the edit sent during the save",
        0x0130,
        &data,
        &ctx.image()?[0x130..0x140],
    )?;
    check::within(
        "the reply to the edit sent during the save",
        reply.elapsed(),
        ORSS_REPLY_TIMEOUT,
    )?;
    Ok(Outcome::Measured(format!(
        "the edit sent during the save was answered in {}ms",
        reply.elapsed().as_millis()
    )))
}

fn reboot(ctx: &mut Ctx) -> Result<Outcome, String> {
    let target = next_target(ctx)?;
    edit(ctx, 0x0140, 26)?;
    ctx.quiesce();
    check_saved(ctx, target)?;
    let before = ctx.image()?;

    ctx.reboot()?;
    let selected = ctx.onerom.selected_slot()?;
    check(selected == target, || {
        format!("One ROM booted slot {selected}, want {target}")
    })?;
    check::image(
        "the image served after the reboot",
        0,
        &before,
        &ctx.image()?,
    )?;
    Ok(Outcome::Pass)
}
