//! Every command, its checksum, its framing and the command timeout.

use onerom_plugin_tester::run::Outcome;

use super::{check, scenario};
use crate::device::Device;
use crate::proto::{self, FAIL, OK, VERSION};
use crate::{Ctx, Layout, Scenario};

pub const SCENARIOS: &[Scenario] = &[
    scenario(
        "commands.version",
        "VV replies with the version bytes",
        version,
    ),
    scenario(
        "commands.version_other",
        "V followed by anything but V fails",
        version_other,
    ),
    scenario(
        "commands.serial",
        "NS replies with the vendor byte, the chip ID and their checksum",
        serial,
    ),
    scenario(
        "commands.serial_bad_checksum",
        "NS with a bad checksum fails",
        serial_bad_checksum,
    ),
    scenario(
        "commands.serial_change",
        "Nx and Nn fail once their 11 bytes have arrived",
        serial_change,
    ),
    scenario(
        "commands.read",
        "R replies with n bytes of the served image and their checksum",
        read,
    ),
    scenario(
        "commands.read_256",
        "R with n = 0 replies with 256 bytes",
        read_256,
    ),
    scenario("commands.read_legacy", "r reads as R does", read_legacy),
    scenario(
        "commands.read_bad_checksum",
        "R with a bad checksum fails",
        read_bad_checksum,
    ),
    scenario(
        "commands.write",
        "W changes the served image and replies O",
        write,
    ),
    scenario(
        "commands.write_256",
        "W with n = 0 writes 256 bytes",
        write_256,
    ),
    scenario("commands.write_legacy", "w writes as W does", write_legacy),
    scenario(
        "commands.write_x",
        "X writes and replies with the 16-bit sum of the data, MSB first",
        write_x,
    ),
    scenario(
        "commands.bulk_read",
        "ZR replies with n 256-byte blocks from MSB:00 and their checksum",
        bulk_read,
    ),
    scenario(
        "commands.bulk_read_mmsb",
        "ZR ignores MMSB in bank 0",
        bulk_read_mmsb,
    ),
    scenario(
        "commands.bulk_read_64k",
        "ZR with n = 0 replies with 64KB, more than the reply channel holds",
        bulk_read_64k,
    ),
    scenario(
        "commands.bulk_write",
        "ZW writes n 256-byte blocks from MSB:00",
        bulk_write,
    ),
    scenario(
        "commands.bulk_unknown",
        "Z followed by anything but R or W fails at once",
        bulk_unknown,
    ),
    scenario(
        "commands.banks",
        "bank reads reply 0, bank 0 is selectable and nothing else",
        banks,
    ),
    scenario(
        "commands.unsupported",
        "trace, baud rate and unknown commands fail and framing continues",
        unsupported,
    ),
    scenario(
        "commands.pipelined",
        "requests sent together are answered in order",
        pipelined,
    ),
    scenario(
        "commands.timeout",
        "an incomplete command is discarded 500ms after its last byte",
        timeout,
    ),
    scenario(
        "commands.reply_dropped",
        "a reply the host stops reading is dropped after 1s and the plugin answers again",
        reply_dropped,
    ),
];

/// The layout, or return the skip from the scenario.
macro_rules! layout_or_skip {
    ($ctx:expr) => {
        match $ctx.layout() {
            Ok(layout) => layout,
            Err(skip) => return Ok(skip),
        }
    };
}
pub(crate) use layout_or_skip;

/// The bytes a read from Ostrich address `addr` returns, from the served
/// image, with addresses wrapping at 64KB and at the chip size.
pub fn expected(d: &Device, layout: &Layout, addr: u32, len: usize) -> Result<Vec<u8>, String> {
    let image = d.served(0, layout.chip_size as usize)?;
    let mask = layout.chip_size - 1;
    Ok((0..len as u32)
        .map(|i| image[(((addr + i) & 0xFFFF) & mask) as usize])
        .collect())
}

fn version(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    d.expect("VV", b"VV", &VERSION)?;
    Ok(Outcome::Pass)
}

fn version_other(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    d.expect("VX", b"VX", &[FAIL])?;
    d.expect("VV after VX", b"VV", &VERSION)?;
    Ok(Outcome::Pass)
}

fn serial(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    let mut want = vec![proto::VENDOR_ID];
    want.extend_from_slice(&d.chip_id()?);
    want.push(proto::sum(&want));
    d.expect("NS", &proto::checked(b"NS"), &want)?;
    Ok(Outcome::Pass)
}

fn serial_bad_checksum(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    d.expect("NS", &proto::bad_checksum(b"NS"), &[FAIL])?;
    Ok(Outcome::Pass)
}

fn serial_change(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    let id = d.chip_id()?;
    let mut vendor = vec![b'N', 0x42];
    vendor.extend_from_slice(&id);
    let mut number = vec![b'N', b'n'];
    number.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

    let mut request = proto::checked(&vendor);
    request.extend(proto::checked(&number));
    request.extend_from_slice(b"VV");
    let mut want = vec![FAIL, FAIL];
    want.extend_from_slice(&VERSION);
    d.expect("Nx, Nn then VV", &request, &want)?;
    Ok(Outcome::Pass)
}

fn read_with(d: &mut Device, ctx: &Ctx, op: u8, addr: u16, len: usize) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let data = expected(d, &layout, u32::from(addr), len)?;
    d.expect(
        "read",
        &proto::read(op, addr, len),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn read(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    read_with(d, ctx, b'R', 0x0123, 16)
}

fn read_256(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    read_with(d, ctx, b'R', 0x0100, 256)
}

fn read_legacy(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    read_with(d, ctx, b'r', 0x0040, 32)
}

fn read_bad_checksum(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    let mut request = proto::read(b'R', 0x0000, 16);
    *request.last_mut().unwrap() ^= 0x01;
    d.expect("R", &request, &[FAIL])?;
    Ok(Outcome::Pass)
}

fn write_with(d: &mut Device, ctx: &Ctx, op: u8, addr: u16, len: usize) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let data = proto::pattern(len, 1);
    d.expect("write", &proto::write(op, addr, &data), &[OK])?;
    let got = expected(d, &layout, u32::from(addr), len)?;
    check(got == data, || {
        "the served image doesn't hold the written bytes".to_string()
    })?;
    // The image reads back the same through the plugin
    d.expect(
        "read back",
        &proto::read(b'R', addr, len),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn write(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    write_with(d, ctx, b'W', 0x0200, 16)
}

fn write_256(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    write_with(d, ctx, b'W', 0x0300, 256)
}

fn write_legacy(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    write_with(d, ctx, b'w', 0x0410, 48)
}

fn write_x(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    // The sum exceeds 8 bits
    let data = vec![0xFFu8; 16];
    d.expect("X", &proto::write(b'X', 0x0500, &data), &[0x0F, 0xF0])?;
    let got = expected(d, &layout, 0x0500, 16)?;
    check(got == data, || {
        "the served image doesn't hold the X bytes".to_string()
    })?;

    let data = proto::pattern(256, 7);
    let total: u32 = data.iter().map(|&b| u32::from(b)).sum();
    d.expect(
        "X 256",
        &proto::write(b'X', 0x0600, &data),
        &(total as u16).to_be_bytes(),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_read(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let data = expected(d, &layout, 0x0100, 512)?;
    d.expect(
        "ZR",
        &proto::bulk_read(2, 0, 0x01),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_read_mmsb(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let data = expected(d, &layout, 0x0200, 256)?;
    d.expect(
        "ZR MMSB 5",
        &proto::bulk_read(1, 5, 0x02),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_read_64k(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let data = expected(d, &layout, 0, 0x10000)?;
    d.expect(
        "ZR 64KB",
        &proto::bulk_read(0, 0, 0),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_write(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let data = proto::pattern(512, 3);
    d.expect("ZW", &proto::bulk_write(0, 0x03, &data), &[OK])?;
    let got = expected(d, &layout, 0x0300, 512)?;
    check(got == data, || {
        "the served image doesn't hold the ZW blocks".to_string()
    })?;
    Ok(Outcome::Pass)
}

fn bulk_unknown(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    let mut want = vec![FAIL];
    want.extend_from_slice(&VERSION);
    d.expect("ZQ then VV", b"ZQVV", &want)?;
    Ok(Outcome::Pass)
}

fn banks(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    let cases: &[(&[u8], u8)] = &[
        (b"BRR", 0),
        (&[b'B', b'R', 0], OK),
        (&[b'B', b'R', 1], FAIL),
        (&[b'B', b'R', 8], FAIL),
        (b"BER", 0),
        (b"BEE", 0),
        (b"BES", 0),
        (&[b'B', b'E', 0], OK),
        (&[b'B', b'E', 1], FAIL),
        (&[b'B', b'E', 8], FAIL),
        (&[b'B', b'S', 0], OK),
        (&[b'B', b'S', 1], FAIL),
        (&[b'B', b'X', 0], FAIL),
    ];
    for &(request, reply) in cases {
        let what = format!("{request:02X?}");
        d.expect(&what, &proto::checked(request), &[reply])?;
    }
    d.expect("BRR bad checksum", &proto::bad_checksum(b"BRR"), &[FAIL])?;
    d.expect(
        "BR 0 bad checksum",
        &proto::bad_checksum(&[b'B', b'R', 0]),
        &[FAIL],
    )?;
    Ok(Outcome::Pass)
}

fn unsupported(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    let trace = [b'T', 0x01, 0, 0, 16, 1, 0, 0, 0, 0, 0xFF, 0xFF];
    let mut request = proto::checked(&trace);
    request.extend(proto::checked(&[b'S', 0]));
    request.push(0x00);
    request.push(b'q');
    request.extend_from_slice(b"VV");
    let mut want = vec![FAIL, FAIL, FAIL, FAIL];
    want.extend_from_slice(&VERSION);
    d.expect("T, S, 0x00, q then VV", &request, &want)?;
    Ok(Outcome::Pass)
}

fn pipelined(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let before = expected(d, &layout, 0x0010, 4)?;
    let data = [0x11, 0x22, 0x33, 0x44];

    let mut request = b"VV".to_vec();
    request.extend(proto::read(b'R', 0x0010, 4));
    request.extend(proto::write(b'W', 0x0010, &data));
    request.extend(proto::read(b'R', 0x0010, 4));
    request.extend(proto::checked(b"BRR"));

    let mut want = VERSION.to_vec();
    want.extend(proto::read_reply(&before));
    want.push(OK);
    want.extend(proto::read_reply(&data));
    want.push(0);
    d.expect("pipelined", &request, &want)?;
    Ok(Outcome::Pass)
}

fn timeout(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let request = proto::read(b'R', 0x0020, 16);

    // A gap just short of the timeout leaves the command in progress
    d.send(&request[..2])?;
    d.settle()?;
    d.advance_ms(499);
    d.step_n(2)?;
    let data = expected(d, &layout, 0x0020, 16)?;
    d.expect("rest of R", &request[2..], &proto::read_reply(&data))?;

    // A gap of the timeout discards it without a reply, and VV is read as a
    // new command
    d.send(&request[..2])?;
    d.settle()?;
    d.advance_ms(500);
    d.step_n(2)?;
    let stray = d.take();
    check(stray.is_empty(), || {
        format!("a discarded command replied {stray:02X?}")
    })?;
    d.expect("VV after the timeout", b"VV", &VERSION)?;
    Ok(Outcome::Pass)
}

fn reply_dropped(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    layout_or_skip!(ctx);
    d.set_draining(false);
    d.send(&proto::bulk_read(0, 0, 0))?;
    d.step_n(50)?;
    d.advance_ms(1000);
    d.step_n(5)?;
    d.set_draining(true);
    let got = d.take();
    check(!got.is_empty() && got.len() < 0x10001, || {
        format!("{} reply bytes, want part of the 65537", got.len())
    })?;
    d.expect("VV after the dropped reply", b"VV", &VERSION)?;
    Ok(Outcome::Pass)
}
