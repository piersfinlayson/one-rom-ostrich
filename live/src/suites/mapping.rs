//! Ostrich's 16-bit addresses wrap at the chip size.

use super::{Ctx, Outcome, Test, test};
use crate::check;
use crate::proto::{self, OK};

pub const TESTS: &[Test] = &[
    test(
        "mapping.upper_half",
        "an image uploaded to the end of the 64KB space lands at offset 0",
        upper_half,
    ),
    test(
        "mapping.read_mirror",
        "reads at addr and at addr + chip size reply alike",
        read_mirror,
    ),
    test(
        "mapping.write_across_chip_end",
        "a write past the chip's last byte continues at offset 0",
        write_across_chip_end,
    ),
    test(
        "mapping.write_across_address_wrap",
        "a write past address 0xFFFF continues at address 0",
        write_across_address_wrap,
    ),
    test(
        "mapping.read_across_address_wrap",
        "a read past address 0xFFFF continues at address 0",
        read_across_address_wrap,
    ),
    test(
        "mapping.bulk_write_64k",
        "a 64KB ZW leaves the last byte written to each offset",
        bulk_write_64k,
    ),
];

/// A whole image written with ZW where Moates tells software to put it: the
/// end of the 64KB space.
fn upper_half(ctx: &mut Ctx) -> Result<Outcome, String> {
    let size = ctx.chip_size as usize;
    if size < 256 {
        return Ok(Outcome::Skip(
            "the chip is smaller than a ZW block".to_string(),
        ));
    }
    let image = proto::pattern(size, 9);
    let msb = ((0x10000 - size) >> 8) as u8;
    ctx.edit("ZW image", &proto::bulk_write(0, msb, &image), &[OK])?;
    check::image("the image onerom peek reads", 0, &image, &ctx.image()?)?;
    Ok(Outcome::Pass)
}

fn read_mirror(ctx: &mut Ctx) -> Result<Outcome, String> {
    if ctx.chip_size == 0x10000 {
        return Ok(Outcome::Skip("a 64KB chip maps one to one".to_string()));
    }
    let data = ctx.image()?[0x40..0x50].to_vec();
    let mirror = (0x40 + ctx.chip_size) as u16;
    ctx.expect(
        "R low",
        &proto::read(b'R', 0x40, 16),
        &proto::read_reply(&data),
    )?;
    ctx.expect(
        "R mirror",
        &proto::read(b'R', mirror, 16),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn write_wrapping(ctx: &mut Ctx, addr: u16) -> Result<Outcome, String> {
    let data = proto::pattern(16, 5);
    ctx.edit("W", &proto::write(b'W', addr, &data), &[OK])?;
    let image = ctx.image()?;
    let tail = (u32::from(addr) & (ctx.chip_size - 1)) as usize;
    let first = (image.len() - tail).min(16);
    check::image(
        "the chip's end",
        tail as u32,
        &data[..first],
        &image[tail..tail + first],
    )?;
    check::image("the chip's start", 0, &data[first..], &image[..16 - first])?;
    Ok(Outcome::Pass)
}

fn write_across_chip_end(ctx: &mut Ctx) -> Result<Outcome, String> {
    let addr = (ctx.chip_size - 8) as u16;
    write_wrapping(ctx, addr)
}

fn write_across_address_wrap(ctx: &mut Ctx) -> Result<Outcome, String> {
    write_wrapping(ctx, 0xFFF8)
}

fn read_across_address_wrap(ctx: &mut Ctx) -> Result<Outcome, String> {
    let image = ctx.image()?;
    let end = (0xFFF8 & (ctx.chip_size - 1)) as usize;
    let data = [&image[end..end + 8], &image[..8]].concat();
    ctx.expect(
        "R",
        &proto::read(b'R', 0xFFF8, 16),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_write_64k(ctx: &mut Ctx) -> Result<Outcome, String> {
    let size = ctx.chip_size as usize;
    let data = proto::pattern(0x10000, 11);
    ctx.edit("ZW 64KB", &proto::bulk_write(0, 0, &data), &[OK])?;
    check::image(
        "the image onerom peek reads",
        0,
        &data[0x10000 - size..],
        &ctx.image()?,
    )?;
    Ok(Outcome::Pass)
}
