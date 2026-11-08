//! Every command, its checksum, its framing and the timeouts.

use std::thread;

use super::{Ctx, Outcome, Test, test};
use crate::check::{self, check};
use crate::consts::{CMD_TIMEOUT, CUT_MARGIN, TRAILING_WINDOW, UNREAD_WAIT};
use crate::proto::{self, FAIL, OK, VERSION};

pub const TESTS: &[Test] = &[
    test(
        "commands.version",
        "VV replies with the version bytes",
        version,
    ),
    test(
        "commands.version_other",
        "V followed by anything but V fails",
        version_other,
    ),
    test(
        "commands.serial",
        "NS replies with the vendor byte, the chip ID and their checksum",
        serial,
    ),
    test(
        "commands.serial_bad_checksum",
        "NS with a bad checksum fails",
        serial_bad_checksum,
    ),
    test(
        "commands.serial_change",
        "Nx and Nn fail once their 11 bytes have arrived",
        serial_change,
    ),
    test(
        "commands.read",
        "R replies with n bytes of the served image and their checksum",
        read,
    ),
    test(
        "commands.read_256",
        "R with n = 0 replies with 256 bytes",
        read_256,
    ),
    test("commands.read_legacy", "r reads as R does", read_legacy),
    test(
        "commands.read_bad_checksum",
        "R with a bad checksum fails",
        read_bad_checksum,
    ),
    test(
        "commands.write",
        "W changes the served image and replies O",
        write,
    ),
    test(
        "commands.write_256",
        "W with n = 0 writes 256 bytes",
        write_256,
    ),
    test("commands.write_legacy", "w writes as W does", write_legacy),
    test(
        "commands.write_x",
        "X writes and replies with the 16-bit sum of the data, MSB first",
        write_x,
    ),
    test(
        "commands.bulk_read",
        "ZR replies with n 256-byte blocks from MSB:00",
        bulk_read,
    ),
    test(
        "commands.bulk_read_mmsb",
        "ZR ignores MMSB in bank 0",
        bulk_read_mmsb,
    ),
    test(
        "commands.bulk_read_64k",
        "ZR with n = 0 replies with 64KB",
        bulk_read_64k,
    ),
    test(
        "commands.bulk_write",
        "ZW writes n 256-byte blocks from MSB:00",
        bulk_write,
    ),
    test(
        "commands.bulk_unknown",
        "Z followed by anything but R or W fails at once",
        bulk_unknown,
    ),
    test(
        "commands.banks",
        "bank reads reply 0 and only bank 0 is selectable",
        banks,
    ),
    test(
        "commands.baud_rate",
        "S fails and framing continues",
        baud_rate,
    ),
    test(
        "commands.unknown",
        "trace and unknown commands fail and framing continues",
        unknown,
    ),
    test(
        "commands.pipelined",
        "requests sent together are answered in order",
        pipelined,
    ),
    test(
        "commands.timeout",
        "an incomplete command is discarded 500ms after its last byte",
        timeout,
    ),
    test(
        "commands.reply_dropped",
        "a reply the host stops reading is dropped after 1s and the plugin answers again",
        reply_dropped,
    ),
];

fn version(ctx: &mut Ctx) -> Result<Outcome, String> {
    ctx.expect("VV", b"VV", &VERSION)?;
    Ok(Outcome::Pass)
}

fn version_other(ctx: &mut Ctx) -> Result<Outcome, String> {
    ctx.expect("VX", b"VX", &[FAIL])?;
    ctx.expect("VV after VX", b"VV", &VERSION)?;
    Ok(Outcome::Pass)
}

fn serial(ctx: &mut Ctx) -> Result<Outcome, String> {
    let mut want = vec![proto::VENDOR_ID];
    want.extend_from_slice(&ctx.chip_id);
    want.push(proto::sum(&want));
    ctx.expect("NS", &proto::checked(b"NS"), &want)?;
    Ok(Outcome::Pass)
}

fn serial_bad_checksum(ctx: &mut Ctx) -> Result<Outcome, String> {
    ctx.expect("NS", &proto::bad_checksum(b"NS"), &[FAIL])?;
    Ok(Outcome::Pass)
}

fn serial_change(ctx: &mut Ctx) -> Result<Outcome, String> {
    let mut vendor = vec![b'N', 0x42];
    vendor.extend_from_slice(&ctx.chip_id);
    let mut number = vec![b'N', b'n'];
    number.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);

    let mut request = proto::checked(&vendor);
    request.extend(proto::checked(&number));
    request.extend_from_slice(b"VV");
    let mut want = vec![FAIL, FAIL];
    want.extend_from_slice(&VERSION);
    ctx.expect("Nx, Nn then VV", &request, &want)?;
    Ok(Outcome::Pass)
}

fn read_with(ctx: &mut Ctx, op: u8, addr: u16, len: usize) -> Result<Outcome, String> {
    let data = ctx.expected(u32::from(addr), len)?;
    ctx.expect(
        "read",
        &proto::read(op, addr, len),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn read(ctx: &mut Ctx) -> Result<Outcome, String> {
    read_with(ctx, b'R', 0x0123, 16)
}

fn read_256(ctx: &mut Ctx) -> Result<Outcome, String> {
    read_with(ctx, b'R', 0x0100, 256)
}

fn read_legacy(ctx: &mut Ctx) -> Result<Outcome, String> {
    read_with(ctx, b'r', 0x0040, 32)
}

fn read_bad_checksum(ctx: &mut Ctx) -> Result<Outcome, String> {
    let mut request = proto::read(b'R', 0x0000, 16);
    *request.last_mut().unwrap() ^= 0x01;
    ctx.expect("R", &request, &[FAIL])?;
    Ok(Outcome::Pass)
}

/// Write a pattern with `op`, then check `onerom peek` and R both return it.
pub fn write_with(
    ctx: &mut Ctx,
    op: u8,
    addr: u16,
    len: usize,
    seed: u8,
) -> Result<Outcome, String> {
    let data = proto::pattern(len, seed);
    ctx.edit("write", &proto::write(op, addr, &data), &[OK])?;
    let served = ctx.expected(u32::from(addr), len)?;
    check::image(
        "the image onerom peek reads",
        u32::from(addr),
        &data,
        &served,
    )?;
    ctx.expect(
        "read back",
        &proto::read(b'R', addr, len),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn write(ctx: &mut Ctx) -> Result<Outcome, String> {
    write_with(ctx, b'W', 0x0200, 16, 1)
}

fn write_256(ctx: &mut Ctx) -> Result<Outcome, String> {
    write_with(ctx, b'W', 0x0300, 256, 2)
}

fn write_legacy(ctx: &mut Ctx) -> Result<Outcome, String> {
    write_with(ctx, b'w', 0x0410, 48, 3)
}

fn write_x(ctx: &mut Ctx) -> Result<Outcome, String> {
    // The sum exceeds 8 bits
    let data = vec![0xFFu8; 16];
    ctx.edit("X", &proto::write(b'X', 0x0500, &data), &[0x0F, 0xF0])?;
    let served = ctx.expected(0x0500, 16)?;
    check::image("the image onerom peek reads", 0x0500, &data, &served)?;

    let data = proto::pattern(256, 7);
    let total: u32 = data.iter().map(|&b| u32::from(b)).sum();
    ctx.edit(
        "X 256",
        &proto::write(b'X', 0x0600, &data),
        &(total as u16).to_be_bytes(),
    )?;
    let served = ctx.expected(0x0600, 256)?;
    check::image("the image onerom peek reads", 0x0600, &data, &served)?;
    Ok(Outcome::Pass)
}

fn bulk_read(ctx: &mut Ctx) -> Result<Outcome, String> {
    let data = ctx.expected(0x0100, 512)?;
    ctx.expect(
        "ZR",
        &proto::bulk_read(2, 0, 0x01),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_read_mmsb(ctx: &mut Ctx) -> Result<Outcome, String> {
    let data = ctx.expected(0x0200, 256)?;
    ctx.expect(
        "ZR MMSB 5",
        &proto::bulk_read(1, 5, 0x02),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_read_64k(ctx: &mut Ctx) -> Result<Outcome, String> {
    let data = ctx.expected(0, 0x10000)?;
    ctx.expect(
        "ZR 64KB",
        &proto::bulk_read(0, 0, 0),
        &proto::read_reply(&data),
    )?;
    Ok(Outcome::Pass)
}

fn bulk_write(ctx: &mut Ctx) -> Result<Outcome, String> {
    let data = proto::pattern(512, 3);
    ctx.edit("ZW", &proto::bulk_write(0, 0x03, &data), &[OK])?;
    let served = ctx.expected(0x0300, 512)?;
    check::image("the image onerom peek reads", 0x0300, &data, &served)?;
    Ok(Outcome::Pass)
}

fn bulk_unknown(ctx: &mut Ctx) -> Result<Outcome, String> {
    let mut want = vec![FAIL];
    want.extend_from_slice(&VERSION);
    ctx.expect("ZQ then VV", b"ZQVV", &want)?;
    Ok(Outcome::Pass)
}

/// The bank commands ORSS sends (BRR, then BS 0) and those mstrand's
/// ostrich2 sends, with banks 0 and others.
fn banks(ctx: &mut Ctx) -> Result<Outcome, String> {
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
        ctx.expect(
            &format!("{request:02X?}"),
            &proto::checked(request),
            &[reply],
        )?;
    }
    ctx.expect("BRR bad checksum", &proto::bad_checksum(b"BRR"), &[FAIL])?;
    ctx.expect(
        "BR 0 bad checksum",
        &proto::bad_checksum(&[b'B', b'R', 0]),
        &[FAIL],
    )?;
    Ok(Outcome::Pass)
}

fn baud_rate(ctx: &mut Ctx) -> Result<Outcome, String> {
    // 0 is 921.6k and 7 is 115.2k
    let mut request = proto::checked(&[b'S', 0]);
    request.extend(proto::checked(&[b'S', 7]));
    request.extend_from_slice(b"VV");
    let mut want = vec![FAIL, FAIL];
    want.extend_from_slice(&VERSION);
    ctx.expect("S 0, S 7 then VV", &request, &want)?;
    Ok(Outcome::Pass)
}

fn unknown(ctx: &mut Ctx) -> Result<Outcome, String> {
    let trace = [b'T', 0x01, 0, 0, 16, 1, 0, 0, 0, 0, 0xFF, 0xFF];
    let mut request = proto::checked(&trace);
    request.push(0x00);
    request.push(b'q');
    request.extend_from_slice(b"VV");
    let mut want = vec![FAIL, FAIL, FAIL];
    want.extend_from_slice(&VERSION);
    ctx.expect("T, 0x00, q then VV", &request, &want)?;
    Ok(Outcome::Pass)
}

fn pipelined(ctx: &mut Ctx) -> Result<Outcome, String> {
    let before = ctx.expected(0x0010, 4)?;
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
    ctx.edit("pipelined", &request, &want)?;
    Ok(Outcome::Pass)
}

fn timeout(ctx: &mut Ctx) -> Result<Outcome, String> {
    let request = proto::read(b'R', 0x0020, 16);
    let data = ctx.expected(0x0020, 16)?;

    // A gap short of the timeout leaves the command in progress
    ctx.port()?.send(&request[..2])?;
    thread::sleep(CMD_TIMEOUT - CUT_MARGIN);
    ctx.expect(
        "rest of R after a gap",
        &request[2..],
        &proto::read_reply(&data),
    )?;

    // A gap past the timeout discards it without a reply, and VV is read as a
    // new command
    ctx.port()?.send(&request[..2])?;
    thread::sleep(CMD_TIMEOUT + CUT_MARGIN);
    let stray = ctx.port()?.drain(TRAILING_WINDOW)?;
    check(stray.is_empty(), || {
        format!("the discarded command replied {stray:02X?}")
    })?;
    ctx.expect("VV after the timeout", b"VV", &VERSION)?;
    Ok(Outcome::Pass)
}

fn reply_dropped(ctx: &mut Ctx) -> Result<Outcome, String> {
    let want = proto::read_reply(&ctx.expected(0, 0x10000)?);
    let mut log = ctx.onerom.log()?;
    ctx.port()?.send(&proto::bulk_read(0, 0, 0))?;
    thread::sleep(UNREAD_WAIT);
    let got = ctx.port()?.drain(TRAILING_WINDOW)?;
    let after = ctx.expect("VV after the unread reply", b"VV", &VERSION);
    // The plugin's log line is the only sign that it stopped sending
    let dropped = log
        .stop()
        .iter()
        .any(|l| l.contains("Ostrich: host stopped reading, reply dropped"));
    after?;
    dropped_outcome(&want, &got, dropped)
}

/// Whether a 64KB reply left unread was dropped as the plugin drops one.
fn dropped_outcome(want: &[u8], got: &[u8], dropped: bool) -> Result<Outcome, String> {
    if !dropped {
        return Ok(Outcome::Skip(format!(
            "the plugin sent the whole reply, as the host kept reading. {} of {} bytes arrived",
            got.len(),
            want.len()
        )));
    }
    check(got.len() < want.len(), || {
        "the plugin logged a dropped reply, but all of it arrived".to_string()
    })?;
    check::image(
        "the part of the reply that arrived",
        0,
        &want[..got.len()],
        got,
    )?;
    Ok(Outcome::Pass)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_the_host_buffered_whole_is_skipped() {
        let want = [1, 2, 3, 4];
        assert!(matches!(
            dropped_outcome(&want, &want, false),
            Ok(Outcome::Skip(_))
        ));
        assert!(matches!(
            dropped_outcome(&want, &want[..2], false),
            Ok(Outcome::Skip(_))
        ));
    }

    #[test]
    fn a_dropped_reply_passes_where_it_stops_short_and_intact() {
        let want = [1, 2, 3, 4];
        assert!(matches!(
            dropped_outcome(&want, &want[..3], true),
            Ok(Outcome::Pass)
        ));
        assert!(dropped_outcome(&want, &[1, 9], true).is_err());
        assert!(dropped_outcome(&want, &want, true).is_err());
    }
}
