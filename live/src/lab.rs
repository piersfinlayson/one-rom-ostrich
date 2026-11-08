//! One ROM Lab's console, with Lab acting as the ECU.
//!
//! Lab's `u` command sends and receives on the UART wired to One ROM's
//! datalog pins, and `x` reads the chip in Lab's socket. Both are documented
//! in One ROM Lab's README.

use std::fmt;
use std::time::Instant;

use crate::consts::{DATALOG_BAUD, LAB_BAUD, LAB_POLL, LAB_QUIET, LAB_REPLY_DEADLINE};
use crate::port::{self, Port, Which};

const PROMPT: &str = "> ";
const ERROR: &str = "Error: ";
const CTRL_C: &[u8] = b"\x03";

/// Lab's `u` modes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Lab sends 0x00 to 0xFF repeating and checks it receives the same
    Pattern,
    /// Lab sends back each byte it receives
    Echo,
}

impl Mode {
    fn arg(self) -> &'static str {
        match self {
            Mode::Pattern => "pattern",
            Mode::Echo => "echo",
        }
    }
}

/// The counts `U` prints when it stops `u`.
#[derive(Debug, Clone, PartialEq)]
pub struct UartCounts {
    pub sent: u64,
    pub received: u64,
    pub gaps: u64,
    /// The line Lab printed
    pub line: String,
}

impl fmt::Display for UartCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lab: {}", self.line.trim())
    }
}

/// The result `x` prints when it stops.
#[derive(Debug, Clone, PartialEq)]
pub struct FastRead {
    pub passes: u64,
    pub mismatches: u64,
    /// The lines Lab printed after it stopped
    pub lines: Vec<String>,
}

pub struct Lab {
    port: Port,
}

impl Lab {
    /// Open the console of the Lab with USB serial number `serial`.
    pub fn connect(serial: &str) -> Result<Self, String> {
        let name = port::find(serial, Which::Port1).map_err(|e| format!("Lab: {e}"))?;
        // Raising DTR starts Lab's console session
        let port = Port::open_with(&name, LAB_BAUD, true)?;
        let mut lab = Self { port };
        lab.reset()?;
        Ok(lab)
    }

    /// Cancel any command at Lab's prompt, including `x`, and stop any `u`.
    pub fn reset(&mut self) -> Result<(), String> {
        self.port.drain(LAB_QUIET)?;
        self.port.send(CTRL_C)?;
        self.port.drain(LAB_QUIET)?;
        match self.command("U") {
            Err(e) if !e.contains("isn't running") => Err(e),
            _ => Ok(()),
        }
    }

    /// Run `line` at Lab's prompt and return what Lab printed before the
    /// next prompt, without the echoed command.
    pub fn command(&mut self, line: &str) -> Result<String, String> {
        self.port.send(format!("{line}\r").as_bytes())?;
        self.reply(line)
    }

    /// Read until Lab's prompt and fail on an error line.
    fn reply(&mut self, line: &str) -> Result<String, String> {
        let deadline = Instant::now() + LAB_REPLY_DEADLINE;
        let mut text = String::new();
        while !text.ends_with(PROMPT) {
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "Lab didn't finish {line}. It printed: {:?}",
                    text.trim()
                ));
            }
            let more = self.port.read_until(4096, (now + LAB_POLL).min(deadline))?;
            text.push_str(&String::from_utf8_lossy(&more));
        }
        let output = text
            .strip_suffix(PROMPT)
            .unwrap_or(&text)
            .split_once('\n')
            .map_or("", |(_echo, rest)| rest);
        match output.lines().find(|l| l.starts_with(ERROR)) {
            Some(error) => Err(format!("Lab {line}: {}", error.trim())),
            None => Ok(output.to_string()),
        }
    }

    /// Start `u` on `sel_a` and `sel_b` at the datalog's baud rate.
    pub fn start_uart(&mut self, mode: Mode) -> Result<(), String> {
        let line = format!("u:sel_a:sel_b:{DATALOG_BAUD}:{}", mode.arg());
        self.command(&line).map(|_| ())
    }

    /// Stop `u` and return its counts.
    pub fn stop_uart(&mut self) -> Result<UartCounts, String> {
        parse_counts(&self.command("U")?)
    }

    /// Start `x` on `chip` with `delay` cycles. Lab prints nothing more until
    /// `stop_fast_read`.
    pub fn start_fast_read(&mut self, chip: &str, delay: u32) -> Result<(), String> {
        self.port.send(format!("x:{chip}:{delay}\r").as_bytes())?;
        Ok(())
    }

    /// Stop `x` with a key and return its result.
    pub fn stop_fast_read(&mut self) -> Result<FastRead, String> {
        self.port.send(b" ")?;
        parse_fast_read(&self.reply("x")?)
    }
}

/// The number after `key` in a line of Lab's counts.
fn count(line: &str, key: &str) -> Option<u64> {
    let mut words = line.split_whitespace();
    words.find(|w| *w == key)?;
    words.next()?.parse().ok()
}

/// The counts in `U`'s output.
pub fn parse_counts(output: &str) -> Result<UartCounts, String> {
    let line = output
        .lines()
        .find(|l| l.contains(" sent "))
        .ok_or_else(|| format!("U didn't print counts: {:?}", output.trim()))?;
    let get = |key| count(line, key).ok_or_else(|| format!("U's counts lack {key}: {line}"));
    Ok(UartCounts {
        sent: get("sent")?,
        received: get("received")?,
        gaps: get("gaps")?,
        line: line.to_string(),
    })
}

/// The result in `x`'s output: the status line after `Stopped.`
pub fn parse_fast_read(output: &str) -> Result<FastRead, String> {
    let lines: Vec<String> = output
        .lines()
        .skip_while(|l| l.trim() != "Stopped.")
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    let status = lines
        .first()
        .ok_or_else(|| format!("x didn't stop: {:?}", output.trim()))?;
    let get = |key| count(status, key).ok_or_else(|| format!("x's result lacks {key}: {status}"));
    Ok(FastRead {
        passes: get("passes")?,
        mismatches: get("mismatches")?,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The format of `U` and `x`, from One ROM Lab's README.
    const U: &str = "Stopped.\r\n  32.8s  sent 126050  received 115140  gaps 3  framing 0  \
                     parity 0  break 0  overrun 0  lost 0\r\n\r\n";

    const X: &str = "Fast reading 27256, read delay 32 cycles. Press any key to stop.\r\n  \
                     Reference SHA1: 4a8a2c6ee8829e8ae3cee645f0875e9edc197e83\r\n  \
                     1.0s  passes 80  reads 2.6M  2.53MHz  mismatches 0\r\n\
                     Stopped.\r\n  1.2s  passes 96  reads 3.1M  2.53MHz  mismatches 2  D1 2\r\n  \
                     Pass 7 address 0x0100: expected 0x12, read 0x10\r\n\r\n";

    #[test]
    fn the_uart_counts_are_read() {
        let counts = parse_counts(U).unwrap();
        assert_eq!(
            (counts.sent, counts.received, counts.gaps),
            (126050, 115140, 3)
        );
        assert!(parse_counts("Stopped.\r\n").is_err());
    }

    #[test]
    fn the_fast_read_result_is_the_line_after_stopped() {
        let read = parse_fast_read(X).unwrap();
        assert_eq!((read.passes, read.mismatches), (96, 2));
        assert_eq!(read.lines.len(), 2);
        assert!(parse_fast_read("  1.0s  passes 80  mismatches 0\r\n").is_err());
    }
}
