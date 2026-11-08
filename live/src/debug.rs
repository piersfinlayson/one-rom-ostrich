//! The RP2350 through the debug probe, with the probe-rs library: memory
//! reads, hardware breakpoints and reset.

use std::thread;
use std::time::Instant;

use probe_rs::probe::DebugProbeSelector;
use probe_rs::probe::list::Lister;
use probe_rs::{Core, MemoryInterface, Permissions, Session};

use crate::consts::{
    BOOTROM_FLAG_ARM_SEC, BOOTROM_SIZE, BOOTROM_TABLE_PTR, HALT_POLL, HALT_TIMEOUT,
};
use crate::probe::CHIP;

/// A probe-rs session. Dropping it clears every breakpoint and resumes any
/// halted core.
pub struct Debugger {
    session: Session,
}

/// A core halted at a breakpoint.
#[derive(Debug, Clone, Copy)]
pub struct Stop {
    pub pc: u32,
    /// The first argument of the function the breakpoint is on
    pub r0: u32,
}

fn err(what: &str) -> impl FnOnce(probe_rs::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

impl Debugger {
    /// Attach to the RP2350 without halting it. `selector` is probe-rs's
    /// `--probe` selector, required where more than one probe is attached.
    pub fn attach(selector: Option<&str>) -> Result<Self, String> {
        let lister = Lister::new();
        let probe = match selector {
            Some(s) => {
                let selector: DebugProbeSelector =
                    s.parse().map_err(|e| format!("--probe {s}: {e}"))?;
                lister.open(selector)
            }
            None => match lister.list_all().as_slice() {
                [only] => only.open(),
                [] => return Err("no debug probe is attached".to_string()),
                _ => return Err("more than one debug probe is attached. Use --probe".to_string()),
            },
        }
        .map_err(|e| format!("opening the debug probe: {e}"))?;
        let session = probe
            .attach(CHIP, Permissions::default())
            .map_err(err("attaching to the RP2350"))?;
        Ok(Self { session })
    }

    fn core(&mut self, core: usize) -> Result<Core<'_>, String> {
        self.session
            .core(core)
            .map_err(|e| format!("core {core}: {e}"))
    }

    pub fn read(&mut self, addr: u32, len: usize) -> Result<Vec<u8>, String> {
        let mut data = vec![0; len];
        self.core(0)?
            .read(u64::from(addr), &mut data)
            .map_err(|e| format!("reading {len} bytes at 0x{addr:08X}: {e}"))?;
        Ok(data)
    }

    pub fn read_u8(&mut self, addr: u32) -> Result<u8, String> {
        Ok(self.read(addr, 1)?[0])
    }

    /// The address of the bootrom function `code`'s Arm secure entry point.
    pub fn bootrom_function(&mut self, code: [u8; 2]) -> Result<u32, String> {
        let ptr = self.read(BOOTROM_TABLE_PTR, 2)?;
        let table = u32::from(u16::from_le_bytes([ptr[0], ptr[1]]));
        let bytes = self.read(table, BOOTROM_SIZE.saturating_sub(table) as usize)?;
        let words: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&w| u16::from_le_bytes(w))
            .collect();
        let entry = bootrom_lookup(&words, u16::from_le_bytes(code), BOOTROM_FLAG_ARM_SEC)
            .ok_or_else(|| {
                format!(
                    "the bootrom table at 0x{table:04X} doesn't list {}",
                    String::from_utf8_lossy(&code)
                )
            })?;
        // The low bit selects Thumb state. A breakpoint is on the halfword.
        Ok(u32::from(entry) & !1)
    }

    pub fn break_at(&mut self, core: usize, addr: u32) -> Result<(), String> {
        self.core(core)?
            .set_hw_breakpoint(u64::from(addr))
            .map_err(|e| format!("breakpoint at 0x{addr:08X}: {e}"))
    }

    pub fn clear_breakpoints(&mut self, core: usize) -> Result<(), String> {
        self.core(core)?
            .clear_all_hw_breakpoints()
            .map_err(|e| format!("clearing core {core}'s breakpoints: {e}"))
    }

    /// Wait until `deadline` for `core` to halt at a breakpoint.
    pub fn wait_stop(&mut self, core: usize, deadline: Instant) -> Result<Stop, String> {
        loop {
            let mut c = self.core(core)?;
            if c.core_halted().map_err(err("reading the halt state"))? {
                let pc_reg = c.program_counter();
                let r0_reg = c.registers().argument_register(0);
                let pc: u32 = c.read_core_reg(pc_reg).map_err(err("reading PC"))?;
                let r0: u32 = c.read_core_reg(r0_reg).map_err(err("reading r0"))?;
                return Ok(Stop { pc, r0 });
            }
            if Instant::now() >= deadline {
                return Err(format!("core {core} didn't stop at a breakpoint"));
            }
            thread::sleep(HALT_POLL);
        }
    }

    pub fn resume(&mut self, core: usize) -> Result<(), String> {
        self.core(core)?.run().map_err(err("resuming"))
    }

    /// Halt every core still running.
    pub fn halt_all(&mut self) -> Result<(), String> {
        for core in self.cores() {
            let mut c = self.core(core)?;
            if !c.core_halted().map_err(err("reading the halt state"))? {
                c.halt(HALT_TIMEOUT).map_err(err("halting"))?;
            }
        }
        Ok(())
    }

    /// Reset the whole RP2350 and let it run. The flash chip stays powered.
    pub fn reset(&mut self) -> Result<(), String> {
        for core in self.cores() {
            self.clear_breakpoints(core)?;
        }
        self.core(0)?.reset().map_err(err("resetting the RP2350"))
    }

    fn cores(&self) -> Vec<usize> {
        self.session
            .list_cores()
            .into_iter()
            .map(|(i, _)| i)
            .collect()
    }
}

impl Drop for Debugger {
    fn drop(&mut self) {
        for core in self.cores() {
            let _ = self.clear_breakpoints(core);
            if let Ok(mut c) = self.session.core(core)
                && c.core_halted().unwrap_or(false)
            {
                let _ = c.run();
            }
        }
    }
}

/// The value with `flag` of function `code` in a bootrom table. An entry is
/// its code, its flags, then a halfword for each flag set, in bit order. A
/// code of 0 ends the table.
pub fn bootrom_lookup(table: &[u16], code: u16, flag: u16) -> Option<u16> {
    let mut i = 0;
    while let (Some(&c), Some(&flags)) = (table.get(i), table.get(i + 1)) {
        if c == 0 {
            return None;
        }
        if c == code {
            if flags & flag == 0 {
                return None;
            }
            let before = (flags & (flag - 1)).count_ones() as usize;
            return table.get(i + 2 + before).copied();
        }
        i += 2 + flags.count_ones() as usize;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Entries of an RP2350 bootrom's table, read with probe-rs, then the end.
    const TABLE: &[u16] = &[
        0x5253, 0x0005, 0xb4a1, 0x28cd, 0x5347, 0x0017, 0xf76f, 0x83df, 0x0bb1, 0x7e19, 0x4353,
        0x0010, 0x7e39, 0x4552, 0x0007, 0xf76f, 0xfc6f, 0x0f15, 0x5052, 0x0007, 0xf76f, 0xfbcf,
        0x0edd, 0x4256, 0x0004, 0x7fd5, 0x5247, 0x0040, 0x7df0, 0x0000, 0x0000,
    ];

    #[test]
    fn a_function_is_found_past_entries_of_every_width() {
        let code = |c: &[u8; 2]| u16::from_le_bytes(*c);
        assert_eq!(bootrom_lookup(TABLE, code(b"RE"), 0x0004), Some(0x0f15));
        assert_eq!(bootrom_lookup(TABLE, code(b"RP"), 0x0004), Some(0x0edd));
        assert_eq!(bootrom_lookup(TABLE, code(b"VB"), 0x0004), Some(0x7fd5));
        assert_eq!(bootrom_lookup(TABLE, code(b"GS"), 0x0010), Some(0x7e19));
    }

    #[test]
    fn a_missing_function_or_flag_is_none() {
        assert_eq!(bootrom_lookup(TABLE, u16::from_le_bytes(*b"ZZ"), 4), None);
        assert_eq!(bootrom_lookup(TABLE, u16::from_le_bytes(*b"CS"), 4), None);
        assert_eq!(
            bootrom_lookup(&TABLE[..3], u16::from_le_bytes(*b"RP"), 4),
            None
        );
    }
}
