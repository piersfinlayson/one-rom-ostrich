//! Declarations for the host shim, `csrc/ostrich_shim.c`, and the plugin's
//! own functions a scenario calls.
//!
//! The structs mirror `csrc/ostrich_shim.h` and `src/datalog.h` field for
//! field.

pub const REGION_NV: u32 = 2;

pub const OP_ERASE: u8 = 1;
pub const OP_PROGRAM: u8 = 2;

pub const PHASE_MID: u8 = 1;
pub const PHASE_AFTER: u8 = 2;

pub const OP_LOG_MAX: usize = 512;
pub const CUT_MAX: usize = 1024;

/// Mirrors `ostrich_flash_state_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FlashState {
    pub ops: u32,
    pub windows: u32,
    pub connect_calls: u32,
    pub select_xip_calls: u32,
    pub flush_calls: u32,
    pub last_clkdiv: u32,
    pub last_xip_mode: u32,
    pub xip_active: u32,
    pub exclusive_depth: u32,
    pub bad_outside_exclusive: u32,
    pub bad_unmasked: u32,
    pub bad_xip_active: u32,
    pub bad_range: u32,
    pub bad_staged_copy: u32,
    pub op_log_count: u32,
    pub cut_count: u32,
}

/// Mirrors `ostrich_flash_op_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlashOp {
    pub kind: u8,
    pub region: u8,
    pub window: u16,
    pub offs: u32,
    pub count: u32,
}

/// Mirrors `ostrich_cut_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Cut {
    pub op: u32,
    pub kind: u8,
    pub phase: u8,
    pub nv_boot_slot: u8,
    pub reserved: u8,
    pub slot_hash: [u64; 2],
}

/// Mirrors `ostrich_plugin_slot_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PluginSlot {
    pub region: u32,
    pub region_size: u32,
    pub overrides1: u8,
    pub properties1: u8,
    pub boot_slot_addr: u32,
    pub channel_2_size: u16,
    pub channel_3_size: u16,
    pub peer_data: u32,
    pub min_fw_version: [u16; 3],
}

/// Mirrors `ostrich_datalog_hw_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DatalogHw {
    pub uart_starts: u32,
    pub uart: u32,
    pub ibrd: u32,
    pub fbrd: u32,
    pub pins: u32,
    pub pin_gpio: [u32; 2],
    pub pin_funcsel: [u32; 2],
    pub pin_rx: [u32; 2],
    pub dma_starts: u32,
    pub dma_ch: u32,
    pub dma_uart: u32,
    pub ring_addr: u32,
    pub ring_bits: u32,
    pub dma_count: u32,
    pub dma_remaining: u32,
    pub tx_level: u32,
    pub bad_order: u32,
    pub bad_target: u32,
    pub bad_tx_overflow: u32,
}

/// Mirrors `ostrich_ora_values_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct OraValues {
    pub user_plugin_base: u32,
    pub log_channel_2_max_size: u32,
    pub log_channel_3_max_size: u32,
    pub clk_peri_hz: u32,
}

/// Mirrors `datalog_pins_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DatalogPins {
    pub uart: u8,
    pub tx_gpio: u8,
    pub tx_funcsel: u8,
    pub rx_gpio: u8,
    pub rx_funcsel: u8,
}

unsafe extern "C" {
    pub fn ostrich_host_test_plugin_slot(index: u8, out: *mut PluginSlot);
    pub fn ostrich_host_test_reset();
    pub fn ostrich_host_test_clear_logs();
    pub fn ostrich_host_test_model_flash() -> u32;
    pub fn ostrich_host_test_slot_addr(slot: u8, addr: *mut u32, size: *mut u32) -> i32;
    pub fn ostrich_host_test_set_nv_boot_slot(slot: u8);
    pub fn ostrich_host_test_flash_state() -> *const FlashState;
    pub fn ostrich_host_test_op_log() -> *const FlashOp;
    pub fn ostrich_host_test_cuts() -> *const Cut;
    pub fn ostrich_host_test_region(index: u32, offs: *mut u32, size: *mut u32) -> *const u8;
    pub fn ostrich_host_test_hash(data: *const u8, len: u32) -> u64;
    pub fn ostrich_host_test_corrupt_program(nth: u32);
    pub fn ostrich_host_test_fail_exclusive(count: u32);
    pub fn ostrich_host_test_fail_nv_program(fail: u8);
    pub fn ostrich_host_test_override_slot(slot: u8, addr: u32, size: u32);
    pub fn ostrich_host_test_override_ram_slot_count(count: u8);
    pub fn ostrich_host_test_set_entry_flags(flags: u8);
    pub fn ostrich_host_test_ora_values(out: *mut OraValues);
    pub fn ostrich_host_test_peer_data() -> *const u32;
    pub fn ostrich_host_test_control_port_desc() -> *const core::ffi::c_char;
    pub fn ostrich_host_test_datalog_port_desc() -> *const core::ffi::c_char;
    pub fn ostrich_host_test_override_x_pins(x1: u8, x2: u8);
    pub fn ostrich_host_test_ecu_send(data: *const u8, len: u32) -> u32;
    pub fn ostrich_host_test_ecu_send_at_sample(data: *const u8, len: u32, nth: u32);
    pub fn ostrich_host_test_ecu_send_at_erase(data: *const u8, len: u32);
    pub fn ostrich_host_test_ecu_receive(buf: *mut u8, max: u32) -> u32;
    pub fn ostrich_host_test_datalog_hw() -> *const DatalogHw;

    pub fn datalog_pins(gpio_a: u8, gpio_b: u8, out: *mut DatalogPins) -> bool;
    pub fn datalog_lost() -> u32;
}
