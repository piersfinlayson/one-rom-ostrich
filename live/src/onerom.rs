//! The One ROM CLI, run as a subprocess against one device.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::consts::{LOG_CHECK_INTERVAL, LOG_START, LOG_START_DEADLINE};

pub struct Onerom {
    path: PathBuf,
    serial: String,
    board: String,
    scratch: PathBuf,
}

impl Onerom {
    pub fn new(path: PathBuf, serial: &str, board: &str) -> Result<Self, String> {
        let scratch = std::env::temp_dir().join(format!("ostrich-live-{}", std::process::id()));
        std::fs::create_dir_all(&scratch)
            .map_err(|e| format!("creating {}: {e}", scratch.display()))?;
        Ok(Self {
            path,
            serial: serial.to_string(),
            board: board.to_string(),
            scratch,
        })
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.path);
        command.args(["--serial", &self.serial]).args(args);
        command
    }

    /// Run `onerom` with `args` and return its stdout.
    pub fn run(&self, args: &[&str]) -> Result<String, String> {
        let shown = format!("onerom {}", args.join(" "));
        let output = self
            .command(args)
            .output()
            .map_err(|e| format!("{shown}: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "{shown} failed, {}: {}",
                output.status,
                stderr.trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Run `onerom` with `args` and `--output`, and return the file it wrote.
    fn run_to_file(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let file = self.scratch.join("out.bin");
        let _ = std::fs::remove_file(&file);
        let file_arg = file.to_string_lossy();
        let mut args = args.to_vec();
        args.extend(["--output", &file_arg]);
        self.run(&args)?;
        std::fs::read(&file).map_err(|e| format!("reading {}: {e}", file.display()))
    }

    /// The device's chip ID, most significant byte first. Fails unless the
    /// device is running and is a `--board` board.
    pub fn chip_id(&self) -> Result<[u8; 8], String> {
        let out = self.run(&["--verbose", "scan", "--board", &self.board])?;
        parse_chip_id(&out, &self.serial)
    }

    /// The served image, read with `onerom peek`.
    pub fn peek(&self) -> Result<Vec<u8>, String> {
        self.run_to_file(&["peek"])
    }

    /// The image in ROM slot `slot`'s flash.
    pub fn save_slot(&self, slot: u8) -> Result<Vec<u8>, String> {
        self.run_to_file(&["save", "--slot", &slot.to_string()])
    }

    /// The ROM slot One ROM selected at boot.
    pub fn selected_slot(&self) -> Result<u8, String> {
        parse_selected_slot(&self.run(&["inspect", "slots"])?)
    }

    /// Where the served image and the ROM slots are, from `onerom inspect info`.
    pub fn layout(&self) -> Result<Layout, String> {
        parse_layout(&self.run(&["inspect", "info"])?)
    }

    /// The served chip type, from `onerom inspect info`.
    pub fn chip_type(&self) -> Result<String, String> {
        parse_chip_type(&self.run(&["inspect", "info"])?)
    }

    pub fn reboot(&self) -> Result<(), String> {
        self.run(&["reboot"]).map(|_| ())
    }

    /// Start `onerom monitor log`.
    pub fn log(&self) -> Result<LogCapture, String> {
        LogCapture::start(self.command(&["monitor", "log"]))
    }
}

impl Drop for Onerom {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// The chip ID from `onerom --verbose scan` output, from the lines beneath
/// the device with `serial`.
pub fn parse_chip_id(out: &str, serial: &str) -> Result<[u8; 8], String> {
    let mut lines = out.lines();
    let device = lines
        .by_ref()
        .find(|l| l.contains(&format!("Serial: {serial}")))
        .ok_or_else(|| format!("onerom scan didn't list {serial} as this board type"))?;
    if !device.contains("State: Running") {
        return Err(format!("the device isn't running: {}", device.trim()));
    }
    let hex = lines
        .take_while(|l| !l.contains("Serial: "))
        .find_map(|l| l.split("Chip ID: ").nth(1))
        .ok_or("onerom scan didn't list a chip ID")?;
    let id = u64::from_str_radix(hex.trim(), 16).map_err(|e| format!("chip ID {hex}: {e}"))?;
    Ok(id.to_be_bytes())
}

/// An address and size in One ROM's memory map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub addr: u32,
    pub size: u32,
}

/// The served image in RAM and each ROM slot in flash, in ROM slot order.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub served: Region,
    pub slots: Vec<Region>,
}

/// The layout from `onerom inspect info` output: the firmware's runtime info
/// for the served image and the slot metadata for the ROM slots. Plugin slots
/// aren't ROM slots.
pub fn parse_layout(out: &str) -> Result<Layout, String> {
    let info = info_json(out)?;
    let addr = |v: &serde_json::Value| v["Addr32"].as_u64().and_then(|a| u32::try_from(a).ok());
    let size = |v: &serde_json::Value| v.as_u64().and_then(|s| u32::try_from(s).ok());
    let runtime = &info["runtime"];
    let served = addr(&runtime["rom_table"])
        .zip(size(&runtime["rom_table_size"]))
        .map(|(addr, size)| Region { addr, size })
        .ok_or("onerom inspect info didn't list the served image's address and size")?;
    let slots = info["metadata"]["rom_slots"]
        .as_array()
        .ok_or("onerom inspect info didn't list the slots")?
        .iter()
        .filter(|slot| {
            !slot["slot_type"]
                .as_str()
                .is_some_and(|t| t.starts_with("RomSlotTypePlugin"))
        })
        .map(|slot| {
            addr(&slot["data"])
                .zip(size(&slot["size"]))
                .map(|(addr, size)| Region { addr, size })
                .ok_or_else(|| format!("a ROM slot without an address and size: {slot}"))
        })
        .collect::<Result<_, _>>()?;
    Ok(Layout { served, slots })
}

/// The JSON in `onerom inspect info` output.
fn info_json(out: &str) -> Result<serde_json::Value, String> {
    let json = out
        .find("\n{")
        .map(|i| &out[i + 1..])
        .ok_or("onerom inspect info didn't print JSON")?;
    serde_json::from_str(json).map_err(|e| format!("onerom inspect info's JSON: {e}"))
}

/// The chip type of the ROM slot One ROM serves, from `onerom inspect info`
/// output.
pub fn parse_chip_type(out: &str) -> Result<String, String> {
    info_json(out)?["runtime"]["current_rom_slot"]["roms"][0]["rom_type"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "onerom inspect info didn't list the served chip type".to_string())
}

/// The selected slot from `onerom inspect slots` output.
pub fn parse_selected_slot(out: &str) -> Result<u8, String> {
    out.split("Slot ")
        .skip(1)
        .find_map(|rest| {
            let (number, rest) = rest.split_once(' ')?;
            rest.starts_with("is selected")
                .then(|| number.parse().ok())?
        })
        .ok_or_else(|| "onerom inspect slots didn't list a selected slot".to_string())
}

/// A line of One ROM's log and when the harness read it.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub at: Instant,
    pub text: String,
}

/// `onerom monitor log`, running, with its output collected.
pub struct LogCapture {
    child: Child,
    lines: Arc<Mutex<Vec<LogLine>>>,
    reader: Option<JoinHandle<()>>,
    /// Lines before this were logged before the capture started
    pub start: usize,
}

impl LogCapture {
    fn start(mut command: Command) -> Result<Self, String> {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("onerom monitor log: {e}"))?;

        let lines = Arc::new(Mutex::new(Vec::new()));
        let stdout = child.stdout.take().expect("stdout is piped");
        let collected = lines.clone();
        let reader = thread::spawn(move || {
            for text in BufReader::new(stdout).lines() {
                let Ok(text) = text else { break };
                let line = LogLine {
                    at: Instant::now(),
                    text,
                };
                collected.lock().unwrap().push(line);
            }
        });

        // The command prints its first line on stderr just before it opens the
        // log. It is the only sign that the command has started.
        let (tx, rx) = std::sync::mpsc::channel();
        let stderr = child.stderr.take().expect("stderr is piped");
        thread::spawn(move || {
            let mut lines = BufReader::new(stderr).lines();
            let _ = tx.send(lines.next());
            for _ in lines {}
        });
        let mut capture = Self {
            child,
            lines,
            reader: Some(reader),
            start: 0,
        };
        let Ok(Some(Ok(first))) = rx.recv_timeout(LOG_START_DEADLINE) else {
            capture.stop();
            return Err("onerom monitor log didn't start".to_string());
        };
        thread::sleep(LOG_START);
        if let Ok(Some(status)) = capture.child.try_wait() {
            return Err(format!("onerom monitor log stopped, {status}: {first}"));
        }
        capture.start = capture.lines.lock().unwrap().len();
        Ok(capture)
    }

    /// Wait until `deadline` for a line from index `from` containing
    /// `text`, and return its index and the line.
    pub fn wait_for(&self, from: usize, text: &str, deadline: Instant) -> Option<(usize, LogLine)> {
        loop {
            if let Some(found) = find(&self.lines.lock().unwrap(), from, text) {
                return Some(found);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(LOG_CHECK_INTERVAL);
        }
    }

    /// Stop the command and return the lines logged since the capture started.
    pub fn stop(&mut self) -> Vec<String> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let lines = self.lines.lock().unwrap();
        lines[self.start.min(lines.len())..]
            .iter()
            .map(|l| l.text.clone())
            .collect()
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The first line from index `from` containing `text`.
pub fn find(lines: &[LogLine], from: usize, text: &str) -> Option<(usize, LogLine)> {
    lines
        .iter()
        .enumerate()
        .skip(from)
        .find(|(_, l)| l.text.contains(text))
        .map(|(i, l)| (i, l.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The format of `onerom --verbose scan`, from docs/CLI-MANUAL.md.
    const SCAN: &str = "Scanning ... \n\
        found 2 connected devices:\n  \
        One ROM Fire 28 C - Firmware: v0.8.1 State: Running Serial: 0123456789ABCDEF\n    \
        MCU: RP2354A Chip ID: 0123456789ABCDEE\n  \
        One ROM Fire 40 B - Firmware: v0.8.0 State: Stopped Serial: DE3F9C232F655B6B\n    \
        MCU: RP235xB Chip ID: DE3F9C232F655B6B\n";

    #[test]
    fn the_chip_id_is_read_most_significant_byte_first() {
        assert_eq!(
            parse_chip_id(SCAN, "0123456789ABCDEF").unwrap(),
            [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEE]
        );
    }

    #[test]
    fn a_stopped_or_absent_device_fails() {
        assert!(parse_chip_id(SCAN, "DE3F9C232F655B6B").is_err());
        assert!(parse_chip_id(SCAN, "0000000000000000").is_err());
    }

    #[test]
    fn another_devices_chip_id_is_never_used() {
        let scan = "  One ROM Fire 28 C - Firmware: v0.8.1 State: Running Serial: AAAA\n  \
                    One ROM Fire 28 C - Firmware: v0.8.1 State: Running Serial: BBBB\n    \
                    MCU: RP2354A Chip ID: 0000000000000001\n";
        assert!(parse_chip_id(scan, "AAAA").is_err());
    }

    // An excerpt of `onerom inspect info` for two ROM slots after the plugins.
    const INFO: &str = r#"One ROM Fire 28 C - Firmware: v0.8.1 State: Running Serial: 0123456789ABCDEF
Device information:
{
  "metadata": {
    "rom_slots": [
      { "data": { "Addr32": 268500992 }, "size": 65536, "slot_type": "RomSlotTypePluginSystem" },
      { "data": { "Addr32": 268566528 }, "size": 65536, "slot_type": "RomSlotTypePluginUser" },
      { "data": { "Addr32": 268632064 }, "size": 65536, "slot_type": "RomSlotTypeSingleRom" },
      { "data": { "Addr32": 268697600 }, "size": 65536, "slot_type": "RomSlotTypeSingleRom" }
    ]
  },
  "runtime": {
    "rom_table": { "Addr32": 536870912 },
    "rom_table_size": 65536,
    "current_rom_slot": { "roms": [ { "rom_type": "27256" } ] }
  }
}
"#;

    #[test]
    fn the_layout_lists_rom_slots_without_plugins() {
        let layout = parse_layout(INFO).unwrap();
        let region = |addr, size| Region { addr, size };
        assert_eq!(layout.served, region(0x2000_0000, 0x10000));
        assert_eq!(
            layout.slots,
            [region(0x1003_0000, 0x10000), region(0x1004_0000, 0x10000)]
        );
        assert!(parse_layout("Device information:\n").is_err());
    }

    #[test]
    fn the_served_chip_type_is_read() {
        assert_eq!(parse_chip_type(INFO).unwrap(), "27256");
        assert!(parse_chip_type("Device information:\n{}").is_err());
    }

    #[test]
    fn the_selected_slot_is_read() {
        let out = "  Configured with 2 slots - Slot 1 is selected (active)\n  \
                   Slot 0:\n  Slot 1 (selected, active):\n";
        assert_eq!(parse_selected_slot(out).unwrap(), 1);
        assert!(parse_selected_slot("  Configured with 2 slots\n").is_err());
    }

    #[test]
    fn a_log_search_starts_at_the_index_provided() {
        let now = Instant::now();
        let lines: Vec<LogLine> = [
            "Ostrich: saving to slot 1",
            "x",
            "Ostrich: saving to slot 1",
        ]
        .iter()
        .map(|t| LogLine {
            at: now,
            text: t.to_string(),
        })
        .collect();
        assert_eq!(find(&lines, 0, "saving to slot 1").unwrap().0, 0);
        assert_eq!(find(&lines, 1, "saving to slot 1").unwrap().0, 2);
        assert!(find(&lines, 3, "saving to slot 1").is_none());
    }
}
