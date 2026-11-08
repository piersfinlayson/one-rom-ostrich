//! The plugin's boot slot byte, read with probe-rs.

use std::path::PathBuf;
use std::process::Command;

use crate::consts::NV_BOOT_SLOT_ADDR;

/// The probe-rs target for every One ROM Fire.
pub const CHIP: &str = "RP235x";

pub struct Probe {
    pub path: PathBuf,
    /// probe-rs's `--probe` selector, where more than one probe is attached
    pub selector: Option<String>,
}

impl Probe {
    /// The boot slot byte in the plugin's NV storage.
    pub fn boot_slot_byte(&self) -> Result<u8, String> {
        let addr = format!("0x{NV_BOOT_SLOT_ADDR:08X}");
        let mut command = Command::new(&self.path);
        command.args(["read", "--chip", CHIP, "--non-interactive"]);
        if let Some(selector) = &self.selector {
            command.args(["--probe", selector]);
        }
        command.args(["b8", &addr, "1"]);
        let output = command
            .output()
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "probe-rs read of {addr} failed, {}: {}",
                output.status,
                stderr.trim()
            ));
        }
        parse_byte(&stdout)
    }
}

/// The first byte in probe-rs's hex output.
pub fn parse_byte(out: &str) -> Result<u8, String> {
    let word = out
        .split_whitespace()
        .next()
        .ok_or("probe-rs printed nothing")?;
    u8::from_str_radix(word, 16).map_err(|e| format!("probe-rs printed {word:?}: {e}"))
}

/// The ROM slot One ROM boots for a boot slot byte. The firmware boots slot 0
/// for a value that isn't a ROM slot of the pair.
pub fn boot_slot(byte: u8) -> u8 {
    if byte < 2 { byte } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_rs_output_is_read_as_hex() {
        assert_eq!(parse_byte("ff\n"), Ok(0xFF));
        assert_eq!(parse_byte("01 \n"), Ok(1));
        assert!(parse_byte("").is_err());
        assert!(parse_byte("Error").is_err());
    }

    #[test]
    fn a_byte_that_isnt_a_pair_slot_boots_slot_0() {
        assert_eq!(boot_slot(0), 0);
        assert_eq!(boot_slot(1), 1);
        assert_eq!(boot_slot(0xFF), 0);
        assert_eq!(boot_slot(2), 0);
    }
}
