//! A write applies only once its checksum matches.

use super::{Ctx, Outcome, Test, test};
use crate::check;
use crate::proto::{self, FAIL, OK};

pub const TESTS: &[Test] = &[
    test(
        "staging.bad_checksum_write",
        "W with a bad checksum fails and leaves the served image unchanged",
        bad_checksum_write,
    ),
    test(
        "staging.bad_checksum_bulk_write",
        "ZW with a bad checksum fails and leaves the served image unchanged",
        bad_checksum_bulk_write,
    ),
    test(
        "staging.stale_data",
        "a write applies its own bytes and none a failed write staged",
        stale_data,
    ),
];

/// Send a request that fails and check the served image is unchanged.
fn unchanged(ctx: &mut Ctx, what: &str, request: &[u8]) -> Result<Outcome, String> {
    let before = ctx.image()?;
    ctx.edit(what, request, &[FAIL])?;
    check::image("the image onerom peek reads", 0, &before, &ctx.image()?)?;
    Ok(Outcome::Pass)
}

fn bad_checksum_write(ctx: &mut Ctx) -> Result<Outcome, String> {
    let request = proto::bad_checksum(&proto::write_unchecked(b'W', 0x0100, &[0x5A; 64]));
    unchanged(ctx, "W", &request)
}

fn bad_checksum_bulk_write(ctx: &mut Ctx) -> Result<Outcome, String> {
    let data = proto::pattern(0x1000, 13);
    let request = proto::bad_checksum(&proto::bulk_write_unchecked(0, 0, &data));
    unchanged(ctx, "ZW", &request)
}

/// A failed 64KB ZW fills the staging slot. A later W must apply only its own
/// range.
fn stale_data(ctx: &mut Ctx) -> Result<Outcome, String> {
    let mut want = ctx.image()?;
    let junk = proto::pattern(0x10000, 17);
    let request = proto::bad_checksum(&proto::bulk_write_unchecked(0, 0, &junk));
    ctx.edit("failed ZW", &request, &[FAIL])?;

    let data = proto::pattern(16, 19);
    ctx.edit("W", &proto::write(b'W', 0x0100, &data), &[OK])?;
    want[0x100..0x110].copy_from_slice(&data);
    check::image("the image onerom peek reads", 0, &want, &ctx.image()?)?;
    Ok(Outcome::Pass)
}
