// Host shim for the Ostrich plugin.
//
// The plugin is compiled natively and linked with the firmware test library.
// This file provides what the device would:
// - the symbols the plugin's linker script defines
// - the ORA host-test functions, with a model of the flash the plugin writes
// - datalog_hw.h's functions, with a model of the UART and the DMA channel
// - stand-ins for API calls a scenario varies or observes
// - the entry point the harness starts the plugin through

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <plugin.h>

#include "datalog_hw.h"
#include "flash_write.h"
#include "ostrich_shim.h"

// ---------------------------------------------------------------------------
// Firmware test library
// ---------------------------------------------------------------------------

void *ora_fn_lookup(api_id_t id);
uint8_t *sram_to_host(uint32_t addr);

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

void ostrich_main(
    ora_lookup_fn_t ora_lookup_fn,
    ora_plugin_type_t plugin_type,
    const ora_entry_args_t *entry_args
);

extern const ora_plugin_header_t ora_plugin_header;
extern const struct ostrich_peer_data peer_data;
extern const char control_port_desc[];
extern const char datalog_port_desc[];

// ---------------------------------------------------------------------------
// Linker symbols
// ---------------------------------------------------------------------------

#define SHIM_NV_STORAGE_SIZE 4096u

// NV storage's offset from the start of flash, as the plugin's linker script
// places it.
#define SHIM_NV_FLASH_OFFSET \
    (ORA_USER_PLUGIN_OFFSET + ORA_USER_PLUGIN_SIZE - SHIM_NV_STORAGE_SIZE)

uint8_t __nv_storage_start[SHIM_NV_STORAGE_SIZE];

// The staged routine is never run from its copy.  ORA_STAGED_FN_PTR returns
// the host build of flash_write_critical instead.  The copy is checked against
// this pattern.
#define SHIM_FN_SIZE 48u
_Static_assert(SHIM_FN_SIZE <= FLASH_WRITE_FN_MAX,
               "the stand-in routine must fit the plugin's buffer");

uint8_t __flash_write_fn_start[SHIM_FN_SIZE];
uint8_t __flash_write_fn_end[1];

static void fill_fn_pattern(void) {
    for (uint32_t i = 0u; i < SHIM_FN_SIZE; i++) {
        __flash_write_fn_start[i] = (uint8_t)((i * 0x9Du) ^ 0x5Bu);
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

typedef struct {
    uint32_t offs;
    uint32_t size;
    uint8_t *data;
} region_t;

static region_t s_regions[OSTRICH_SHIM_REGIONS];

static ostrich_flash_state_t s_state;
static ostrich_flash_op_t    s_ops[OSTRICH_SHIM_OP_LOG_MAX];
static ostrich_cut_t         s_cuts[OSTRICH_SHIM_CUT_MAX];

static uint32_t s_irq_depth;
static uint32_t s_corrupt_program;
static uint32_t s_programs;
static uint32_t s_fail_exclusive;
static uint8_t  s_fail_nv_program;

static bool     s_slot_override[2];
static uint32_t s_slot_override_addr[2];
static uint32_t s_slot_override_size[2];
static uint8_t  s_ram_count_override;

#define SHIM_WITHHOLD_MAX 8u
static uint32_t s_withheld[SHIM_WITHHOLD_MAX];
static uint32_t s_withheld_count;

static ora_entry_args_t s_entry_args;

static bool    s_x_pins_override;
static uint8_t s_x_pins[2];

// The datalog model's state, its TX FIFO and the ECU bytes held back for a
// later moment.
#define SHIM_TX_FIFO_DEPTH  32u
#define SHIM_ECU_LATER_MAX  4096u
#define SHIM_LATER_NONE     0u
#define SHIM_LATER_SAMPLE   1u
#define SHIM_LATER_ERASE    2u

static ostrich_datalog_hw_t s_dl;
static uint8_t  s_tx_fifo[SHIM_TX_FIFO_DEPTH];
static uint32_t s_tx_head;
static uint8_t  s_ecu_later[SHIM_ECU_LATER_MAX];
static uint32_t s_ecu_later_len;
static uint32_t s_ecu_later_when;
static uint32_t s_ecu_later_samples;

static void (*s_yield_hook)(void);

// ---------------------------------------------------------------------------
// Harness control
// ---------------------------------------------------------------------------

void ostrich_host_test_clear_logs(void) {
    memset(&s_state, 0, sizeof(s_state));
    s_state.xip_active = 1u;
    s_irq_depth = 0u;
    s_programs = 0u;
    fill_fn_pattern();
}

// Put every override and fault back to a device's defaults.  The flash model
// is emptied and NV storage erased.
void ostrich_host_test_reset(void) {
    ostrich_host_test_clear_logs();
    for (uint32_t i = 0u; i < OSTRICH_SHIM_REGION_NV; i++) {
        free(s_regions[i].data);
        s_regions[i].data = NULL;
        s_regions[i].offs = 0u;
        s_regions[i].size = 0u;
    }
    memset(__nv_storage_start, 0xFF, sizeof(__nv_storage_start));
    s_regions[OSTRICH_SHIM_REGION_NV].offs = SHIM_NV_FLASH_OFFSET;
    s_regions[OSTRICH_SHIM_REGION_NV].size = SHIM_NV_STORAGE_SIZE;
    s_regions[OSTRICH_SHIM_REGION_NV].data = __nv_storage_start;

    s_corrupt_program = 0u;
    s_fail_exclusive = 0u;
    s_fail_nv_program = 0u;
    s_slot_override[0] = false;
    s_slot_override[1] = false;
    s_ram_count_override = 0u;
    s_withheld_count = 0u;
    s_x_pins_override = false;

    memset(&s_dl, 0, sizeof(s_dl));
    s_tx_head = 0u;
    s_ecu_later_len = 0u;
    s_ecu_later_when = SHIM_LATER_NONE;

    memset(&s_entry_args, 0, sizeof(s_entry_args));
    s_entry_args.flags = ORA_ENTRY_FLAG_EXCLUSIVE_SERIAL_IN;
}

// Plugin slot 0 is a USB plugin requiring firmware 0.8.1.  Slot 1 has the
// Ostrich plugin's own header fields.
void ostrich_host_test_plugin_slot(uint8_t index, ostrich_plugin_slot_t *out) {
    memset(out, 0, sizeof(*out));
    if (index == 0u) {
        out->region = ORA_SYSTEM_PLUGIN_BASE;
        out->region_size = ORA_SYSTEM_PLUGIN_SIZE;
        out->properties1 = ORA_PROPERTY1_SUPPORTS_USB_RUNNING |
                           ORA_PROPERTY1_SUPPORTS_YIELD;
        out->min_fw_version[0] = 0u;
        out->min_fw_version[1] = 8u;
        out->min_fw_version[2] = 1u;
    } else {
        out->region = ORA_USER_PLUGIN_BASE;
        out->region_size = ORA_USER_PLUGIN_SIZE;
        out->overrides1 = ora_plugin_header.overrides1;
        out->properties1 = ora_plugin_header.properties1;
        out->boot_slot_addr = ora_plugin_header.boot_slot_addr;
        out->channel_2_size = ora_plugin_header.channel_2_size;
        out->channel_3_size = ora_plugin_header.channel_3_size;
        out->peer_data = ora_plugin_header.peer_data;
        out->min_fw_version[0] = ora_plugin_header.min_fw_major_version;
        out->min_fw_version[1] = ora_plugin_header.min_fw_minor_version;
        out->min_fw_version[2] = ora_plugin_header.min_fw_patch_version;
    }
}

void ostrich_host_test_ora_values(ostrich_ora_values_t *out) {
    out->user_plugin_base = ORA_USER_PLUGIN_BASE;
    out->log_channel_2_max_size = ORA_LOG_CHANNEL_2_MAX_SIZE;
    out->log_channel_3_max_size = ORA_LOG_CHANNEL_3_MAX_SIZE;
    ora_get_metadata_uint_fn_t get_uint = ora_fn_lookup(ORA_ID_GET_METADATA_UINT);
    out->clk_peri_hz = 0u;
    (void)get_uint(ORA_METADATA_KEY_CLK_PERI_HZ, &out->clk_peri_hz);
}

// The peer data.  On a device it is at the address in peer_data.
const uint32_t *ostrich_host_test_peer_data(void) {
    return (const uint32_t *)&peer_data;
}

// The serial ports' descriptions.  On a device they are at the addresses in the peer
// data block.
const char *ostrich_host_test_control_port_desc(void) {
    return control_port_desc;
}

const char *ostrich_host_test_datalog_port_desc(void) {
    return datalog_port_desc;
}

// The firmware's own address and size for a pair slot, ignoring overrides.
int ostrich_host_test_slot_addr(uint8_t slot, uint32_t *addr, uint32_t *size) {
    ora_get_flash_slot_addr_fn_t fw = ora_fn_lookup(ORA_ID_GET_FLASH_SLOT_ADDR);
    return fw(slot, ORA_FLASH_SLOT_FLAG_EXCLUDE_PLUGINS, addr, size) == ORA_RESULT_OK;
}

// Model the pair slots' flash with the contents the firmware has for them.
// Returns the number of slots modelled.
//
// Each slot is copied into RAM slot 1 and read from there.  Call it before the
// plugin starts, as the plugin stages writes in that slot.
uint32_t ostrich_host_test_model_flash(void) {
    ora_get_ram_slot_count_fn_t ram_count = ora_fn_lookup(ORA_ID_GET_RAM_SLOT_COUNT);
    ora_get_ram_slot_info_fn_t ram_info = ora_fn_lookup(ORA_ID_GET_RAM_SLOT_INFO);
    ora_copy_flash_slot_to_ram_slot_fn_t copy = ora_fn_lookup(ORA_ID_COPY_FLASH_SLOT_TO_RAM_SLOT);

    uint32_t modelled = 0u;
    for (uint8_t slot = 0u; slot < 2u; slot++) {
        uint32_t addr = 0u;
        uint32_t size = 0u;
        if (!ostrich_host_test_slot_addr(slot, &addr, &size)) {
            break;
        }
        region_t *r = &s_regions[slot];
        r->offs = addr - ORA_FLASH_CS0_BASE_ADDR;
        r->size = size;
        r->data = malloc(size);
        if (r->data == NULL) {
            abort();
        }
        memset(r->data, 0xFF, size);

        uint32_t ram_addr = 0u;
        uint32_t ram_size = 0u;
        if ((ram_count() >= 2u) &&
            (ram_info(1u, &ram_addr, &ram_size, NULL) == ORA_RESULT_OK) &&
            (ram_size == size) &&
            (copy(slot, ORA_FLASH_SLOT_FLAG_EXCLUDE_PLUGINS, 1u, 0u) == ORA_RESULT_OK)) {
            memcpy(r->data, sram_to_host(ram_addr), size);
        }
        modelled++;
    }
    return modelled;
}

void ostrich_host_test_set_nv_boot_slot(uint8_t slot) {
    __nv_storage_start[0] = slot;
}

const ostrich_flash_state_t *ostrich_host_test_flash_state(void) {
    return &s_state;
}

const ostrich_flash_op_t *ostrich_host_test_op_log(void) {
    return s_ops;
}

const ostrich_cut_t *ostrich_host_test_cuts(void) {
    return s_cuts;
}

const uint8_t *ostrich_host_test_region(uint32_t index, uint32_t *offs, uint32_t *size) {
    if (index >= OSTRICH_SHIM_REGIONS) {
        return NULL;
    }
    *offs = s_regions[index].offs;
    *size = s_regions[index].size;
    return s_regions[index].data;
}

// FNV-1a, 64 bits.  The harness hashes its reference images with this too.
uint64_t ostrich_host_test_hash(const uint8_t *data, uint32_t len) {
    uint64_t h = 0xCBF29CE484222325ull;
    for (uint32_t i = 0u; i < len; i++) {
        h ^= data[i];
        h *= 0x100000001B3ull;
    }
    return h;
}

// Program the nth program call's first byte wrong.  0 turns it off.
void ostrich_host_test_corrupt_program(uint32_t nth) {
    s_corrupt_program = nth;
}

// Leave NV storage unchanged by a program, as a program that fails silently
// does.  0 turns it off.
void ostrich_host_test_fail_nv_program(uint8_t fail) {
    s_fail_nv_program = fail;
}

// Fail the next count entries to exclusive mode.
void ostrich_host_test_fail_exclusive(uint32_t count) {
    s_fail_exclusive = count;
}

void ostrich_host_test_override_slot(uint8_t slot, uint32_t addr, uint32_t size) {
    if (slot < 2u) {
        s_slot_override[slot] = true;
        s_slot_override_addr[slot] = addr;
        s_slot_override_size[slot] = size;
    }
}

// 0 reports the firmware's count.
void ostrich_host_test_override_ram_slot_count(uint8_t count) {
    s_ram_count_override = count;
}

void ostrich_host_test_set_entry_flags(uint8_t flags) {
    s_entry_args.flags = flags;
}

// Report X1 and X2 as these GPIOs.
void ostrich_host_test_override_x_pins(uint8_t x1, uint8_t x2) {
    s_x_pins_override = true;
    s_x_pins[0] = x1;
    s_x_pins[1] = x2;
}

void ora_host_test_withhold_api(const uint32_t *ids, uint32_t count) {
    if (count > SHIM_WITHHOLD_MAX) {
        count = SHIM_WITHHOLD_MAX;
    }
    for (uint32_t i = 0u; i < count; i++) {
        s_withheld[i] = ids[i];
    }
    s_withheld_count = count;
}

// ---------------------------------------------------------------------------
// ORA host-test functions
// ---------------------------------------------------------------------------

void *ora_host_test_sram_ptr(uint32_t addr) {
    return sram_to_host(addr);
}

void ora_host_test_set_yield_hook(void (*hook)(void)) {
    s_yield_hook = hook;
}

void ora_host_test_yield(void) {
    if (s_yield_hook == NULL) {
        fprintf(stderr, "ostrich shim: plugin yielded without a hook installed\n");
        abort();
    }
    s_yield_hook();
}

uint32_t ora_host_test_irq_disable(void) {
    uint32_t previous = s_irq_depth;
    s_irq_depth++;
    return previous;
}

void ora_host_test_irq_restore(uint32_t primask) {
    s_irq_depth = primask;
}

// Any non-zero divisor.  The plugin must pass back the one it read.
#define SHIM_XIP_CLKDIV 4u

uint8_t ora_host_test_xip_clkdiv(void) {
    return SHIM_XIP_CLKDIV;
}

uint32_t ora_host_test_flash_offset(const void *addr) {
    if ((const uint8_t *)addr == __nv_storage_start) {
        return SHIM_NV_FLASH_OFFSET;
    }
    fprintf(stderr, "ostrich shim: ORA_FLASH_OFFSET of a pointer other than NV storage\n");
    abort();
}

uint32_t ora_host_test_staged_fn_size(const void *start, const void *end) {
    (void)start;
    (void)end;
    return SHIM_FN_SIZE;
}

void *ora_host_test_staged_fn_ptr(uintptr_t addr) {
    const uint8_t *copy = (const uint8_t *)addr;
    for (uint32_t i = 0u; i < SHIM_FN_SIZE; i++) {
        if (copy[i] != __flash_write_fn_start[i]) {
            s_state.bad_staged_copy = 1u;
            break;
        }
    }
    return (void *)flash_write_critical;
}

// ---------------------------------------------------------------------------
// Datalog model
//
// The DMA channel's copy of a received byte is modelled when the harness
// sends ECU bytes.  The TX FIFO holds what the plugin wrote until the harness
// reads it off the wire.
// ---------------------------------------------------------------------------

// The ECU bytes sent so far, modulo the DMA channel's count
static uint32_t dma_received(void) {
    return (s_dl.dma_count - s_dl.dma_remaining) % s_dl.dma_count;
}

static uint32_t ecu_deliver(const uint8_t *data, uint32_t len) {
    if (s_dl.dma_starts == 0u) {
        return 0u;
    }
    uint32_t ring_size = 1u << s_dl.ring_bits;
    for (uint32_t i = 0u; i < len; i++) {
        uint32_t pos = dma_received() % ring_size;
        *(uint8_t *)ora_host_test_sram_ptr(s_dl.ring_addr + pos) = data[i];
        s_dl.dma_remaining--;
        if (s_dl.dma_remaining == 0u) {
            s_dl.dma_remaining = s_dl.dma_count;
        }
    }
    return len;
}

// Send ECU bytes now.  Returns the number received, 0 if the datalog's DMA
// channel isn't running.
uint32_t ostrich_host_test_ecu_send(const uint8_t *data, uint32_t len) {
    return ecu_deliver(data, len);
}

static void ecu_later(const uint8_t *data, uint32_t len, uint32_t when) {
    if (len > SHIM_ECU_LATER_MAX) {
        fprintf(stderr, "ostrich shim: %u ECU bytes held back, at most %u\n",
                (unsigned)len, (unsigned)SHIM_ECU_LATER_MAX);
        abort();
    }
    memcpy(s_ecu_later, data, len);
    s_ecu_later_len = len;
    s_ecu_later_when = when;
}

static void ecu_later_arrives(uint32_t when) {
    if (s_ecu_later_when == when) {
        s_ecu_later_when = SHIM_LATER_NONE;
        (void)ecu_deliver(s_ecu_later, s_ecu_later_len);
    }
}

// Send ECU bytes as the plugin reads the DMA channel's count for the nth time
// from now, counting from 1.  They arrive before the read.
void ostrich_host_test_ecu_send_at_sample(const uint8_t *data, uint32_t len, uint32_t nth) {
    ecu_later(data, len, SHIM_LATER_SAMPLE);
    s_ecu_later_samples = nth;
}

// Send ECU bytes during the next flash erase, while the plugin is stopped.
void ostrich_host_test_ecu_send_at_erase(const uint8_t *data, uint32_t len) {
    ecu_later(data, len, SHIM_LATER_ERASE);
}

// Take up to max bytes the UART has sent to the ECU.
uint32_t ostrich_host_test_ecu_receive(uint8_t *buf, uint32_t max) {
    uint32_t n = (s_dl.tx_level < max) ? s_dl.tx_level : max;
    for (uint32_t i = 0u; i < n; i++) {
        buf[i] = s_tx_fifo[s_tx_head];
        s_tx_head = (s_tx_head + 1u) % SHIM_TX_FIFO_DEPTH;
    }
    s_dl.tx_level -= n;
    return n;
}

const ostrich_datalog_hw_t *ostrich_host_test_datalog_hw(void) {
    return &s_dl;
}

void datalog_hw_uart_start(uint8_t uart, uint32_t ibrd, uint32_t fbrd) {
    s_dl.uart_starts++;
    s_dl.uart = uart;
    s_dl.ibrd = ibrd;
    s_dl.fbrd = fbrd;
}

void datalog_hw_pin(uint8_t gpio, uint8_t funcsel, bool rx) {
    if (s_dl.uart_starts == 0u) {
        s_dl.bad_order = 1u;
    }
    if (s_dl.pins < 2u) {
        s_dl.pin_gpio[s_dl.pins] = gpio;
        s_dl.pin_funcsel[s_dl.pins] = funcsel;
        s_dl.pin_rx[s_dl.pins] = rx ? 1u : 0u;
    }
    s_dl.pins++;
}

void datalog_hw_rx_dma_start(
    uint8_t  ch,
    uint8_t  uart,
    uint32_t ring_addr,
    uint8_t  ring_bits,
    uint32_t count
) {
    if (s_dl.uart_starts == 0u) {
        s_dl.bad_order = 1u;
    }
    s_dl.dma_starts++;
    s_dl.dma_ch = ch;
    s_dl.dma_uart = uart;
    s_dl.ring_addr = ring_addr;
    s_dl.ring_bits = ring_bits;
    s_dl.dma_count = count;
    s_dl.dma_remaining = count;
}

uint32_t datalog_hw_rx_dma_remaining(uint8_t ch) {
    if ((s_dl.dma_starts == 0u) || (ch != s_dl.dma_ch)) {
        s_dl.bad_target = 1u;
        return 0u;
    }
    if ((s_ecu_later_when == SHIM_LATER_SAMPLE) && (--s_ecu_later_samples == 0u)) {
        ecu_later_arrives(SHIM_LATER_SAMPLE);
    }
    return s_dl.dma_remaining;
}

uint32_t datalog_hw_tx_room(uint8_t uart) {
    if ((s_dl.uart_starts == 0u) || (uart != s_dl.uart)) {
        s_dl.bad_target = 1u;
        return 0u;
    }
    if (s_dl.tx_level == 0u) {
        return SHIM_TX_FIFO_DEPTH;
    }
    return (s_dl.tx_level < SHIM_TX_FIFO_DEPTH) ? 1u : 0u;
}

void datalog_hw_tx_write(uint8_t uart, uint8_t b) {
    if ((s_dl.uart_starts == 0u) || (uart != s_dl.uart)) {
        s_dl.bad_target = 1u;
        return;
    }
    if (s_dl.tx_level == SHIM_TX_FIFO_DEPTH) {
        s_dl.bad_tx_overflow = 1u;
        return;
    }
    s_tx_fifo[(s_tx_head + s_dl.tx_level) % SHIM_TX_FIFO_DEPTH] = b;
    s_dl.tx_level++;
}

// ---------------------------------------------------------------------------
// Flash model
// ---------------------------------------------------------------------------

static void check_call(void) {
    if (s_irq_depth == 0u) {
        s_state.bad_unmasked = 1u;
    }
    if (s_state.exclusive_depth == 0u) {
        s_state.bad_outside_exclusive = 1u;
    }
}

// The region holding [offs, offs + count), or -1.
static int find_region(uint32_t offs, uint32_t count) {
    for (uint32_t i = 0u; i < OSTRICH_SHIM_REGIONS; i++) {
        const region_t *r = &s_regions[i];
        if ((r->data != NULL) && (offs >= r->offs) &&
            ((offs - r->offs) <= r->size) && (count <= r->size - (offs - r->offs))) {
            return (int)i;
        }
    }
    return -1;
}

static void log_op(uint8_t kind, int region, uint32_t offs, uint32_t count) {
    if (s_state.op_log_count < OSTRICH_SHIM_OP_LOG_MAX) {
        ostrich_flash_op_t *op = &s_ops[s_state.op_log_count++];
        op->kind = kind;
        op->region = (region < 0) ? 0xFFu : (uint8_t)region;
        op->window = (uint16_t)s_state.windows;
        op->offs = offs;
        op->count = count;
    }
}

static uint64_t region_hash(uint32_t index) {
    const region_t *r = &s_regions[index];
    return (r->data == NULL) ? 0u : ostrich_host_test_hash(r->data, r->size);
}

// Partway through an erase or program of NV storage the boot slot byte can read
// as any value, so it is recorded as one that isn't a ROM slot.
#define SHIM_PARTIAL_BOOT_SLOT 0xA5u

static void record_cut(int region, uint8_t kind, uint8_t phase) {
    if (s_state.cut_count < OSTRICH_SHIM_CUT_MAX) {
        ostrich_cut_t *c = &s_cuts[s_state.cut_count++];
        c->op = s_state.ops;
        c->kind = kind;
        c->phase = phase;
        c->nv_boot_slot = ((region == (int)OSTRICH_SHIM_REGION_NV) &&
                           (phase == OSTRICH_SHIM_PHASE_MID))
                              ? SHIM_PARTIAL_BOOT_SLOT
                              : __nv_storage_start[0];
        c->reserved = 0u;
        c->slot_hash[0] = region_hash(0u);
        c->slot_hash[1] = region_hash(1u);
    }
}

static void shim_connect_internal_flash(void) {
    check_call();
    s_state.connect_calls++;
}

static void shim_flash_exit_xip(void) {
    check_call();
    s_state.xip_active = 0u;
}

// An erase or program is applied in two halves with the flash recorded after
// each, as a power cut partway through would leave it.
static void shim_flash_range_erase(uint32_t offs, uint32_t count, uint32_t block_size,
                                   uint8_t block_cmd) {
    (void)block_size;
    (void)block_cmd;
    check_call();
    ecu_later_arrives(SHIM_LATER_ERASE);
    int region = find_region(offs, count);
    log_op(OSTRICH_SHIM_OP_ERASE, region, offs, count);
    if (s_state.xip_active) {
        s_state.bad_xip_active = 1u;
    }
    if ((region < 0) || ((offs % 4096u) != 0u) || ((count % 4096u) != 0u)) {
        s_state.bad_range = 1u;
    } else {
        uint8_t *p = s_regions[region].data + (offs - s_regions[region].offs);
        uint32_t half = count / 2u;
        memset(p, 0xFF, half);
        record_cut(region, OSTRICH_SHIM_OP_ERASE, OSTRICH_SHIM_PHASE_MID);
        memset(p + half, 0xFF, count - half);
        record_cut(region, OSTRICH_SHIM_OP_ERASE, OSTRICH_SHIM_PHASE_AFTER);
    }
    s_state.ops++;
}

static void shim_flash_range_program(uint32_t offs, const uint8_t *data, uint32_t count) {
    check_call();
    int region = find_region(offs, count);
    log_op(OSTRICH_SHIM_OP_PROGRAM, region, offs, count);
    if (s_state.xip_active) {
        s_state.bad_xip_active = 1u;
    }
    s_programs++;
    if ((region < 0) || ((offs % 256u) != 0u) || ((count % 256u) != 0u)) {
        s_state.bad_range = 1u;
    } else {
        // Flash can only clear bits, so a program over unerased flash leaves
        // the AND of the two.
        uint8_t *p = s_regions[region].data + (offs - s_regions[region].offs);
        uint32_t half = count / 2u;
        bool fails = s_fail_nv_program && (region == (int)OSTRICH_SHIM_REGION_NV);
        for (uint32_t i = 0u; i < count; i++) {
            uint8_t b = fails ? 0xFFu : data[i];
            if ((i == 0u) && (s_programs == s_corrupt_program)) {
                b = (uint8_t)~b;
            }
            p[i] &= b;
            if (i + 1u == half) {
                record_cut(region, OSTRICH_SHIM_OP_PROGRAM, OSTRICH_SHIM_PHASE_MID);
            }
        }
        record_cut(region, OSTRICH_SHIM_OP_PROGRAM, OSTRICH_SHIM_PHASE_AFTER);
    }
    s_state.ops++;
}

static void shim_flash_flush_cache(void) {
    check_call();
    s_state.flush_calls++;
}

static void shim_flash_select_xip_read_mode(uint8_t mode, uint8_t clkdiv) {
    check_call();
    s_state.select_xip_calls++;
    s_state.last_xip_mode = mode;
    s_state.last_clkdiv = clkdiv;
    s_state.xip_active = 1u;
}

#define SHIM_ROM_CODE(a, b) \
    ((uint32_t)((uint32_t)(uint8_t)(b) << 8) | (uint32_t)(uint8_t)(a))

void *ora_host_test_bootrom_lookup(uint32_t code, uint32_t mask) {
    (void)mask;
    switch (code) {
        case SHIM_ROM_CODE('I', 'F'): return (void *)shim_connect_internal_flash;
        case SHIM_ROM_CODE('E', 'X'): return (void *)shim_flash_exit_xip;
        case SHIM_ROM_CODE('R', 'E'): return (void *)shim_flash_range_erase;
        case SHIM_ROM_CODE('F', 'C'): return (void *)shim_flash_flush_cache;
        case SHIM_ROM_CODE('X', 'M'): return (void *)shim_flash_select_xip_read_mode;
        case SHIM_ROM_CODE('R', 'P'): return (void *)shim_flash_range_program;
        default: return NULL;
    }
}

// ---------------------------------------------------------------------------
// API stand-ins
// ---------------------------------------------------------------------------

static ora_result_t shim_enter_exclusive(void) {
    if (s_fail_exclusive > 0u) {
        s_fail_exclusive--;
        return ORA_RESULT_NOT_SUPPORTED;
    }
    ora_enter_exclusive_mode_fn_t fw = ora_fn_lookup(ORA_ID_ENTER_EXCLUSIVE_MODE);
    ora_result_t r = fw();
    if (r == ORA_RESULT_OK) {
        s_state.exclusive_depth++;
        s_state.windows++;
    }
    return r;
}

static ora_result_t shim_exit_exclusive(void) {
    if (s_state.exclusive_depth > 0u) {
        s_state.exclusive_depth--;
    }
    ora_exit_exclusive_mode_fn_t fw = ora_fn_lookup(ORA_ID_EXIT_EXCLUSIVE_MODE);
    return fw();
}

// A modelled pair slot is read from the model, as a device reads flash.
static ora_result_t shim_copy_flash(uint8_t flash_slot, uint32_t flags,
                                    uint8_t ram_slot, uint32_t copy_flags) {
    if ((flags == ORA_FLASH_SLOT_FLAG_EXCLUDE_PLUGINS) && (flash_slot < 2u) &&
        (s_regions[flash_slot].data != NULL)) {
        ora_get_ram_slot_info_fn_t info = ora_fn_lookup(ORA_ID_GET_RAM_SLOT_INFO);
        uint32_t addr = 0u;
        uint32_t size = 0u;
        ora_result_t r = info(ram_slot, &addr, &size, NULL);
        if (r != ORA_RESULT_OK) {
            return r;
        }
        if (size != s_regions[flash_slot].size) {
            return ORA_RESULT_INVALID_SIZE;
        }
        memcpy(sram_to_host(addr), s_regions[flash_slot].data, size);
        return ORA_RESULT_OK;
    }
    ora_copy_flash_slot_to_ram_slot_fn_t fw = ora_fn_lookup(ORA_ID_COPY_FLASH_SLOT_TO_RAM_SLOT);
    return fw(flash_slot, flags, ram_slot, copy_flags);
}

static ora_result_t shim_flash_slot_addr(uint8_t flash_slot, uint32_t flags,
                                         uint32_t *addr_out, uint32_t *size_out) {
    if ((flags == ORA_FLASH_SLOT_FLAG_EXCLUDE_PLUGINS) && (flash_slot < 2u) &&
        s_slot_override[flash_slot]) {
        if (addr_out != NULL) {
            *addr_out = s_slot_override_addr[flash_slot];
        }
        if (size_out != NULL) {
            *size_out = s_slot_override_size[flash_slot];
        }
        return ORA_RESULT_OK;
    }
    ora_get_flash_slot_addr_fn_t fw = ora_fn_lookup(ORA_ID_GET_FLASH_SLOT_ADDR);
    return fw(flash_slot, flags, addr_out, size_out);
}

static uint8_t shim_ram_slot_count(void) {
    if (s_ram_count_override != 0u) {
        return s_ram_count_override;
    }
    ora_get_ram_slot_count_fn_t fw = ora_fn_lookup(ORA_ID_GET_RAM_SLOT_COUNT);
    return fw();
}

static ora_result_t shim_metadata_uint_at(ora_metadata_key_t key, uint32_t index,
                                          uint32_t *out) {
    if (s_x_pins_override && (index == 0u) && (out != NULL) &&
        ((key == ORA_METADATA_KEY_GPIO_X1) || (key == ORA_METADATA_KEY_GPIO_X2))) {
        *out = s_x_pins[(key == ORA_METADATA_KEY_GPIO_X1) ? 0u : 1u];
        return ORA_RESULT_OK;
    }
    ora_get_metadata_uint_at_fn_t fw = ora_fn_lookup(ORA_ID_GET_METADATA_UINT_AT);
    return fw(key, index, out);
}

static void *shim_lookup(api_id_t id) {
    for (uint32_t i = 0u; i < s_withheld_count; i++) {
        if (s_withheld[i] == (uint32_t)id) {
            return NULL;
        }
    }
    switch (id) {
        case ORA_ID_ENTER_EXCLUSIVE_MODE:
            return shim_enter_exclusive;
        case ORA_ID_EXIT_EXCLUSIVE_MODE:
            return shim_exit_exclusive;
        case ORA_ID_COPY_FLASH_SLOT_TO_RAM_SLOT:
            return shim_copy_flash;
        case ORA_ID_GET_FLASH_SLOT_ADDR:
            return shim_flash_slot_addr;
        case ORA_ID_GET_RAM_SLOT_COUNT:
            return shim_ram_slot_count;
        case ORA_ID_GET_METADATA_UINT_AT:
            return shim_metadata_uint_at;
        default:
            return ora_fn_lookup(id);
    }
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

// Doesn't return.  The harness runs it on a thread it abandons.
void ora_host_test_run_plugin(void) {
    ostrich_main(shim_lookup, ORA_PLUGIN_TYPE_USER, &s_entry_args);
}

uint32_t ora_host_test_plugin_version(void) {
    return ((uint32_t)ora_plugin_header.major_version << 24)
         | ((uint32_t)ora_plugin_header.minor_version << 16)
         | ((uint32_t)ora_plugin_header.patch_version << 8)
         | (uint32_t)ora_plugin_header.build_version;
}
