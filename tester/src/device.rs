//! The device as a scenario drives it.
//!
//! A scenario sends host bytes, collects replies, moves time on and inspects
//! the served image and the flash model.  For the datalog it sends bytes from
//! the ECU and the PC and collects what reaches each.  It doesn't call the
//! emulator or the shim itself.

use onerom_fw_emulator::{Emulator, OraResult};
use onerom_plugin_tester::harness::Plugin;

use crate::ffi::{self, Cut, DatalogHw, FlashOp, FlashState};
use crate::{SYSTEM, USER};

/// Channel the USB plugin writes host bytes to
pub const CH_IN: u32 = 1;
/// Channel the plugin writes replies to
pub const CH_OUT: u32 = 0;
/// Channel the plugin writes ECU bytes to, sent to the PC on serial port 2
pub const CH_TO_PC: u32 = 2;
/// Channel the USB plugin writes the PC's serial port 2 bytes to
pub const CH_FROM_PC: u32 = 3;

/// Passes of the plugin's main loop without progress before a wait fails.
const STALL_PASSES: u32 = 10_000;

/// Passes without output before the plugin counts as idle.
const QUIET_PASSES: u32 = 4;

pub struct Device<'a> {
    emu: &'a Emulator,
    plugin: &'a Plugin,
    active_ram: u8,
    /// Reply bytes read from the channel and not yet handed to the scenario
    pending: Vec<u8>,
    /// Drain the reply channel after each pass, as the USB plugin does
    draining: bool,
    /// ECU bytes read from channel 2 and not yet handed to the scenario
    to_pc: Vec<u8>,
    /// Drain channel 2 after each pass, as the USB plugin does
    pc_reading: bool,
    passes: u32,
}

impl<'a> Device<'a> {
    pub fn new(emu: &'a Emulator, plugin: &'a Plugin) -> Self {
        emu.set_calling_plugin(SYSTEM);
        let active_ram = emu.get_active_ram_slot().1.unwrap_or(0);
        emu.set_calling_plugin(USER);
        Device {
            emu,
            plugin,
            active_ram,
            pending: Vec::new(),
            draining: true,
            to_pc: Vec::new(),
            pc_reading: true,
            passes: 0,
        }
    }

    /// Call the firmware as the USB plugin.
    fn as_usb<R>(&self, f: impl FnOnce(&Emulator) -> R) -> R {
        self.emu.set_calling_plugin(SYSTEM);
        let r = f(self.emu);
        self.emu.set_calling_plugin(USER);
        r
    }

    /// Run the plugin to its next yield, then read what it replied.
    pub fn step(&mut self) -> Result<(), String> {
        self.passes += 1;
        self.plugin
            .resume(&format!("main loop pass {}", self.passes))?;
        if self.draining {
            self.drain();
        }
        if self.pc_reading {
            self.drain_to_pc();
        }
        Ok(())
    }

    pub fn step_n(&mut self, n: u32) -> Result<(), String> {
        for _ in 0..n {
            self.step()?;
        }
        Ok(())
    }

    /// Stop or restart reading the reply channel, as does a host that stops
    /// reading the serial port.
    pub fn set_draining(&mut self, draining: bool) {
        self.draining = draining;
    }

    fn drain(&mut self) {
        loop {
            let (r, bytes) = self.as_usb(|emu| emu.log_read(CH_OUT, 4096));
            if !r.is_ok() || bytes.is_empty() {
                return;
            }
            self.pending.extend_from_slice(&bytes);
        }
    }

    fn drain_to_pc(&mut self) {
        loop {
            let (r, bytes) = self.as_usb(|emu| emu.log_read(CH_TO_PC, 4096));
            if !r.is_ok() || bytes.is_empty() {
                return;
            }
            self.to_pc.extend_from_slice(&bytes);
        }
    }

    /// (free, pending) bytes in a channel.
    pub fn channel(&self, channel: u32) -> (u32, u32) {
        let (_, _, free, pending) = self.as_usb(|emu| emu.log_query(channel));
        (free, pending)
    }

    /// Queue host bytes, running the plugin whenever the channel is full.
    pub fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut rest = bytes;
        let mut stalled = 0u32;
        while !rest.is_empty() {
            let (free, _) = self.channel(CH_IN);
            let n = rest.len().min(free as usize);
            if n == 0 {
                stalled += 1;
                if stalled > STALL_PASSES {
                    return Err("the plugin stopped reading host bytes".to_string());
                }
                self.step()?;
                continue;
            }
            stalled = 0;
            let r = self.as_usb(|emu| emu.log_write(CH_IN, &rest[..n]));
            if r != OraResult::Ok {
                return Err(format!("writing host bytes: {r:?}"));
            }
            rest = &rest[n..];
        }
        Ok(())
    }

    /// Run the plugin until it has read every host byte and gone quiet.
    pub fn settle(&mut self) -> Result<(), String> {
        let mut quiet = 0u32;
        let mut passes = 0u32;
        while quiet < QUIET_PASSES {
            let before = self.pending.len();
            let (_, waiting) = self.channel(CH_IN);
            self.step()?;
            quiet = if waiting == 0 && self.pending.len() == before {
                quiet + 1
            } else {
                0
            };
            passes += 1;
            if passes > STALL_PASSES {
                return Err("the plugin didn't go quiet".to_string());
            }
        }
        Ok(())
    }

    /// Run the plugin until `len` reply bytes have arrived, and return them.
    pub fn reply(&mut self, len: usize) -> Result<Vec<u8>, String> {
        let mut stalled = 0u32;
        while self.pending.len() < len {
            let before = self.pending.len();
            let (_, waiting) = self.channel(CH_IN);
            self.step()?;
            let (_, waiting_after) = self.channel(CH_IN);
            if self.pending.len() == before && waiting_after == waiting {
                stalled += 1;
                if stalled > STALL_PASSES {
                    return Err(format!(
                        "waiting for {len} reply bytes, got {}: {:02X?}",
                        self.pending.len(),
                        &self.pending[..self.pending.len().min(16)]
                    ));
                }
            } else {
                stalled = 0;
            }
        }
        Ok(self.pending.drain(..len).collect())
    }

    /// Send a request and return its reply, which must be `len` bytes with
    /// nothing after it.
    pub fn transact(&mut self, request: &[u8], len: usize) -> Result<Vec<u8>, String> {
        self.send(request)?;
        let reply = self.reply(len)?;
        self.settle()?;
        if !self.pending.is_empty() {
            return Err(format!(
                "{} bytes after a {len}-byte reply: {:02X?}",
                self.pending.len(),
                &self.pending[..self.pending.len().min(16)]
            ));
        }
        Ok(reply)
    }

    /// Send a request and check its reply.
    pub fn expect(&mut self, what: &str, request: &[u8], want: &[u8]) -> Result<(), String> {
        let got = self.transact(request, want.len())?;
        if got != want {
            let n = got.iter().zip(want).position(|(a, b)| a != b).unwrap_or(0);
            return Err(format!(
                "{what}: reply differs at byte {n} of {}: got {:02X?}, want {:02X?}",
                want.len(),
                &got[n..(n + 16).min(got.len())],
                &want[n..(n + 16).min(want.len())]
            ));
        }
        Ok(())
    }

    /// Every reply byte received so far and not yet returned.
    pub fn take(&mut self) -> Vec<u8> {
        self.drain();
        std::mem::take(&mut self.pending)
    }

    /// Move the plugin's clock on.
    pub fn advance_ms(&mut self, ms: u64) {
        self.emu.advance_timer_us(ms * 1000);
    }

    /// Logical bytes of the served image.
    pub fn served(&self, offset: u32, len: usize) -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; len];
        let r = self.as_usb(|emu| emu.read_ram_rom_slot(self.active_ram, offset, &mut buf));
        if r != OraResult::Ok {
            return Err(format!("reading the served image at 0x{offset:X}: {r:?}"));
        }
        Ok(buf)
    }

    /// The RP2350 chip ID as the firmware reports it.
    pub fn chip_id(&self) -> Result<[u8; 8], String> {
        let mut id = [0u8; 8];
        for (i, b) in id.iter_mut().enumerate() {
            let (r, v) = self.as_usb(|emu| {
                emu.get_metadata_uint_at(
                    onerom_fw_emulator::ffi::ora_metadata_key_t_ORA_METADATA_KEY_CHIP_ID,
                    i as u32,
                )
            });
            *b = v.ok_or_else(|| format!("chip ID byte {i}: {r:?}"))? as u8;
        }
        Ok(id)
    }

    /// The served RAM slot's bytes in the order flash holds them.
    pub fn served_physical(&self) -> Vec<u8> {
        let (_, info) = self.as_usb(|emu| emu.get_ram_slot_info(self.active_ram));
        let info = info.expect("the served RAM slot has info");
        let p = self.emu.sram_host_ptr(info.addr) as *const u8;
        // SAFETY: the slot lies within the emulated SRAM and nothing else runs.
        unsafe { std::slice::from_raw_parts(p, info.size as usize) }.to_vec()
    }

    pub fn flash_state(&self) -> FlashState {
        // SAFETY: the shim's state is read while the plugin is parked.
        unsafe { *ffi::ostrich_host_test_flash_state() }
    }

    pub fn flash_ops(&self) -> Vec<FlashOp> {
        let n = (self.flash_state().op_log_count as usize).min(ffi::OP_LOG_MAX);
        // SAFETY: the log holds at least `n` entries.
        unsafe { std::slice::from_raw_parts(ffi::ostrich_host_test_op_log(), n) }.to_vec()
    }

    pub fn cuts(&self) -> Vec<Cut> {
        let n = (self.flash_state().cut_count as usize).min(ffi::CUT_MAX);
        // SAFETY: the log holds at least `n` entries.
        unsafe { std::slice::from_raw_parts(ffi::ostrich_host_test_cuts(), n) }.to_vec()
    }

    pub fn clear_flash_logs(&self) {
        // SAFETY: the plugin is parked.
        unsafe { ffi::ostrich_host_test_clear_logs() };
    }

    /// A modelled flash region's contents.
    pub fn region(&self, index: u32) -> Vec<u8> {
        let (mut offs, mut size) = (0u32, 0u32);
        // SAFETY: the outputs outlive the call and the region is `size` bytes.
        unsafe {
            let p = ffi::ostrich_host_test_region(index, &mut offs, &mut size);
            if p.is_null() {
                return Vec::new();
            }
            std::slice::from_raw_parts(p, size as usize).to_vec()
        }
    }

    pub fn nv_boot_slot(&self) -> u8 {
        self.region(ffi::REGION_NV)[0]
    }
}

impl Device<'_> {
    /// Stop or restart reading channel 2, as does a PC that stops reading
    /// serial port 2.
    pub fn set_pc_reading(&mut self, reading: bool) {
        self.pc_reading = reading;
    }

    /// Every ECU byte the PC has received and not yet returned.
    pub fn take_to_pc(&mut self) -> Vec<u8> {
        if self.pc_reading {
            self.drain_to_pc();
        }
        std::mem::take(&mut self.to_pc)
    }

    /// Write the PC's bytes to channel 3, as many as fit.  Returns how many.
    pub fn pc_send(&self, bytes: &[u8]) -> Result<usize, String> {
        let (free, _) = self.channel(CH_FROM_PC);
        let n = bytes.len().min(free as usize);
        if n == 0 {
            return Ok(0);
        }
        let r = self.as_usb(|emu| emu.log_write(CH_FROM_PC, &bytes[..n]));
        if r != OraResult::Ok {
            return Err(format!("writing the PC's bytes: {r:?}"));
        }
        Ok(n)
    }

    /// Send ECU bytes into the UART.  Returns how many the DMA channel took,
    /// 0 if it isn't running.
    pub fn ecu_send(&self, bytes: &[u8]) -> usize {
        // SAFETY: `bytes` outlives the call and the plugin is parked.
        unsafe { ffi::ostrich_host_test_ecu_send(bytes.as_ptr(), bytes.len() as u32) as usize }
    }

    /// Send ECU bytes as the plugin reads the DMA channel's count for the nth
    /// time from now.
    pub fn ecu_send_at_sample(&self, bytes: &[u8], nth: u32) {
        // SAFETY: the shim copies `bytes`, and the plugin is parked.
        unsafe {
            ffi::ostrich_host_test_ecu_send_at_sample(bytes.as_ptr(), bytes.len() as u32, nth)
        };
    }

    /// Send ECU bytes during the plugin's next flash erase.
    pub fn ecu_send_at_erase(&self, bytes: &[u8]) {
        // SAFETY: the shim copies `bytes`, and the plugin is parked.
        unsafe { ffi::ostrich_host_test_ecu_send_at_erase(bytes.as_ptr(), bytes.len() as u32) };
    }

    /// The bytes the UART has sent to the ECU since the last call.
    pub fn ecu_receive(&self) -> Vec<u8> {
        let mut buf = vec![0u8; 64];
        // SAFETY: `buf` is 64 bytes and the plugin is parked.
        let n = unsafe { ffi::ostrich_host_test_ecu_receive(buf.as_mut_ptr(), 64) };
        buf.truncate(n as usize);
        buf
    }

    pub fn datalog_hw(&self) -> DatalogHw {
        // SAFETY: the shim's state is read while the plugin is parked.
        unsafe { *ffi::ostrich_host_test_datalog_hw() }
    }

    pub fn datalog_lost(&self) -> u32 {
        // SAFETY: the plugin is parked.
        unsafe { ffi::datalog_lost() }
    }

    /// A log channel's capacity.
    pub fn channel_size(&self, channel: u32) -> u32 {
        self.as_usb(|emu| emu.log_query(channel)).1
    }

    /// Runtime info's UART claims, as `onerom_hw_claim_t` bits.
    pub fn hw_claims(&self) -> u8 {
        self.emu.hw_claims()
    }

    /// Runtime info's DMA channel claims, bit N for channel N.
    pub fn dma_channel_claims(&self) -> u16 {
        self.emu.dma_channel_claims()
    }

    /// The RAM slot count the firmware reports.
    pub fn ram_slot_count(&self) -> u8 {
        self.as_usb(|emu| emu.get_ram_slot_count())
    }

    /// A RAM slot's address and size.
    pub fn ram_slot(&self, slot: u8) -> Option<(u32, u32)> {
        let (_, info) = self.as_usb(|emu| emu.get_ram_slot_info(slot));
        info.map(|info| (info.addr, info.size))
    }

    pub fn active_ram_slot(&self) -> Option<u8> {
        self.as_usb(|emu| emu.get_active_ram_slot()).1
    }

    /// What One ROM uses a GPIO for, as an `ora_gpio_use_t`.
    pub fn gpio_use(&self, gpio: u8) -> Option<u8> {
        let (r, info) = self.as_usb(|emu| emu.gpio_query(gpio));
        r.is_ok().then_some(info.gpio_use)
    }
}

/// The hash the shim records flash with.
pub fn hash(data: &[u8]) -> u64 {
    // SAFETY: `data` outlives the call.
    unsafe { ffi::ostrich_host_test_hash(data.as_ptr(), data.len() as u32) }
}
