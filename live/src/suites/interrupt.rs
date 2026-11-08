//! A reset at any point of a save leaves One ROM booting a complete image.
//!
//! Each test starts a save of a new image and halts the RP2350 with a hardware
//! breakpoint at one point of it. It then resets the RP2350 through the debug
//! probe and checks what One ROM boots and that the next save works.
//!
//! The breakpoints are on the bootrom's flash functions. The plugin runs its
//! flash routine from a copy on its stack, so a breakpoint on the routine's
//! symbol never hits, and the plugin's ELF doesn't have a line table. Each
//! bootrom function's first argument is the flash offset, which identifies the
//! sector.
//!
//! The reset doesn't cut the flash chip's power, and every breakpoint is
//! between flash commands. An erase or program cut off inside the flash chip
//! isn't covered. That requires a power cut.
//!
//! [`prepare`], [`start_save`], [`check_boot`] and [`check_next_save`] don't
//! depend on how the save is interrupted.

use std::thread;
use std::time::Instant;

use super::{Ctx, Outcome, Test, test};
use crate::check::{self, check};
use crate::consts::{
    BOOTROM_RANGE_ERASE, BOOTROM_RANGE_PROGRAM, BOOTROM_SELECT_XIP, FLASH_BASE, FLASH_SECTOR_SIZE,
    LATENCY, LOG_LATENCY, NV_BOOT_SLOT_ADDR, NV_STORAGE_ADDR, SAVE_MAX, SAVE_QUIET, STOP_ALLOWANCE,
    USER_PLUGIN_CORE,
};
use crate::debug::{Debugger, Stop};
use crate::onerom::{Layout, Region};
use crate::probe::boot_slot;
use crate::proto::{self, OK};

pub const TESTS: &[Test] = &[
    test(
        "interrupt.first_sector_before_erase",
        "a reset before the save erases its first sector boots a complete image",
        |ctx| run(ctx, &FIRST_BEFORE_ERASE),
    ),
    test(
        "interrupt.first_sector_after_erase",
        "a reset after the save erases its first sector boots a complete image",
        |ctx| run(ctx, &FIRST_AFTER_ERASE),
    ),
    test(
        "interrupt.middle_sector_before_erase",
        "a reset before the save erases its middle sector boots a complete image",
        |ctx| run(ctx, &MIDDLE_BEFORE_ERASE),
    ),
    test(
        "interrupt.middle_sector_after_erase",
        "a reset after the save erases its middle sector boots a complete image",
        |ctx| run(ctx, &MIDDLE_AFTER_ERASE),
    ),
    test(
        "interrupt.last_sector_before_erase",
        "a reset before the save erases its last sector boots a complete image",
        |ctx| run(ctx, &LAST_BEFORE_ERASE),
    ),
    test(
        "interrupt.last_sector_after_erase",
        "a reset after the save erases its last sector boots a complete image",
        |ctx| run(ctx, &LAST_AFTER_ERASE),
    ),
    test(
        "interrupt.before_boot_slot_erase",
        "a reset after the slot is written and before the boot slot byte is erased boots a complete image",
        |ctx| run(ctx, &BEFORE_BOOT_SLOT_ERASE),
    ),
    test(
        "interrupt.before_boot_slot_write_from_0",
        "with slot 0 booted, a reset after the boot slot byte is erased and before it is written boots a complete image",
        |ctx| run(ctx, &BEFORE_BOOT_SLOT_WRITE_FROM_0),
    ),
    test(
        "interrupt.before_boot_slot_write_from_1",
        "with slot 1 booted, a reset after the boot slot byte is erased and before it is written boots a complete image",
        |ctx| run(ctx, &BEFORE_BOOT_SLOT_WRITE_FROM_1),
    ),
    test(
        "interrupt.after_boot_slot_write",
        "a reset after the boot slot byte is written boots the new image",
        |ctx| run(ctx, &AFTER_BOOT_SLOT_WRITE),
    ),
];

/// A flash sector the save erases and programs.
#[derive(Clone, Copy)]
enum Sector {
    /// The first, middle or last sector of the slot the save writes
    First,
    Middle,
    Last,
    /// The plugin's NV storage, which holds the boot slot byte
    Nv,
}

/// The boot slot byte the save has left at a point.
#[derive(Clone, Copy)]
enum Byte {
    Unchanged,
    Erased,
    Target,
}

/// A point in a save: the call of bootrom function `function` for `sector`,
/// then, where `then` is set, the next call of that function.
struct Point {
    function: [u8; 2],
    sector: Sector,
    then: Option<[u8; 2]>,
    byte: Byte,
    /// The slot booted before the save, where it matters. A save first makes
    /// One ROM boot it.
    from: Option<u8>,
}

const fn point(function: [u8; 2], sector: Sector, byte: Byte) -> Point {
    Point {
        function,
        sector,
        then: None,
        byte,
        from: None,
    }
}

// The program call follows the erase of the same sector, so a stop there is
// after the erase.
const FIRST_BEFORE_ERASE: Point = point(BOOTROM_RANGE_ERASE, Sector::First, Byte::Unchanged);
const FIRST_AFTER_ERASE: Point = point(BOOTROM_RANGE_PROGRAM, Sector::First, Byte::Unchanged);
const MIDDLE_BEFORE_ERASE: Point = point(BOOTROM_RANGE_ERASE, Sector::Middle, Byte::Unchanged);
const MIDDLE_AFTER_ERASE: Point = point(BOOTROM_RANGE_PROGRAM, Sector::Middle, Byte::Unchanged);
const LAST_BEFORE_ERASE: Point = point(BOOTROM_RANGE_ERASE, Sector::Last, Byte::Unchanged);
const LAST_AFTER_ERASE: Point = point(BOOTROM_RANGE_PROGRAM, Sector::Last, Byte::Unchanged);
const BEFORE_BOOT_SLOT_ERASE: Point = point(BOOTROM_RANGE_ERASE, Sector::Nv, Byte::Unchanged);
// An erased boot slot byte selects slot 0. For a save from slot 0 that is the
// old slot, and for a save from slot 1 it is the slot just written.
const BEFORE_BOOT_SLOT_WRITE_FROM_0: Point = Point {
    from: Some(0),
    ..point(BOOTROM_RANGE_PROGRAM, Sector::Nv, Byte::Erased)
};
const BEFORE_BOOT_SLOT_WRITE_FROM_1: Point = Point {
    from: Some(1),
    ..point(BOOTROM_RANGE_PROGRAM, Sector::Nv, Byte::Erased)
};
// The plugin restores XIP right after the program call returns.
const AFTER_BOOT_SLOT_WRITE: Point = Point {
    then: Some(BOOTROM_SELECT_XIP),
    ..point(BOOTROM_RANGE_PROGRAM, Sector::Nv, Byte::Target)
};

/// An erased flash byte.
const ERASED: u8 = 0xFF;

/// The plugin's log line at startup, followed by the slot it booted.
const BOOTED: &str = "Ostrich: chip ";
const BOOTED_SLOT: &str = "booted slot ";

/// Where the next save's edit writes.
const NEXT_SAVE_ADDR: u16 = 0x0100;

/// One ROM before the save, and the save's image.
pub struct Before {
    pub layout: Layout,
    pub byte: u8,
    /// The slot the save writes
    pub target: u8,
    /// The served image before and after the edit, as the RAM holds them
    pub old: Vec<u8>,
    pub new: Vec<u8>,
}

/// What One ROM booted after the interrupted save.
pub struct Booted {
    pub byte: u8,
    pub slot: u8,
    pub image: &'static str,
}

fn name(function: [u8; 2]) -> String {
    String::from_utf8_lossy(&function).into_owned()
}

fn run(ctx: &mut Ctx, point: &Point) -> Result<Outcome, String> {
    let mut dbg = Debugger::attach(ctx.probe.selector.as_deref())?;
    let mut before = prepare(ctx, &mut dbg)?;
    if point.from == Some(before.target) {
        check_next_save(ctx, &mut dbg, &before.layout, 1 - before.target)?;
        before = prepare(ctx, &mut dbg)?;
    }
    let offset = point.sector.offset(&before);
    let first = dbg.bootrom_function(point.function)?;
    let then = point.then.map(|f| dbg.bootrom_function(f)).transpose()?;
    dbg.break_at(USER_PLUGIN_CORE, first)?;

    let edited = start_save(ctx, &mut dbg, &mut before)?;
    let deadline = edited + SAVE_QUIET + SAVE_MAX + STOP_ALLOWANCE;
    let stop = stop_at(&mut dbg, first, offset, deadline).map_err(|e| {
        format!(
            "waiting for {} at offset 0x{offset:08X}: {e}",
            name(point.function)
        )
    })?;
    let mut at = format!("{} at 0x{:08X}", name(point.function), stop.r0);
    if let (Some(function), Some(addr)) = (point.then, then) {
        dbg.clear_breakpoints(USER_PLUGIN_CORE)?;
        dbg.break_at(USER_PLUGIN_CORE, addr)?;
        dbg.resume(USER_PLUGIN_CORE)?;
        let next = dbg.wait_stop(USER_PLUGIN_CORE, deadline)?;
        stopped_at(next, addr)?;
        at = format!("{at}, then {}", name(function));
    }
    dbg.halt_all()?;
    dbg.reset()?;
    ctx.reconnect()?;

    let booted = check_boot(ctx, &mut dbg, &before)?;
    let want = match point.byte {
        Byte::Unchanged => before.byte,
        Byte::Erased => ERASED,
        Byte::Target => before.target,
    };
    check(booted.byte == want, || {
        format!(
            "stopped at {at}, the boot slot byte is 0x{:02X}, want 0x{want:02X}",
            booted.byte
        )
    })?;
    let next = check_next_save(ctx, &mut dbg, &before.layout, booted.slot)?;
    Ok(Outcome::Measured(format!(
        "stopped at {at}. Boot slot byte 0x{:02X}, booted slot {} with the {} image, next save to slot {next}",
        booted.byte, booted.slot, booted.image
    )))
}

impl Sector {
    /// The flash offset the bootrom function is called with for this sector.
    fn offset(self, before: &Before) -> u32 {
        let slot = before.layout.slots[usize::from(before.target)];
        let sectors = slot.size / FLASH_SECTOR_SIZE;
        let sector = match self {
            Sector::First => 0,
            Sector::Middle => sectors / 2,
            Sector::Last => sectors - 1,
            Sector::Nv => return NV_STORAGE_ADDR - FLASH_BASE,
        };
        slot.addr - FLASH_BASE + sector * FLASH_SECTOR_SIZE
    }
}

/// Fail unless the core stopped at the breakpoint at `addr`.
fn stopped_at(stop: Stop, addr: u32) -> Result<(), String> {
    check(stop.pc == addr, || {
        format!(
            "the core halted at 0x{:08X}, not at the breakpoint at 0x{addr:08X}",
            stop.pc
        )
    })
}

/// Let the save run past the breakpoint at `addr` until the call for
/// `offset`.
fn stop_at(dbg: &mut Debugger, addr: u32, offset: u32, deadline: Instant) -> Result<Stop, String> {
    loop {
        let stop = dbg.wait_stop(USER_PLUGIN_CORE, deadline)?;
        stopped_at(stop, addr)?;
        if stop.r0 == offset {
            return Ok(stop);
        }
        dbg.resume(USER_PLUGIN_CORE)?;
    }
}

/// Wait for any save to finish and record One ROM's state. The booted slot
/// must hold the served image.
pub fn prepare(ctx: &mut Ctx, dbg: &mut Debugger) -> Result<Before, String> {
    ctx.quiesce();
    let layout = ctx.onerom.layout()?;
    check(
        layout.slots.len() == 2 && layout.slots.iter().all(|s| s.size == layout.served.size),
        || format!("the plugin requires two ROM slots of the served image's size: {layout:?}"),
    )?;
    let byte = dbg.read_u8(NV_BOOT_SLOT_ADDR)?;
    let booted = boot_slot(byte);
    let old = read(dbg, layout.served)?;
    let flash = read(dbg, layout.slots[usize::from(booted)])?;
    check::image(
        &format!("before the save, slot {booted}'s flash"),
        0,
        &old,
        &flash,
    )?;
    Ok(Before {
        layout,
        byte,
        target: 1 - booted,
        old,
        new: Vec::new(),
    })
}

fn read(dbg: &mut Debugger, region: Region) -> Result<Vec<u8>, String> {
    dbg.read(region.addr, region.size as usize)
}

/// Replace the served image with its inverse, so every sector of the save's
/// image differs from the old one, and return when the edit was answered.
pub fn start_save(
    ctx: &mut Ctx,
    dbg: &mut Debugger,
    before: &mut Before,
) -> Result<Instant, String> {
    let image: Vec<u8> = ctx.image()?.iter().map(|b| !b).collect();
    let msb = ((0x10000 - image.len()) >> 8) as u8;
    let reply = ctx.edit("ZW", &proto::bulk_write(0, msb, &image), &[OK])?;
    before.new = read(dbg, before.layout.served)?;
    let sector = FLASH_SECTOR_SIZE as usize;
    let same = before
        .old
        .chunks(sector)
        .zip(before.new.chunks(sector))
        .position(|(old, new)| old == new);
    check(same.is_none(), || {
        format!(
            "sector {} of the edited image matches the old image",
            same.unwrap()
        )
    })?;
    Ok(reply.last)
}

/// One letter per sector of `flash`: `o` for the old image's, `n` for the
/// new's and `x` for neither.
fn sector_map(flash: &[u8], before: &Before) -> String {
    let sector = FLASH_SECTOR_SIZE as usize;
    flash
        .chunks(sector)
        .zip(before.old.chunks(sector).zip(before.new.chunks(sector)))
        .map(|(f, (old, new))| match f {
            f if f == old => 'o',
            f if f == new => 'n',
            _ => 'x',
        })
        .collect()
}

/// The plugin's log, the boot slot byte and the firmware agree on the booted
/// slot, the slot holds exactly the old or the new image, and One ROM serves
/// it.
pub fn check_boot(ctx: &mut Ctx, dbg: &mut Debugger, before: &Before) -> Result<Booted, String> {
    let mut log = ctx.onerom.log()?;
    let line = log.wait_for(0, BOOTED, Instant::now() + LOG_LATENCY);
    log.stop();
    let (_, line) = line.ok_or_else(|| format!("the log doesn't contain \"{BOOTED}\""))?;
    let logged = line
        .text
        .split(BOOTED_SLOT)
        .nth(1)
        .and_then(|n| n.trim().parse::<u8>().ok())
        .ok_or_else(|| {
            format!(
                "the log line doesn't contain the booted slot: {}",
                line.text
            )
        })?;

    let byte = dbg.read_u8(NV_BOOT_SLOT_ADDR)?;
    let slot = boot_slot(byte);
    check(logged == slot, || {
        format!(
            "the plugin logged booted slot {logged}, but the boot slot byte 0x{byte:02X} selects slot {slot}"
        )
    })?;
    let selected = ctx.onerom.selected_slot()?;
    check(selected == slot, || {
        format!("onerom inspect slots lists slot {selected} as selected, want {slot}")
    })?;

    let flash = read(dbg, before.layout.slots[usize::from(slot)])?;
    let image = if flash == before.old {
        "old"
    } else if flash == before.new {
        "new"
    } else {
        return Err(format!(
            "slot {slot} booted, but its flash is neither the old image nor the new. By sector: {} (o old, n new, x neither)",
            sector_map(&flash, before)
        ));
    };
    check::image(
        &format!("the served image after slot {slot} booted"),
        0,
        &flash,
        &read(dbg, before.layout.served)?,
    )?;
    Ok(Booted { byte, slot, image })
}

/// An edit is saved to the slot One ROM didn't boot, and the boot slot byte
/// selects that slot. Returns the slot.
pub fn check_next_save(
    ctx: &mut Ctx,
    dbg: &mut Debugger,
    layout: &Layout,
    booted: u8,
) -> Result<u8, String> {
    let target = 1 - booted;
    let data = proto::pattern(16, booted);
    let reply = ctx.edit(
        "W after the reset",
        &proto::write(b'W', NEXT_SAVE_ADDR, &data),
        &[OK],
    )?;
    let done = reply.last + SAVE_QUIET + SAVE_MAX + LATENCY;
    thread::sleep(done.saturating_duration_since(Instant::now()));

    let byte = dbg.read_u8(NV_BOOT_SLOT_ADDR)?;
    check(byte == target, || {
        format!("after the next save the boot slot byte is 0x{byte:02X}, want {target}")
    })?;
    check::image(
        &format!("after the next save, slot {target}'s flash"),
        0,
        &read(dbg, layout.served)?,
        &read(dbg, layout.slots[usize::from(target)])?,
    )?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn before() -> Before {
        let region = |addr| Region {
            addr,
            size: 4 * FLASH_SECTOR_SIZE,
        };
        Before {
            layout: Layout {
                served: region(0x2000_0000),
                slots: vec![region(0x1003_0000), region(0x1004_0000)],
            },
            byte: ERASED,
            target: 1,
            old: vec![1; 4 * FLASH_SECTOR_SIZE as usize],
            new: vec![2; 4 * FLASH_SECTOR_SIZE as usize],
        }
    }

    #[test]
    fn sectors_are_offsets_in_the_target_slot() {
        let b = before();
        assert_eq!(Sector::First.offset(&b), 0x4_0000);
        assert_eq!(Sector::Middle.offset(&b), 0x4_2000);
        assert_eq!(Sector::Last.offset(&b), 0x4_3000);
        assert_eq!(Sector::Nv.offset(&b), NV_STORAGE_ADDR - FLASH_BASE);
    }

    #[test]
    fn a_mixture_is_mapped_by_sector() {
        let b = before();
        let sector = FLASH_SECTOR_SIZE as usize;
        let mut flash = b.new.clone();
        flash[2 * sector..].copy_from_slice(&b.old[2 * sector..]);
        flash[3 * sector] = 9;
        assert_eq!(sector_map(&flash, &b), "nnox");
    }
}
