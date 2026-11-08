//! Times and addresses the tests check against, each with its source.

use std::time::Duration;

/// A value build.rs copied out of the plugin's or One ROM's source.
macro_rules! extracted {
    ($name:literal) => {
        include!(concat!(env!("OUT_DIR"), "/", $name))
    };
}

const fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// ---------------------------------------------------------------------------
// The plugin, from src/ostrich_main.c
// ---------------------------------------------------------------------------

/// A partly received command is discarded after this long without a byte.
/// `CMD_TIMEOUT_MS`.
pub const CMD_TIMEOUT: Duration = ms(extracted!("CMD_TIMEOUT_MS"));

/// A reply the host stops reading is dropped after this long without progress.
/// `REPLY_TIMEOUT_MS`.
pub const REPLY_TIMEOUT: Duration = ms(extracted!("REPLY_TIMEOUT_MS"));

/// The served image is saved this long after the last edit. `SAVE_QUIET_MS`.
pub const SAVE_QUIET: Duration = ms(extracted!("SAVE_QUIET_MS"));

/// `NV_STORAGE_SIZE`, the flash at the end of the user plugin region.
pub const NV_STORAGE_SIZE: u32 = extracted!("NV_STORAGE_SIZE");

/// `NV_BOOT_SLOT`, the boot slot byte's offset in NV storage.
pub const NV_BOOT_SLOT: u32 = extracted!("NV_BOOT_SLOT");

/// A save erases and writes one sector of this many bytes at a time.
/// `FLASH_SECTOR_SIZE`.
pub const FLASH_SECTOR_SIZE: u32 = extracted!("FLASH_SECTOR_SIZE");

/// The description of the datalog's USB serial port, One ROM's serial port 2.
/// `datalog_port_desc`.
pub const DATALOG_PORT_DESC: &str = extracted!("datalog_port_desc");

// ---------------------------------------------------------------------------
// The plugin, from src/datalog.c
// ---------------------------------------------------------------------------

/// The datalog's fixed baud rate. `DATALOG_BAUD`.
pub const DATALOG_BAUD: u32 = extracted!("DATALOG_BAUD");

/// The datalog's bytes a second, at 8N1's 10 bits a byte.
pub const DATALOG_LINE_RATE: u32 = DATALOG_BAUD / 10;

// ---------------------------------------------------------------------------
// The plugin API, from firmware/ora/onerom_constants_generated.h
// ---------------------------------------------------------------------------

/// One ROM's USB VID and PID while it runs. `ORA_USB_PLUGIN_VID` and
/// `ORA_USB_PLUGIN_PID`.
pub const USB_PLUGIN_VID: u16 = extracted!("ORA_USB_PLUGIN_VID");
pub const USB_PLUGIN_PID: u16 = extracted!("ORA_USB_PLUGIN_PID");

/// The address of flash offset 0. `ORA_FLASH_CS0_BASE_ADDR`.
pub const FLASH_BASE: u32 = extracted!("ORA_FLASH_CS0_BASE_ADDR");

/// The plugin's NV storage, as its `NV_STORAGE_ADDR` computes it.
pub const NV_STORAGE_ADDR: u32 =
    FLASH_BASE + extracted!("ORA_USER_PLUGIN_OFFSET") + extracted!("ORA_USER_PLUGIN_SIZE")
        - NV_STORAGE_SIZE;

/// The boot slot byte's address.
pub const NV_BOOT_SLOT_ADDR: u32 = NV_STORAGE_ADDR + NV_BOOT_SLOT;

// ---------------------------------------------------------------------------
// The firmware and the RP2350
// ---------------------------------------------------------------------------

/// The core the user plugin runs on. `ora_calling_plugin` in One ROM's
/// `firmware/src/plugin.c`.
pub const USER_PLUGIN_CORE: usize = 0;

/// The address of the bootrom's 16-bit pointer to its function table.
/// `BOOTROM_FUNC_TABLE_OFFSET` in pico-sdk's `boot/bootrom_constants.h`.
pub const BOOTROM_TABLE_PTR: u32 = 0x14;

/// The RP2350 bootrom's size, from the RP2350 datasheet's address map.
pub const BOOTROM_SIZE: u32 = 0x8000;

/// The bootrom table flag of a function's Arm secure entry point, the one the
/// plugin looks up. `RT_FLAG_FUNC_ARM_SEC` in pico-sdk's
/// `boot/bootrom_constants.h`.
pub const BOOTROM_FLAG_ARM_SEC: u16 = 0x0004;

/// The codes of the bootrom functions the plugin writes flash with, from
/// pico-sdk's `boot/bootrom_constants.h` and `flash_write` in the plugin's
/// `src/ostrich_main.c`.
pub const BOOTROM_RANGE_ERASE: [u8; 2] = *b"RE";
pub const BOOTROM_RANGE_PROGRAM: [u8; 2] = *b"RP";
pub const BOOTROM_SELECT_XIP: [u8; 2] = *b"XM";

// ---------------------------------------------------------------------------
// The One ROM CLI
// ---------------------------------------------------------------------------

/// How long the CLI retries a device another program has open.
/// `OPEN_RETRY_LIMIT` in `rust/cli/src/lib/usb.rs`.
pub const OPEN_RETRY_LIMIT: Duration = ms(extracted!("OPEN_RETRY_LIMIT"));

/// The interval between `onerom monitor log`'s reads of a log in RAM. `POLL`
/// in `rust/cli/src/lib/ram_log.rs`.
pub const LOG_POLL: Duration = ms(extracted!("POLL"));

// ---------------------------------------------------------------------------
// Other sources
// ---------------------------------------------------------------------------

/// ORSS fails a command whose reply is slower than this.
pub const ORSS_REPLY_TIMEOUT: Duration = ms(400);

/// The longest 4KB sector erase, tSE in the W25Q16JV datasheet.
pub const SECTOR_ERASE_MAX: Duration = ms(400);

/// The longest 256-byte page program, tPP in the W25Q16JV datasheet.
pub const PAGE_PROGRAM_MAX: Duration = ms(3);

/// The longest erase and program of one 4KB sector.
pub const SECTOR_WRITE_MAX: Duration =
    ms(SECTOR_ERASE_MAX.as_millis() as u64 + 16 * PAGE_PROGRAM_MAX.as_millis() as u64);

/// The longest a save lasts: every sector of the largest image the plugin
/// supports, 64KB, then the boot slot byte's sector.
pub const SAVE_MAX: Duration = ms(17 * SECTOR_WRITE_MAX.as_millis() as u64);

// ---------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------

/// Allowance for USB transfers and host scheduling between an event on the
/// device and the harness seeing it.
pub const LATENCY: Duration = ms(20);

/// How far short of and past `CMD_TIMEOUT` a command is cut off, giving
/// 400ms and 600ms.
pub const CUT_MARGIN: Duration = ms(100);

/// The longest wait for a whole reply. It covers a 64KB reply, the plugin's
/// reply timeout and USB pausing for one sector's erase and program.
pub const REPLY_DEADLINE: Duration = ms(3000);

/// How long the port is watched for stray bytes after a complete reply.
pub const TRAILING_WINDOW: Duration = ms(50);

/// How long a 64KB reply is left unread.
pub const UNREAD_WAIT: Duration = ms(2000);

/// The longest time between the plugin logging a line and `onerom monitor log`
/// printing it: one poll interval, and as long again for the poll itself.
pub const LOG_LATENCY: Duration = ms(2 * LOG_POLL.as_millis() as u64);

/// How long `onerom monitor log` runs after its first line on stderr before
/// its output counts. It has opened the log and read it once by then.
pub const LOG_START: Duration = ms(3 * LOG_POLL.as_millis() as u64);

/// The longest wait for `onerom monitor log` to start.
pub const LOG_START_DEADLINE: Duration = ms(10_000);

/// The interval between searches of the captured log.
pub const LOG_CHECK_INTERVAL: Duration = ms(10);

/// How long after a save starts the edit that abandons it is sent. A sector
/// erase typically lasts 45ms, tSE in the W25Q16JV datasheet, so the save is
/// still running then.
pub const MID_SAVE_DELAY: Duration = ms(30);

/// The longest wait for One ROM's serial port and the plugin to return after
/// `onerom reboot`.
pub const REBOOT_DEADLINE: Duration = ms(15_000);

/// The interval between searches for the serial port after a reboot.
pub const PORT_SEARCH_INTERVAL: Duration = ms(250);

/// The interval between reads of a core's halt state while a breakpoint is
/// armed.
pub const HALT_POLL: Duration = ms(1);

/// The longest wait for a core to halt when asked to.
pub const HALT_TIMEOUT: Duration = ms(100);

/// Allowance added to a save's longest time for the breakpoint stops before
/// the one a test waits for. Each stop is a few probe transfers.
pub const STOP_ALLOWANCE: Duration = ms(2000);

/// How long `onerom monitor log` is watched for its holds of picoboot.
pub const HOLD_WINDOW: Duration = ms(5000);

/// The fewest holds in a valid measurement. `HOLD_WINDOW` covers about 50 polls
/// and a poll while the harness holds picoboot is skipped.
pub const HOLD_MIN_SEEN: usize = 10;

/// The interval between the harness's claims of picoboot. Short against a
/// poll's hold so the hold is measured to within a few milliseconds.
pub const HOLD_PROBE_INTERVAL: Duration = ms(2);

/// The interval between the incomplete commands sent while holds are measured.
/// The plugin logs each one it discards, so each poll has log bytes to read.
pub const HOLD_LOG_INTERVAL: Duration = ms(1000);

// ---------------------------------------------------------------------------
// The datalog suite, with One ROM Lab
// ---------------------------------------------------------------------------

/// Lab's console baud rate, from One ROM Lab's README.
pub const LAB_BAUD: u32 = 115_200;

/// The longest wait for Lab to print a command's reply. `U` waits for Lab's
/// UART to send what it holds.
pub const LAB_REPLY_DEADLINE: Duration = ms(3000);

/// How long Lab's console is read for after its last byte, to clear it.
pub const LAB_QUIET: Duration = ms(300);

/// How often a wait for Lab's prompt checks what has arrived.
pub const LAB_POLL: Duration = ms(20);

/// How long each datalog test sends traffic.
pub const DATALOG_RUN: Duration = ms(10_000);

/// How long the datalog port is read for at a time.
pub const DATALOG_POLL: Duration = ms(10);

/// How long Lab's traffic runs after the PC stops sending, so Lab receives
/// the PC's last bytes before it stops.
pub const DATALOG_TAIL: Duration = ms(300);

/// How long the datalog port is read for after its last byte, once Lab has
/// stopped.
pub const DATALOG_QUIET: Duration = ms(300);

/// The lowest rate the PC receives Lab's pattern at, as a percentage of the
/// line rate.
pub const DATALOG_MIN_RATE_PERCENT: u32 = 95;

/// When the edit is sent after Lab's traffic starts, in the save test.
pub const DATALOG_EDIT_AT: Duration = ms(1000);

/// Lab's `x` read delay in 150MHz cycles. At 16 cycles a read rarely fails
/// while the datalog runs.
pub const LAB_READ_DELAY: u32 = 32;

/// How long the datalog port stays closed while Lab sends, in the late open
/// test.
pub const DATALOG_CLOSED: Duration = ms(5000);

/// How long the datalog port is read for after the late open.
pub const DATALOG_LATE_READ: Duration = ms(3000);

const _: () = {
    assert!(CUT_MARGIN.as_millis() < CMD_TIMEOUT.as_millis());
    assert!(REPLY_DEADLINE.as_millis() > REPLY_TIMEOUT.as_millis() + SECTOR_WRITE_MAX.as_millis());
    assert!(UNREAD_WAIT.as_millis() > REPLY_TIMEOUT.as_millis());
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boot_slot_byte_is_where_the_plugin_map_puts_nv_storage() {
        // __nv_storage_start in build/ostrich_plugin.map
        assert_eq!(NV_BOOT_SLOT_ADDR, 0x1002_F000);
    }
}
