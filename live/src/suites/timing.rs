//! Reply times against ORSS's timeout, and how long the CLI holds picoboot.

use std::thread;
use std::time::{Duration, Instant};

use super::{Ctx, Outcome, Test, test};
use crate::check::{self, check};
use crate::consts::{
    CMD_TIMEOUT, CUT_MARGIN, HOLD_LOG_INTERVAL, HOLD_MIN_SEEN, HOLD_PROBE_INTERVAL, HOLD_WINDOW,
    OPEN_RETRY_LIMIT, ORSS_REPLY_TIMEOUT, TRAILING_WINDOW,
};
use crate::picoboot::{self, Picoboot, Sample};
use crate::proto::{self, OK};

pub const TESTS: &[Test] = &[
    test(
        "timing.bulk_read_4k",
        "a 4KB ZR is answered within ORSS's 400ms",
        bulk_read_4k,
    ),
    test(
        "timing.bulk_write_4k",
        "a 4KB ZW is answered within ORSS's 400ms",
        bulk_write_4k,
    ),
    test(
        "timing.log_poll_hold",
        "one onerom monitor log poll holds picoboot for less than the CLI's open retry limit",
        log_poll_hold,
    ),
];

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

/// ORSS's transfers are 4KB at 0x8000 and above for a 27256.
fn bulk_read_4k(ctx: &mut Ctx) -> Result<Outcome, String> {
    ctx.quiesce();
    let data = ctx.expected(0x8000, 4096)?;
    let reply = ctx.expect(
        "ZR 4KB",
        &proto::bulk_read(16, 0, 0x80),
        &proto::read_reply(&data),
    )?;
    check::within("the 4KB ZR reply", reply.elapsed(), ORSS_REPLY_TIMEOUT)?;
    Ok(Outcome::Measured(format!("{}ms", ms(reply.elapsed()))))
}

/// The bytes written are those already served.
fn bulk_write_4k(ctx: &mut Ctx) -> Result<Outcome, String> {
    ctx.quiesce();
    let data = ctx.expected(0x8000, 4096)?;
    let reply = ctx.edit("ZW 4KB", &proto::bulk_write(0, 0x80, &data), &[OK])?;
    check::within("the 4KB ZW reply", reply.elapsed(), ORSS_REPLY_TIMEOUT)?;
    Ok(Outcome::Measured(format!("{}ms", ms(reply.elapsed()))))
}

/// The harness claims picoboot over and over while `onerom monitor log` polls
/// a log in RAM. Each run of failed claims is one poll's hold.
fn log_poll_hold(ctx: &mut Ctx) -> Result<Outcome, String> {
    // A save pauses USB, which would lengthen a hold
    ctx.quiesce();
    let picoboot = Picoboot::find(&ctx.serial)?;
    check(picoboot.claim(), || {
        "the harness couldn't claim picoboot before onerom monitor log started".to_string()
    })?;

    let mut log = ctx.onerom.log()?;
    let mut samples = Vec::new();
    let end = Instant::now() + HOLD_WINDOW;
    let mut next_log = Instant::now();
    while Instant::now() < end {
        if Instant::now() >= next_log {
            // A command never finished, which the plugin logs as it discards
            ctx.port()?.send(b"R")?;
            next_log += HOLD_LOG_INTERVAL;
        }
        let start = Instant::now();
        let free = picoboot.claim();
        samples.push(Sample {
            start,
            end: Instant::now(),
            free,
        });
        thread::sleep(HOLD_PROBE_INTERVAL);
    }
    log.stop();

    // The last incomplete command is discarded before the next test sends
    thread::sleep(CMD_TIMEOUT + CUT_MARGIN);
    ctx.port()?.drain(TRAILING_WINDOW)?;

    hold_outcome(&picoboot::holds(&samples))
}

fn hold_outcome(holds: &[Duration]) -> Result<Outcome, String> {
    check(holds.len() >= HOLD_MIN_SEEN, || {
        format!(
            "{} holds seen, want {HOLD_MIN_SEEN}. Is the log in RAM?",
            holds.len()
        )
    })?;
    let mut sorted = holds.to_vec();
    sorted.sort();
    let longest = sorted[sorted.len() - 1];
    let median = sorted[sorted.len() / 2];
    check::within("the longest hold of picoboot", longest, OPEN_RETRY_LIMIT)?;
    Ok(Outcome::Measured(format!(
        "longest hold {}ms, median {}ms, of {} holds. Open retry limit {}ms",
        ms(longest),
        ms(median),
        holds.len(),
        ms(OPEN_RETRY_LIMIT)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holds(ms: &[u64]) -> Vec<Duration> {
        ms.iter().map(|&m| Duration::from_millis(m)).collect()
    }

    #[test]
    fn holds_under_the_retry_limit_pass() {
        let mut seen = holds(&[5; HOLD_MIN_SEEN]);
        seen.push(OPEN_RETRY_LIMIT - Duration::from_millis(1));
        assert!(matches!(hold_outcome(&seen), Ok(Outcome::Measured(_))));
    }

    #[test]
    fn a_hold_at_the_retry_limit_fails() {
        let mut seen = holds(&[5; HOLD_MIN_SEEN]);
        seen.push(OPEN_RETRY_LIMIT);
        assert!(hold_outcome(&seen).is_err());
    }

    #[test]
    fn too_few_holds_fail() {
        assert!(hold_outcome(&holds(&[5; HOLD_MIN_SEEN - 1])).is_err());
    }
}
