//! Ostrich's 16-bit addresses wrap at the chip size.

use onerom_plugin_tester::run::Outcome;

use super::commands::layout_or_skip;
use super::{check, scenario};
use crate::device::Device;
use crate::proto::{self, OK};
use crate::{Ctx, Scenario};

pub const SCENARIOS: &[Scenario] = &[
    scenario(
        "mapping.upper_half",
        "an image uploaded to the end of the 64KB space lands at offset 0",
        upper_half,
    ),
    scenario(
        "mapping.read_mirror",
        "reads at addr and at addr + chip size reply alike",
        read_mirror,
    ),
    scenario(
        "mapping.write_across_chip_end",
        "a write past the chip's last byte continues at offset 0",
        write_across_chip_end,
    ),
    scenario(
        "mapping.write_across_address_wrap",
        "a write past address 0xFFFF continues at address 0",
        write_across_address_wrap,
    ),
    scenario(
        "mapping.read_across_address_wrap",
        "a read past address 0xFFFF continues at address 0",
        read_across_address_wrap,
    ),
    scenario(
        "mapping.bulk_write_64k",
        "a 64KB ZW leaves the last byte written to each offset",
        bulk_write_64k,
    ),
];

/// A whole image written with ZW where Moates tells software to put it: the
/// end of the 64KB space.
fn upper_half(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let size = layout.chip_size as usize;
    if size < 256 {
        return Ok(Outcome::Skip("chip smaller than a ZW block".to_string()));
    }
    let image = proto::pattern(size, 9);
    let msb = ((0x10000 - size) >> 8) as u8;
    d.expect("ZW image", &proto::bulk_write(0, msb, &image), &[OK])?;
    let got = d.served(0, size)?;
    check(got == image, || "the image isn't at offset 0".to_string())?;
    Ok(Outcome::Pass)
}

fn read_mirror(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    if layout.chip_size == 0x10000 {
        return Ok(Outcome::Skip("a 64KB chip maps one to one".to_string()));
    }
    let data = d.served(0x40, 16)?;
    let mirror = (0x40 + layout.chip_size) as u16;
    d.expect(
        "R low",
        &proto::read(b'R', 0x40, 16),
        &proto::read_reply(&data),
    )?;
    d.expect(
        "R mirror",
        &proto::read(b'R', mirror, 16),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn write_wrapping(d: &mut Device, ctx: &Ctx, addr: u16) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let data = proto::pattern(16, 5);
    d.expect("W", &proto::write(b'W', addr, &data), &[OK])?;
    let tail = (u32::from(addr) & (layout.chip_size - 1)) as usize;
    let first = (layout.chip_size as usize - tail).min(16);
    let end = d.served(tail as u32, first)?;
    let start = d.served(0, 16 - first)?;
    check(end == data[..first] && start == data[first..], || {
        format!("W at 0x{addr:04X} didn't wrap to offset 0")
    })?;
    Ok(Outcome::Pass)
}

fn write_across_chip_end(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let size = match ctx.layout() {
        Ok(layout) => layout.chip_size,
        Err(skip) => return Ok(skip),
    };
    write_wrapping(d, ctx, (size - 8) as u16)
}

fn write_across_address_wrap(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    write_wrapping(d, ctx, 0xFFF8)
}

fn read_across_address_wrap(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let mut data = d.served(0xFFF8 & (layout.chip_size - 1), 8)?;
    data.extend(d.served(0, 8)?);
    d.expect(
        "R",
        &proto::read(b'R', 0xFFF8, 16),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_write_64k(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let size = layout.chip_size as usize;
    let data = proto::pattern(0x10000, 11);
    d.expect("ZW 64KB", &proto::bulk_write(0, 0, &data), &[OK])?;
    let got = d.served(0, size)?;
    check(got == data[0x10000 - size..], || {
        "the served image isn't the last chip-sized part of the data".to_string()
    })?;
    Ok(Outcome::Pass)
}
