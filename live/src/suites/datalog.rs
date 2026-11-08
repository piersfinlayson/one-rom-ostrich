//! The datalog: One ROM's serial port 2, bridged to the ECU's serial port.
//!
//! One ROM Lab acts as the ECU. Its `u` traffic runs on the UART wired to the
//! datalog's pins. The tests are skipped without `--lab`.

use std::thread;
use std::time::{Instant, SystemTime};

use super::{Ctx, Outcome, Test, opt_in_test, test};
use crate::check::{self, check};
use crate::consts::{
    DATALOG_BAUD, DATALOG_CLOSED, DATALOG_EDIT_AT, DATALOG_LATE_READ, DATALOG_LINE_RATE,
    DATALOG_MIN_RATE_PERCENT, DATALOG_POLL, DATALOG_QUIET, DATALOG_RUN, DATALOG_TAIL,
    LAB_READ_DELAY, LATENCY, SAVE_MAX, SAVE_QUIET,
};
use crate::lab::{Lab, Mode, UartCounts};
use crate::port::{self, Port, Which};
use crate::proto::{self, OK, VERSION};

pub const TESTS: &[Test] = &[
    test(
        "datalog.ecu_to_pc",
        "the PC receives Lab's pattern without gaps at close to the line rate",
        ecu_to_pc,
    ),
    test(
        "datalog.both_ways",
        "with the PC and Lab both sending the pattern neither sees a gap, and Lab receives every byte the PC sent",
        both_ways,
    ),
    test(
        "datalog.echo",
        "random bytes the PC sends come back unchanged from Lab's echo",
        echo,
    ),
    test(
        "datalog.save",
        "a save during Lab's pattern doesn't cause a gap, and the saved slot holds the edit",
        save,
    ),
    opt_in_test(
        "datalog.reads",
        "while Lab reads the chip during traffic both ways neither side sees a gap, the reads don't mismatch and One ROM stays on USB",
        "a fire-28-c's USB can fail during this test until the board is power cycled",
        reads,
    ),
    test(
        "datalog.late_open",
        "serial port 2 opened after Lab starts its pattern receives at most one break in it",
        late_open,
    ),
];

/// Run `f` with Lab, and leave Lab's traffic and fast read stopped if it
/// fails.
fn with_lab(
    ctx: &mut Ctx,
    f: impl FnOnce(&mut Ctx, &mut Lab) -> Result<Outcome, String>,
) -> Result<Outcome, String> {
    let Some(mut lab) = ctx.lab.take() else {
        return Ok(Outcome::Skip("--lab isn't set".to_string()));
    };
    // A save pauses USB
    ctx.quiesce();
    let result = f(ctx, &mut lab);
    let reset = if result.is_err() { lab.reset() } else { Ok(()) };
    ctx.lab = Some(lab);
    result.and_then(|outcome| reset.map(|()| outcome))
}

/// Serial port 2 while traffic runs, and what the PC sent and received.
struct Traffic {
    port: Port,
    /// What the PC sends, at the line rate from the first `run`
    out: Vec<u8>,
    sent: usize,
    start: Option<Instant>,
    got: Vec<u8>,
    /// When the first byte arrived
    first: Option<Instant>,
}

impl Traffic {
    /// Open serial port 2 before Lab's traffic starts, and discard what
    /// earlier traffic left in it.
    fn open_idle(ctx: &Ctx, out: Vec<u8>) -> Result<Self, String> {
        let mut t = Self::open(ctx, out)?;
        t.port.drain(DATALOG_QUIET)?;
        Ok(t)
    }

    fn open(ctx: &Ctx, out: Vec<u8>) -> Result<Self, String> {
        let name = port::find(&ctx.serial, Which::Port2)?;
        let port = Port::open_with(&name, DATALOG_BAUD, false)?;
        Ok(Self {
            port,
            out,
            sent: 0,
            start: None,
            got: Vec::new(),
            first: None,
        })
    }

    /// Send what is due and read what arrives, until `until`.
    fn run(&mut self, until: Instant) -> Result<(), String> {
        let start = *self.start.get_or_insert_with(Instant::now);
        while Instant::now() < until {
            let due = start.elapsed().as_millis() as usize * DATALOG_LINE_RATE as usize / 1000;
            let due = due.min(self.out.len());
            if due > self.sent {
                self.port.send(&self.out[self.sent..due])?;
                self.sent = due;
            }
            self.receive((Instant::now() + DATALOG_POLL).min(until))?;
        }
        Ok(())
    }

    /// Read what arrives until `deadline`, and return whether anything did.
    fn receive(&mut self, deadline: Instant) -> Result<bool, String> {
        let more = self.port.read_until(4096, deadline)?;
        if more.is_empty() {
            return Ok(false);
        }
        self.first.get_or_insert_with(Instant::now);
        self.got.extend(more);
        Ok(true)
    }

    /// Stop the PC sending, then stop Lab's traffic once Lab has received
    /// the PC's last bytes, and read what follows.
    fn finish(&mut self, lab: &mut Lab) -> Result<UartCounts, String> {
        self.out.truncate(self.sent);
        self.run(Instant::now() + DATALOG_TAIL)?;
        let counts = lab.stop_uart()?;
        while self.receive(Instant::now() + DATALOG_QUIET)? {}
        Ok(counts)
    }

    /// The bytes a second the PC has received since the first `run`.
    fn rate(&self) -> u32 {
        let secs = self.start.map_or(0.0, |s| s.elapsed().as_secs_f64());
        (self.got.len() as f64 / secs) as u32
    }
}

/// The sizes of the runs of consecutive bytes in `bytes`, 0xFF followed by
/// 0x00 being consecutive.
fn runs(bytes: &[u8]) -> Vec<usize> {
    let mut runs = Vec::new();
    let mut start = 0;
    for i in 1..bytes.len() {
        if bytes[i] != bytes[i - 1].wrapping_add(1) {
            runs.push(i - start);
            start = i;
        }
    }
    if !bytes.is_empty() {
        runs.push(bytes.len() - start);
    }
    runs
}

/// The PC received every byte of Lab's pattern without a gap.
fn check_pc(t: &Traffic, lab: &UartCounts) -> Result<(), String> {
    let runs = runs(&t.got);
    check(runs.len() == 1 && t.got.len() as u64 == lab.sent, || {
        format!(
            "the PC received {} bytes in {} runs of consecutive bytes, the first {:?}. {lab}",
            t.got.len(),
            runs.len(),
            &runs[..runs.len().min(8)]
        )
    })
}

/// Lab received the PC's pattern without a gap.
fn check_lab(t: &Traffic, lab: &UartCounts) -> Result<(), String> {
    check(lab.gaps == 0 && lab.received == t.sent as u64, || {
        format!("the PC sent {}. {lab}", t.sent)
    })
}

fn pattern() -> Vec<u8> {
    (0..run_len()).map(|i| i as u8).collect()
}

/// The bytes for `DATALOG_RUN` at the line rate.
fn run_len() -> usize {
    DATALOG_LINE_RATE as usize * DATALOG_RUN.as_millis() as usize / 1000
}

fn random(len: usize) -> Vec<u8> {
    let mut x = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1;
    (0..len)
        .map(|_| {
            // xorshift64
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn ecu_to_pc(ctx: &mut Ctx) -> Result<Outcome, String> {
    with_lab(ctx, |ctx, lab| {
        let mut t = Traffic::open_idle(ctx, Vec::new())?;
        // The rate is timed from before Lab starts, so what arrives while Lab
        // answers the command counts against that time too.
        t.start = Some(Instant::now());
        lab.start_uart(Mode::Pattern)?;
        t.run(Instant::now() + DATALOG_RUN)?;
        let rate = t.rate();
        let counts = t.finish(lab)?;
        check_pc(&t, &counts)?;
        let min = DATALOG_LINE_RATE * DATALOG_MIN_RATE_PERCENT / 100;
        check(rate >= min, || {
            format!("the PC received {rate} bytes a second, want {min}. {counts}")
        })?;
        let first = t.first.zip(t.start).map_or(0, |(f, s)| (f - s).as_millis());
        Ok(Outcome::Measured(format!(
            "{rate} bytes a second, line rate {DATALOG_LINE_RATE}, first byte after {first}ms"
        )))
    })
}

fn both_ways(ctx: &mut Ctx) -> Result<Outcome, String> {
    with_lab(ctx, |ctx, lab| {
        let mut t = Traffic::open_idle(ctx, pattern())?;
        lab.start_uart(Mode::Pattern)?;
        t.run(Instant::now() + DATALOG_RUN)?;
        let counts = t.finish(lab)?;
        check_pc(&t, &counts)?;
        check_lab(&t, &counts)?;
        Ok(Outcome::Pass)
    })
}

fn echo(ctx: &mut Ctx) -> Result<Outcome, String> {
    with_lab(ctx, |ctx, lab| {
        let mut t = Traffic::open_idle(ctx, random(run_len()))?;
        lab.start_uart(Mode::Echo)?;
        t.run(Instant::now() + DATALOG_RUN)?;
        let counts = t.finish(lab)?;
        check::image("the bytes echoed", 0, &t.out, &t.got)
            .map_err(|e| format!("{e}. {counts}"))?;
        Ok(Outcome::Pass)
    })
}

fn save(ctx: &mut Ctx) -> Result<Outcome, String> {
    with_lab(ctx, |ctx, lab| {
        // Bytes no earlier run left in either slot
        let data = random(16);
        let mut t = Traffic::open_idle(ctx, Vec::new())?;
        lab.start_uart(Mode::Pattern)?;
        let start = Instant::now();
        t.run(start + DATALOG_EDIT_AT)?;
        let edited = ctx
            .edit("W", &proto::write(b'W', 0x0150, &data), &[OK])?
            .last;
        // The traffic runs until any save the edit started has finished
        t.run((start + DATALOG_RUN).max(edited + SAVE_QUIET + SAVE_MAX + LATENCY))?;
        let counts = t.finish(lab)?;
        check_pc(&t, &counts)?;

        let served = ctx.image()?;
        check::image("the edit", 0x0150, &data, &served[0x150..0x160])?;
        for slot in 0..2 {
            if ctx.onerom.save_slot(slot)? == served {
                return Ok(Outcome::Measured(format!("saved to slot {slot}")));
            }
        }
        Err("neither slot's flash holds the served image".to_string())
    })
}

fn reads(ctx: &mut Ctx) -> Result<Outcome, String> {
    with_lab(ctx, |ctx, lab| {
        let chip = ctx.onerom.chip_type()?;
        let mut t = Traffic::open_idle(ctx, pattern())?;
        lab.start_uart(Mode::Pattern)?;
        lab.start_fast_read(&chip, LAB_READ_DELAY)?;
        t.run(Instant::now() + DATALOG_RUN)?;
        let read = lab.stop_fast_read()?;
        let counts = t.finish(lab)?;
        ctx.expect("VV after the reads", b"VV", &VERSION)?;
        check_pc(&t, &counts)?;
        check_lab(&t, &counts)?;
        check(read.passes > 0 && read.mismatches == 0, || {
            format!("Lab's fast read: {}", read.lines.join(" / ").trim())
        })?;
        Ok(Outcome::Measured(format!(
            "{} passes of {chip} at {LAB_READ_DELAY} cycles",
            read.passes
        )))
    })
}

fn late_open(ctx: &mut Ctx) -> Result<Outcome, String> {
    with_lab(ctx, |ctx, lab| {
        lab.start_uart(Mode::Pattern)?;
        thread::sleep(DATALOG_CLOSED);
        let mut t = Traffic::open(ctx, Vec::new())?;
        t.run(Instant::now() + DATALOG_LATE_READ)?;
        let counts = t.finish(lab)?;
        let runs = runs(&t.got);
        check(!runs.is_empty() && runs.len() <= 2, || {
            format!(
                "the PC received {} bytes in {} runs of consecutive bytes, the first {:?}. {counts}",
                t.got.len(),
                runs.len(),
                &runs[..runs.len().min(8)]
            )
        })?;
        Ok(Outcome::Measured(format!("runs of {runs:?} bytes")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_continues_from_0xff_to_0x00() {
        assert_eq!(runs(&[0xFE, 0xFF, 0x00, 0x01]), [4]);
        assert_eq!(runs(&[1, 2, 7, 8, 9]), [2, 3]);
        assert_eq!(runs(&[5, 5]), [1, 1]);
        assert!(runs(&[]).is_empty());
    }
}
