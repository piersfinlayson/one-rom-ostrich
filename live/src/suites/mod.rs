//! The tests, and the device as they drive it.

use std::thread;
use std::time::Instant;

use crate::check;
use crate::consts::{LATENCY, PORT_SEARCH_INTERVAL, REBOOT_DEADLINE, SAVE_MAX, SAVE_QUIET};
use crate::lab::Lab;
use crate::onerom::Onerom;
use crate::port::{self, Port, Reply, Which};
use crate::probe::Probe;
use crate::proto::VERSION;

mod commands;
mod datalog;
mod interrupt;
mod mapping;
mod save;
mod staging;
mod timing;

pub enum Outcome {
    Pass,
    /// Passed, with the value measured
    Measured(String),
    /// Couldn't be tested on this host or device, and why
    Skip(String),
}

pub type TestFn = fn(&mut Ctx) -> Result<Outcome, String>;

pub struct Test {
    /// `suite.test`, selected with `--test`
    pub name: &'static str,
    /// The assertion, displayed by `--list` and on failure
    pub about: &'static str,
    pub run: TestFn,
    /// The reason that is displayed when the test is skipped
    pub opt_in: Option<&'static str>,
}

pub const fn test(name: &'static str, about: &'static str, run: TestFn) -> Test {
    Test {
        name,
        about,
        run,
        opt_in: None,
    }
}

/// A test that is only run when identified by `--test`.  `why` is displayed
/// when it is skipped.
pub const fn opt_in_test(
    name: &'static str,
    about: &'static str,
    why: &'static str,
    run: TestFn,
) -> Test {
    Test {
        name,
        about,
        run,
        opt_in: Some(why),
    }
}

/// Every test, in the order they run.
pub fn all() -> impl Iterator<Item = &'static Test> {
    [
        commands::TESTS,
        mapping::TESTS,
        staging::TESTS,
        save::TESTS,
        timing::TESTS,
        interrupt::TESTS,
        datalog::TESTS,
    ]
    .into_iter()
    .flatten()
}

/// The tests `names` select: each is a test's name or a suite's. Every test
/// where `names` is empty.
pub fn select(names: &[String]) -> Result<Vec<&'static Test>, String> {
    if let Some(unknown) = names.iter().find(|n| !all().any(|t| matches(t.name, n))) {
        return Err(format!("no test or suite is called {unknown}"));
    }
    Ok(all()
        .filter(|t| names.is_empty() || names.iter().any(|n| matches(t.name, n)))
        .collect())
}

fn matches(test: &str, name: &str) -> bool {
    test == name || test.split('.').next() == Some(name)
}

/// One ROM, running the Ostrich plugin, and the tools that read it.
pub struct Ctx {
    pub serial: String,
    pub onerom: Onerom,
    pub probe: Probe,
    /// The Lab acting as the ECU, where `--lab` is set
    pub lab: Option<Lab>,
    port: Option<Port>,
    /// The chip ID NS replies with
    pub chip_id: [u8; 8],
    pub chip_size: u32,
    /// The served image as last read, until an edit
    image: Option<Vec<u8>>,
    /// When the last edit was sent. A save may follow it.
    last_edit: Instant,
}

impl Ctx {
    pub fn connect(
        serial: &str,
        onerom: Onerom,
        probe: Probe,
        lab: Option<Lab>,
    ) -> Result<Self, String> {
        let chip_id = onerom.chip_id()?;
        let port = Port::open(&port::find(serial, Which::Port1)?)?;
        let mut ctx = Self {
            serial: serial.to_string(),
            onerom,
            probe,
            lab,
            port: Some(port),
            chip_id,
            chip_size: 0,
            image: None,
            // An earlier run may have left a save to come
            last_edit: Instant::now(),
        };
        ctx.chip_size = ctx.image()?.len() as u32;
        check::check(
            ctx.chip_size.is_power_of_two() && ctx.chip_size <= 0x10000,
            || format!("the plugin doesn't support a {}-byte image", ctx.chip_size),
        )?;
        Ok(ctx)
    }

    pub fn port(&mut self) -> Result<&mut Port, String> {
        self.port
            .as_mut()
            .ok_or_else(|| "the serial port is closed".to_string())
    }

    /// Send `request` and check the reply is `want`, byte for byte, with
    /// nothing after it.
    pub fn expect(&mut self, what: &str, request: &[u8], want: &[u8]) -> Result<Reply, String> {
        let reply = self.port()?.exchange(request, want.len())?;
        check::reply(want, &reply.bytes, &reply.trailing).map_err(|e| format!("{what}: {e}"))?;
        Ok(reply)
    }

    /// `expect` for a request that changes the served image.
    pub fn edit(&mut self, what: &str, request: &[u8], want: &[u8]) -> Result<Reply, String> {
        self.image = None;
        let reply = self.expect(what, request, want);
        self.last_edit = Instant::now();
        reply
    }

    /// The served image, read with `onerom peek`.
    pub fn image(&mut self) -> Result<Vec<u8>, String> {
        if self.image.is_none() {
            self.image = Some(self.onerom.peek()?);
        }
        Ok(self.image.clone().unwrap())
    }

    /// The bytes a read from Ostrich address `addr` replies with. Addresses
    /// wrap at 64KB and at the chip size.
    pub fn expected(&mut self, addr: u32, len: usize) -> Result<Vec<u8>, String> {
        let image = self.image()?;
        let mask = self.chip_size - 1;
        Ok((0..len as u32)
            .map(|i| image[(((addr + i) & 0xFFFF) & mask) as usize])
            .collect())
    }

    /// Wait for any save the last edit started to finish.
    pub fn quiesce(&self) {
        let done = self.last_edit + SAVE_QUIET + SAVE_MAX + LATENCY;
        thread::sleep(done.saturating_duration_since(Instant::now()));
    }

    /// Reboot One ROM and reopen its serial port once the plugin answers.
    pub fn reboot(&mut self) -> Result<(), String> {
        self.port = None;
        self.onerom.reboot()?;
        self.reconnect()
    }

    /// Reopen One ROM's serial port once the plugin answers, after a reset.
    pub fn reconnect(&mut self) -> Result<(), String> {
        self.port = None;
        self.image = None;
        let deadline = Instant::now() + REBOOT_DEADLINE;
        let mut last_err;
        loop {
            match port::find(&self.serial, Which::Port1).and_then(|n| Port::open(&n)) {
                Ok(port) => {
                    self.port = Some(port);
                    match self.expect("VV after the reboot", b"VV", &VERSION) {
                        Ok(_) => return Ok(()),
                        Err(e) => last_err = e,
                    }
                }
                Err(e) => last_err = e,
            }
            if Instant::now() >= deadline {
                return Err(last_err);
            }
            thread::sleep(PORT_SEARCH_INTERVAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_is_selected_by_its_name_or_its_suites() {
        let one = select(&["commands.version".to_string()]).unwrap();
        assert_eq!(one.len(), 1);
        let suite = select(&["save".to_string()]).unwrap();
        assert!(!suite.is_empty() && suite.iter().all(|t| t.name.starts_with("save.")));
        assert_eq!(select(&[]).unwrap().len(), all().count());
        assert!(select(&["commands.nonesuch".to_string()]).is_err());
        assert!(select(&["command".to_string()]).is_err());
    }

    #[test]
    fn test_names_are_unique_and_dotted() {
        let names: Vec<&str> = all().map(|t| t.name).collect();
        for (i, name) in names.iter().enumerate() {
            assert_eq!(name.matches('.').count(), 1, "{name}");
            assert!(!names[i + 1..].contains(name), "{name} repeats");
        }
    }
}
