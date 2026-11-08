//! Ostrich plugin tester
//!
//! Runs the Ostrich plugin's C source natively against the One ROM firmware
//! emulator.  The test driver acts as the USB plugin.  It writes host
//! bytes to log channel 1 and reads the plugin's replies from channel 0.  For
//! the datalog it reads channel 2 and writes channel 3.  A shim models the
//! flash the plugin saves to, and the UART and DMA channel of the datalog.
//!
//! # Environment variables
//!
//! | Variable     | Required | Description                                              |
//! |--------------|----------|----------------------------------------------------------|
//! | `BOARD`      | yes      | Board name, for example `fire-28-a`                      |
//! | `CONFIG`     | yes      | Firmware config JSON, absolute or relative to `BASE_DIR` |
//! | `BASE_DIR`   | no       | One ROM checkout, `build/one-rom` by default             |
//! | `ONEROM_LOG` | no       | `1` prints firmware and plugin logging                   |
//! | `RUST_LOG`   | no       | Tester log level (default: `warn`)                       |
//!
//! # Arguments
//!
//! `--suite <name>` runs one suite and `--scenario <substr>` runs scenarios
//! whose name contains the substring.  Both default to everything.
//!
//! Exits 0 if every scenario that ran passed, 1 otherwise.

use onerom_config::hw::Board;
use onerom_fw_emulator::{Emulator, ORA_FLASH_SLOT_FLAG_EXCLUDE_PLUGINS, ffi as fw};
use onerom_metadata::onerom_firmware_flag_t;
use onerom_plugin_tester::harness::Plugin;
use onerom_plugin_tester::run::{Filters, Outcome, Tally, suite_header};

mod device;
mod ffi;
mod proto;
mod suites;

use device::Device;

pub const SYSTEM: fw::ora_plugin_type_t = fw::ora_plugin_type_t_ORA_PLUGIN_TYPE_SYSTEM;
pub const USER: fw::ora_plugin_type_t = fw::ora_plugin_type_t_ORA_PLUGIN_TYPE_USER;

/// The two ROM slots the plugin saves between, and the device around them, as
/// the tester works them out from the firmware.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub chip_size: u32,
    pub region_size: u32,
    /// The pair slot the firmware booted
    pub booted: u8,
    /// Each pair slot's offset from the start of flash
    pub slot_offs: [u32; 2],
}

/// Facts about the device, read before the plugin starts.
pub struct Ctx {
    pub board: Board,
    /// The layout, or why the device is unsupported
    pub expect: Result<Layout, String>,
}

impl Ctx {
    /// The layout, or a skip for a configuration the plugin doesn't support.
    pub fn layout(&self) -> Result<Layout, Outcome> {
        self.expect
            .clone()
            .map_err(|why| Outcome::Skip(format!("configuration unsupported: {why}")))
    }
}

/// The plugin's boot slot byte at boot.  The firmware boots the slot it holds,
/// or slot 0.
#[derive(Clone, Copy)]
pub struct Boot {
    pub nv: u8,
}

pub const BOOT_DEFAULT: Boot = Boot { nv: 0xFF };

pub type ScenarioFn = fn(&mut Device, &Ctx) -> Result<Outcome, String>;

pub struct Scenario {
    /// Dotted name, filtered on by `--scenario`
    pub name: &'static str,
    /// The scenario's assertion, printed when it fails
    pub about: &'static str,
    pub run: ScenarioFn,
    pub boot: Boot,
    /// Run after the shim is reset and before the plugin starts, so the plugin
    /// starts on the device the scenario arranged.
    pub before_start: Option<fn()>,
}

pub struct Suite {
    pub name: &'static str,
    pub blurb: &'static str,
    pub scenarios: &'static [Scenario],
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let filters = Filters::from_args();

    let board_str = std::env::var("BOARD").expect("BOARD must be set, for example fire-28-a");
    let board =
        Board::try_from_str(&board_str).unwrap_or_else(|| panic!("Unknown board '{board_str}'"));
    let config = std::env::var("CONFIG").expect("CONFIG must be set");
    let log_enabled = std::env::var("ONEROM_LOG").is_ok_and(|v| v == "1");

    let mut tally = Tally::new();
    for suite in suites::SUITES {
        if !filters.suite_matches(suite.name) {
            continue;
        }
        let selected: Vec<&Scenario> = suite
            .scenarios
            .iter()
            .filter(|s| filters.scenario_matches(s.name))
            .collect();
        if selected.is_empty() {
            continue;
        }

        suite_header(suite.name, suite.blurb);
        for sc in selected {
            let result = run_scenario(sc, board, log_enabled);
            tally.record(sc.name, sc.about, result);
        }
    }

    tally.finish("ostrich", &format!("{board_str} / {config}"));
}

/// Boot the firmware, start the plugin and run one scenario against it.
///
/// Every scenario gets a fresh boot, a fresh flash model and a fresh entry
/// into the plugin.
fn run_scenario(sc: &Scenario, board: Board, log_enabled: bool) -> Result<Outcome, String> {
    let addrs = [0u8, 1].map(Emulator::rom_slot_flash_addr);
    install_plugins(sc.boot.nv);
    let result = boot_and_run(sc, board, log_enabled);
    // The plugin slots and the flash byte outlive a boot in this process.
    Emulator::restore_rom_slots();
    Emulator::clear_flash();
    for (index, addr) in (0u8..).zip(addrs) {
        Emulator::set_rom_slot_flash_addr(index, addr);
    }
    result
}

/// Make flash slots 0 and 1 a USB plugin and a plugin with the Ostrich plugin's
/// header fields.  Each is in its region and `nv` is in the boot slot byte.
/// The firmware boots the slot the byte holds, starts both plugins with
/// exclusive serial IN and creates log channels 2 and 3.
///
/// A host build reads a plugin header with the host's struct layout, where the
/// entry point is 8 bytes, so the configs hold stub plugins and the headers are
/// set here.
fn install_plugins(nv: u8) {
    Emulator::overlay_plugin_slots();
    for index in 0..2u8 {
        let mut slot = ffi::PluginSlot::default();
        // SAFETY: `slot` outlives the call.
        unsafe { ffi::ostrich_host_test_plugin_slot(index, &mut slot) };
        let entry = slot.region + slot.region_size - 1;
        Emulator::set_plugin_header(index, entry, slot.overrides1, slot.properties1);
        Emulator::set_plugin_boot_slot_addr(index, slot.boot_slot_addr);
        Emulator::set_plugin_channel_sizes(index, slot.channel_2_size, slot.channel_3_size);
        Emulator::set_plugin_peer_data(index, slot.peer_data);
        let [major, minor, patch] = slot.min_fw_version;
        Emulator::set_plugin_min_fw_version(index, major, minor, patch);
        Emulator::set_rom_slot_flash_addr(index, slot.region);
        if index == 1 {
            Emulator::clear_flash();
            Emulator::set_flash_byte(slot.boot_slot_addr, nv);
        }
    }
}

fn boot_and_run(sc: &Scenario, board: Board, log_enabled: bool) -> Result<Outcome, String> {
    Emulator::set_logging(log_enabled);
    Emulator::set_rp_variant(board.rp_variant());
    Emulator::set_sel_image(0);
    let mut emu = Emulator::boot();
    if emu.limp_mode() {
        return Err("firmware entered limp mode".to_string());
    }
    if !Emulator::user_plugin_to_start() {
        return Err("the firmware doesn't start the plugin".to_string());
    }
    let flags = Emulator::plugin_entry_flags();
    if flags & fw::ORA_ENTRY_FLAG_EXCLUSIVE_SERIAL_IN as u8 == 0 {
        return Err(format!(
            "entry flags 0x{flags:02X} lack exclusive serial IN"
        ));
    }

    // Booting runs plugin launch until the plugins start, which moves the log and
    // creates channels 2 and 3.
    if emu.firmware_flags() & onerom_firmware_flag_t::FIRMWARE_FLAG_LOG_IN_UP1 == 0 {
        return Err("plugin launch didn't move the log".to_string());
    }

    emu.set_calling_plugin(SYSTEM);

    // The driver is the USB plugin.  It reads replies from channel 0 and writes
    // host bytes to channel 1, and for the datalog reads channel 2 and writes
    // channel 3.  A boot doesn't clear the channels or their claims, so
    // channels 1 and 3 are emptied first.
    emu.reset_log_claims();
    for channel in [device::CH_IN, device::CH_FROM_PC] {
        if emu.log_open_read(channel).is_ok() {
            while !emu.log_read(channel, 4096).1.is_empty() {}
            let _ = emu.log_close_read(channel);
        }
    }
    if !emu.log_open_read(device::CH_OUT).is_ok() {
        return Err("could not claim channel 0 as the USB plugin".to_string());
    }
    while !emu.log_read(device::CH_OUT, 4096).1.is_empty() {}
    if !emu.log_open_write(device::CH_IN, c"usb").is_ok() {
        return Err("could not claim channel 1 as the USB plugin".to_string());
    }
    if !emu.log_open_read(device::CH_TO_PC).is_ok() {
        return Err("could not claim channel 2 as the USB plugin".to_string());
    }
    while !emu.log_read(device::CH_TO_PC, 4096).1.is_empty() {}
    if !emu.log_open_write(device::CH_FROM_PC, c"usb-in-2").is_ok() {
        return Err("could not claim channel 3 as the USB plugin".to_string());
    }

    let mut pins = [0u8; 16];
    let word_size = emu.get_data_pin_nums(&mut pins, 16);
    emu.setup_epio(word_size);

    // SAFETY: the shim's state is process-global and the plugin isn't running.
    unsafe {
        ffi::ostrich_host_test_reset();
        ffi::ostrich_host_test_model_flash();
        ffi::ostrich_host_test_set_nv_boot_slot(sc.boot.nv);
        ffi::ostrich_host_test_set_entry_flags(flags);
    }

    let ctx = Ctx {
        board,
        expect: expected_layout(&emu),
    };

    if let Some(before_start) = sc.before_start {
        before_start();
    }

    emu.set_calling_plugin(USER);
    // SAFETY: `emu` outlives `plugin`, which is dropped at the end of this fn.
    let plugin = unsafe { Plugin::start(&emu)? };
    let mut device = Device::new(&emu, &plugin);
    (sc.run)(&mut device, &ctx)
}

/// The expected layout, or why the device is unsupported, read from the
/// firmware.
fn expected_layout(emu: &Emulator) -> Result<Layout, String> {
    let only_roms = ORA_FLASH_SLOT_FLAG_EXCLUDE_PLUGINS;

    if emu.get_ram_slot_count() < 2 {
        return Err("one RAM slot".to_string());
    }
    let (_, active) = emu.get_active_ram_slot();
    let active = active.ok_or("no active RAM slot")?;
    let (_, info) = emu.get_ram_slot_info(active);
    let region_size = info.ok_or("no RAM slot info")?.size;

    let rom_slots = emu.get_flash_slot_count(only_roms);
    if rom_slots != 2 {
        return Err(format!("{rom_slots} ROM slots"));
    }

    let mut types = [0u32; 2];
    let mut chips = [0u32; 2];
    let mut slot_offs = [0u32; 2];
    for slot in 0..2u8 {
        let (_, info) = emu.get_flash_slot_info(slot, only_roms);
        let info = info.ok_or("no flash slot info")?;
        if info.rom_count != 1 {
            return Err(format!("slot {slot} holds {} ROMs", info.rom_count));
        }
        let (_, ext) = emu.get_flash_slot_ext_info(slot, 0, only_roms);
        types[slot as usize] = info.rom_type;
        chips[slot as usize] = ext.ok_or("no flash slot ext info")?.chip_size;

        let (mut addr, mut size) = (0u32, 0u32);
        // SAFETY: both outputs outlive the call.
        if unsafe { ffi::ostrich_host_test_slot_addr(slot, &mut addr, &mut size) } == 0 {
            return Err(format!("slot {slot} has no flash address"));
        }
        if addr % 4096 != 0 || size % 4096 != 0 {
            return Err(format!(
                "slot {slot} at 0x{addr:08X} size 0x{size:X} not in whole sectors"
            ));
        }
        if size != region_size {
            return Err(format!("slot {slot} size 0x{size:X} isn't the RAM slot's"));
        }
        slot_offs[slot as usize] = addr - 0x1000_0000;
    }
    if types[0] != types[1] || chips[0] != chips[1] {
        return Err("slots 0 and 1 differ".to_string());
    }
    let chip_size = chips[0];
    if chip_size > 0x10000 || !chip_size.is_power_of_two() {
        return Err(format!("chip size 0x{chip_size:X}"));
    }

    let (_, index) = emu.get_metadata_uint(fw::ora_metadata_key_t_ORA_METADATA_KEY_ROM_SLOT_INDEX);
    let index = index.ok_or("no ROM slot index")?;
    let plugins = u32::from(emu.get_flash_slot_count(0)) - u32::from(rom_slots);
    let booted = index.checked_sub(plugins).ok_or("booted a plugin slot")?;

    Ok(Layout {
        chip_size,
        region_size,
        booted: booted as u8,
        slot_offs,
    })
}
