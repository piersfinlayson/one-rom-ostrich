// Types the host shim shares with the Rust harness.  Each struct is mirrored
// field for field in src/ffi.rs.

#if !defined(OSTRICH_SHIM_H)
#define OSTRICH_SHIM_H

#include <stdint.h>

// The pair slots and the NV sector the flash model holds.
#define OSTRICH_SHIM_REGIONS     3u
#define OSTRICH_SHIM_REGION_NV   2u

#define OSTRICH_SHIM_OP_ERASE    1u
#define OSTRICH_SHIM_OP_PROGRAM  2u

#define OSTRICH_SHIM_PHASE_MID   1u
#define OSTRICH_SHIM_PHASE_AFTER 2u

// Counts of the flash calls a save made, and the faults found in them.
typedef struct {
    uint32_t ops;
    uint32_t windows;
    uint32_t connect_calls;
    uint32_t select_xip_calls;
    uint32_t flush_calls;
    uint32_t last_clkdiv;
    uint32_t last_xip_mode;
    uint32_t xip_active;
    uint32_t exclusive_depth;
    // Non-zero where a flash call arrived outside exclusive mode
    uint32_t bad_outside_exclusive;
    // Non-zero where a flash call arrived with interrupts unmasked
    uint32_t bad_unmasked;
    // Non-zero where an erase or program arrived with XIP active
    uint32_t bad_xip_active;
    // Non-zero where an erase or program fell outside the modelled regions,
    // or an erase wasn't whole 4KB sectors
    uint32_t bad_range;
    // Non-zero where the staged routine's copy didn't match the original
    uint32_t bad_staged_copy;
    uint32_t op_log_count;
    uint32_t cut_count;
} ostrich_flash_state_t;

// One erase or program
typedef struct {
    uint8_t  kind;
    uint8_t  region;
    uint16_t window;
    uint32_t offs;
    uint32_t count;
} ostrich_flash_op_t;

#define OSTRICH_SHIM_OP_LOG_MAX 512u

// The flash as a power cut would leave it halfway through an erase or program,
// and once it completes.  Each slot is recorded as a hash of its contents.
typedef struct {
    uint32_t op;
    uint8_t  kind;
    uint8_t  phase;
    uint8_t  nv_boot_slot;
    uint8_t  reserved;
    uint64_t slot_hash[2];
} ostrich_cut_t;

#define OSTRICH_SHIM_CUT_MAX 1024u

// A plugin slot's region and the header fields the firmware reads from it
typedef struct {
    uint32_t region;
    uint32_t region_size;
    uint8_t  overrides1;
    uint8_t  properties1;
    uint32_t boot_slot_addr;
    uint16_t channel_2_size;
    uint16_t channel_3_size;
    uint32_t peer_data;
    uint16_t min_fw_version[3];
} ostrich_plugin_slot_t;

// What the plugin did with the datalog's UART, pins and DMA channel
typedef struct {
    uint32_t uart_starts;
    uint32_t uart;
    uint32_t ibrd;
    uint32_t fbrd;
    uint32_t pins;
    uint32_t pin_gpio[2];
    uint32_t pin_funcsel[2];
    uint32_t pin_rx[2];
    uint32_t dma_starts;
    uint32_t dma_ch;
    uint32_t dma_uart;
    uint32_t ring_addr;
    uint32_t ring_bits;
    uint32_t dma_count;
    uint32_t dma_remaining;
    // Bytes in the TX FIFO
    uint32_t tx_level;
    // Non-zero where a pin was connected or the DMA channel started before
    // the UART
    uint32_t bad_order;
    // Non-zero where a call used a UART or DMA channel that wasn't started
    uint32_t bad_target;
    // Non-zero where a byte was written to a full TX FIFO
    uint32_t bad_tx_overflow;
} ostrich_datalog_hw_t;

// Plugin API values a scenario checks against
typedef struct {
    uint32_t user_plugin_base;
    uint32_t log_channel_2_max_size;
    uint32_t log_channel_3_max_size;
    uint32_t clk_peri_hz;
} ostrich_ora_values_t;

#endif // OSTRICH_SHIM_H
