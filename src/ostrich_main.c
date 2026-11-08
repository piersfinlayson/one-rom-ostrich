// Moates Ostrich 2.0 serial protocol for One ROM.
//
// ECU tuning software reads and edits the served ROM image over One ROM's USB
// serial port as it would an Ostrich 2.0.  An edit changes the served RAM
// image at once.  After SAVE_QUIET_MS without an edit the image is written to
// the other ROM slot of the pair, and One ROM then boots that slot in future.
//
// The ECU's own serial port is bridged to the USB plugin's serial port 2.  See
// datalog.c.

#include <stdint.h>
#include <stdbool.h>

#include <plugin.h>

#include <usb_peer.h>

#include "datalog.h"
#include "flash_write.h"
#include "ram_slots.h"

// ---------------------------------------------------------------------------
// Values to confirm against a real Ostrich 2.0
// ---------------------------------------------------------------------------

// Reply to VV.  Unverified.  Real units may reply 10 07 'O' instead.
#define OST_VERSION_MAJOR   0x14u
#define OST_VERSION_MINOR   0x09u
#define OST_VERSION_TYPE    'O'

// First byte of the reply to NS.  Moates doesn't publish the value.
#define OST_VENDOR_ID       0x00u

// A command with bytes still to come is discarded after this long without one.
#define CMD_TIMEOUT_MS      500u

// A reply the host stops reading is dropped after this long without progress.
#define REPLY_TIMEOUT_MS    1000u

// The served image is saved to flash this long after the last edit.
#define SAVE_QUIET_MS       3000u

// ---------------------------------------------------------------------------
// Plugin header
// ---------------------------------------------------------------------------

// Flash storage reserved at the end of the plugin's region.  The first byte
// holds the ROM slot One ROM boots, and the plugin header contains its
// address.
#define NV_STORAGE_SIZE     4096u
#define NV_BOOT_SLOT        0u
#define NV_STORAGE_ADDR \
    (ORA_USER_PLUGIN_BASE + ORA_USER_PLUGIN_SIZE - NV_STORAGE_SIZE)

// This plugin's peer data and the serial ports' descriptions.  The peer data is a USBc
// block for the USB plugin followed by the 0 word.  The linker script places
// them after the header in this order.  The header and the block contain their
// addresses as constants rather than &peer_data because tester/ also compiles
// this file for a 64-bit host, where an address doesn't fit a 32-bit field.
// Serial port 1's description is padded to the longest a description can be
// so serial port 2's is at a fixed address.
struct ostrich_peer_data {
    usb_peer_usbc_t usbc;
    uint32_t        end;
};
_Static_assert(sizeof(struct ostrich_peer_data) == sizeof(usb_peer_usbc_t) + 4u,
               "the peer data must be contiguous");

#define CONTROL_PORT_DESC       "One ROM Ostrich"
#define PEER_DATA_ADDR          (ORA_USER_PLUGIN_BASE + ORA_PLUGIN_HEADER_SIZE)
#define CONTROL_PORT_DESC_ADDR  (PEER_DATA_ADDR + sizeof(struct ostrich_peer_data))
#define DATALOG_PORT_DESC_ADDR  (CONTROL_PORT_DESC_ADDR + sizeof(control_port_desc))

_Static_assert((sizeof(CONTROL_PORT_DESC) - 1u)
                   <= USB_PEER_SERIAL_PORT_DESC_MAX_LEN,
               "serial port 1's description is too long");
ORA_SECTION(".control_port_desc")
const char control_port_desc[USB_PEER_SERIAL_PORT_DESC_MAX_LEN + 1u] =
    CONTROL_PORT_DESC;

ORA_SECTION(".datalog_port_desc")
const char datalog_port_desc[] = "One ROM Ostrich Datalog";
_Static_assert((sizeof(datalog_port_desc) - 1u)
                   <= USB_PEER_SERIAL_PORT_DESC_MAX_LEN,
               "serial port 2's description is too long");

ORA_SECTION(".peer_data")
const struct ostrich_peer_data peer_data = {
    .usbc = {
        .hdr = {
            .format  = USB_PEER_USBC_FORMAT,
            .size    = sizeof(usb_peer_usbc_t),
            .version = USB_PEER_USBC_VERSION,
        },
        .flags = USB_PEER_USBC_SERIAL_PORT_1 | USB_PEER_USBC_SERIAL_PORT_2,
        .serial_port_1_desc = CONTROL_PORT_DESC_ADDR,
        .serial_port_2_desc = DATALOG_PORT_DESC_ADDR,
    },
    .end = 0,
};

void ostrich_main(
    ora_lookup_fn_t ora_lookup_fn,
    ora_plugin_type_t plugin_type,
    const ora_entry_args_t *entry_args
);

ORA_SECTION(".plugin_header")
const ora_plugin_header_t ora_plugin_header = {
    .magic = ORA_PLUGIN_MAGIC,
    .api_version = ORA_PLUGIN_VERSION_1,
    .major_version = MAJOR_VERSION,
    .minor_version = MINOR_VERSION,
    .patch_version = PATCH_VERSION,
    .build_version = BUILD_VERSION,
    .entry = ostrich_main,
    .plugin_type = ORA_PLUGIN_TYPE_USER,
    .sam_usage = 255,
    .overrides1 = ORA_OVERRIDE1_BOOT_SLOT | ORA_OVERRIDE1_IGNORE_IMAGE_SELECT,
    .properties1 = ORA_PROPERTY1_SUPPORTS_USB_RUNNING
                 | ORA_PROPERTY1_SUPPORTS_YIELD
                 | ORA_PROPERTY1_EXCLUSIVE_SERIAL_IN,
    .min_fw_major_version = 0,
    .min_fw_minor_version = 8,
    .min_fw_patch_version = 1,
    .boot_slot_addr = NV_STORAGE_ADDR + NV_BOOT_SLOT,
    .channel_2_size = ORA_LOG_CHANNEL_2_MAX_SIZE,
    .channel_3_size = ORA_LOG_CHANNEL_3_MAX_SIZE,
    .peer_data = PEER_DATA_ADDR,
    .reserved = {0},
};

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

#define OST_OK              'O'
#define OST_FAIL            '?'

// Bank replies and arguments.  Only bank 0 exists.
#define OST_BANK            0u

// Ostrich addresses are 16 bits.
#define OST_ADDR_SPACE      0x10000u

#define IN_BUF_SIZE         32u
#define READ_CHUNK          64u

// The longest header stored.  Longer headers are counted but not kept.
#define HDR_MAX             4u

typedef enum {
    PS_IDLE = 0,
    PS_HEADER,
    PS_DATA,
    PS_CHECKSUM,
} __attribute__((packed)) parse_state_t;

typedef enum {
    CK_VERSION = 0,
    CK_READ,
    CK_WRITE,
    CK_WRITE_X,
    CK_BULK,
    CK_BANK,
    CK_SERIAL,
    // Recognised and consumed in full, then answered with a failure
    CK_FAIL,
} __attribute__((packed)) cmd_kind_t;

typedef struct {
    parse_state_t state;
    cmd_kind_t    kind;
    uint8_t       hdr[HDR_MAX];
    uint8_t       hdr_need;
    uint8_t       hdr_got;
    uint8_t       sum;
    uint16_t      data_sum;
    uint32_t      addr;
    uint32_t      data_len;
    uint32_t      data_got;
    uint32_t      last_ms;
} parser_t;

// ---------------------------------------------------------------------------
// Flash and RAM
// ---------------------------------------------------------------------------

#define FLASH_SECTOR_SIZE   4096u
#define FLASH_PAGE_SIZE     256u
#define PAIR_SLOTS          2u

// Bytes of stack the flash write routine is copied into.  The Makefile
// defines FLASH_WRITE_FN_MAX for the compiler and the linker.
_Static_assert((FLASH_WRITE_FN_MAX % 4u) == 0u,
               "FLASH_WRITE_FN_MAX must be a multiple of 4");

extern const uint8_t __nv_storage_start[];
extern const uint8_t __flash_write_fn_start[];
extern const uint8_t __flash_write_fn_end[];

typedef struct {
    bool     supported;
    uint8_t  active_ram;
    uint8_t  spare_ram;
    // Slot of the pair the served image was booted from or last saved to
    uint8_t  active_flash;
    uint32_t chip_size;
    uint32_t region_size;
    uint32_t active_base;
    uint32_t spare_base;
    uint32_t flash_offs[PAIR_SLOTS];
} device_t;

typedef struct {
    bool     dirty;
    bool     active;
    uint8_t  target;
    uint16_t sector;
    uint32_t last_edit_ms;
} save_t;

// ---------------------------------------------------------------------------
// State
//
// Every static is set by init_state() at entry.  A host test build enters the
// plugin once per scenario in the same process, and doesn't zero .bss.
// ---------------------------------------------------------------------------

static ora_lookup_fn_t                 s_lookup;
static ora_log_fn_t                    s_log;
static ora_err_log_fn_t                s_err;
static ora_yield_fn_t                  s_yield;
static ora_get_plugin_uptime_ms_fn_t   s_uptime;
static ora_log_read_fn_t               s_read_in;
static ora_log_write_fn_t              s_write_out;
static ora_log_query_fn_t              s_query;
static ora_reprogram_ram_rom_slot_fn_t s_reprogram;
static ora_read_ram_rom_slot_fn_t      s_read_slot;

static device_t s_dev;
static save_t   s_save;
static parser_t s_cmd;

// The spare RAM slot.  Write data is staged here at its offset in the chip
// and applied to the served slot once the checksum matches.
static volatile uint8_t *s_staging;

static uint8_t s_in[IN_BUF_SIZE];

static void init_state(void) {
    s_lookup    = NULL;
    s_log       = NULL;
    s_err       = NULL;
    s_yield     = NULL;
    s_uptime    = NULL;
    s_read_in   = NULL;
    s_write_out = NULL;
    s_query     = NULL;
    s_reprogram = NULL;
    s_read_slot = NULL;

    s_dev.supported    = false;
    s_dev.active_ram   = 0u;
    s_dev.spare_ram    = 0u;
    s_dev.active_flash = 0u;
    s_dev.chip_size    = 0u;
    s_dev.region_size  = 0u;
    s_dev.active_base  = 0u;
    s_dev.spare_base   = 0u;
    for (uint32_t i = 0u; i < PAIR_SLOTS; i++) {
        s_dev.flash_offs[i] = 0u;
    }

    s_save.dirty        = false;
    s_save.active       = false;
    s_save.target       = 0u;
    s_save.sector       = 0u;
    s_save.last_edit_ms = 0u;

    s_cmd.state   = PS_IDLE;
    s_cmd.last_ms = 0u;
    s_staging     = NULL;
}

// Copy the plugin's initialised data to RAM and zero its .bss.  A host build
// doesn't have either section.
static void init_data_bss(void) {
#if !defined(ORA_HOST_TEST)
    extern uint32_t __data_start;
    extern uint32_t __data_end;
    extern uint32_t __data_load;
    extern uint32_t __bss_start;
    extern uint32_t __bss_end;

    uint32_t *src = &__data_load;
    uint32_t *dst = &__data_start;
    while (dst < &__data_end) {
        *dst++ = *src++;
    }

    dst = &__bss_start;
    while (dst < &__bss_end) {
        *dst++ = 0u;
    }
#endif
}

static uint32_t now_ms(void) {
    return s_uptime();
}

static uint32_t min_u32(uint32_t a, uint32_t b) {
    return (a < b) ? a : b;
}

// ---------------------------------------------------------------------------
// Replies
// ---------------------------------------------------------------------------

// Write a reply to the host, waiting for room in the channel.  Returns false
// if the host stopped reading and the rest of the reply was dropped.
static bool reply(const uint8_t *buf, uint32_t len) {
    uint32_t progress_ms = now_ms();
    while (len > 0u) {
        datalog_service();
        uint32_t room = 0u;
        (void)s_query(ORA_LOG_CHANNEL_0, NULL, &room, NULL);
        uint32_t n = min_u32(len, room);
        if ((n > 0u) && (s_write_out(ORA_LOG_CHANNEL_0, buf, n) == ORA_RESULT_OK)) {
            buf += n;
            len -= n;
            progress_ms = now_ms();
            continue;
        }
        if ((now_ms() - progress_ms) >= REPLY_TIMEOUT_MS) {
            s_err("Ostrich: host stopped reading, reply dropped");
            return false;
        }
        (void)s_yield(NULL);
        ORA_TEST_YIELD();
    }
    return true;
}

static void reply_byte(uint8_t b) {
    (void)reply(&b, 1u);
}

// Reply with len bytes of the served image from Ostrich address addr, then
// their checksum.
static void reply_image(uint32_t addr, uint32_t len) {
    uint8_t buf[READ_CHUNK];
    uint8_t sum = 0u;
    while (len > 0u) {
        uint32_t offset = addr & (s_dev.chip_size - 1u);
        uint32_t n = min_u32(min_u32(len, READ_CHUNK), s_dev.chip_size - offset);
        if (s_read_slot(s_dev.active_ram, offset, buf, n) != ORA_RESULT_OK) {
            s_err("Ostrich: read of offset 0x%04X failed", (unsigned)offset);
            return;
        }
        for (uint32_t i = 0u; i < n; i++) {
            sum = (uint8_t)(sum + buf[i]);
        }
        if (!reply(buf, n)) {
            return;
        }
        addr += n;
        len -= n;
    }
    reply_byte(sum);
}

static void reply_version(void) {
    const uint8_t v[3] = { OST_VERSION_MAJOR, OST_VERSION_MINOR, OST_VERSION_TYPE };
    (void)reply(v, sizeof(v));
}

// The vendor byte, the RP2350's 8-byte chip ID and their checksum.
static void reply_serial(void) {
    ora_get_metadata_uint_at_fn_t get_at = s_lookup(ORA_ID_GET_METADATA_UINT_AT);
    uint8_t out[10];
    uint8_t sum = OST_VENDOR_ID;
    out[0] = OST_VENDOR_ID;
    for (uint32_t i = 0u; i < 8u; i++) {
        uint32_t v = 0u;
        if ((get_at == NULL) ||
            (get_at(ORA_METADATA_KEY_CHIP_ID, i, &v) != ORA_RESULT_OK)) {
            s_err("Ostrich: chip ID unavailable");
            reply_byte(OST_FAIL);
            return;
        }
        out[1u + i] = (uint8_t)v;
        sum = (uint8_t)(sum + (uint8_t)v);
    }
    out[9] = sum;
    (void)reply(out, sizeof(out));
}

// ---------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------

// Copy the staged bytes of the command just received into the served slot.
static bool apply_write(void) {
    uint32_t size  = s_dev.chip_size;
    uint32_t start = s_cmd.addr & (size - 1u);
    uint32_t len   = s_cmd.data_len;
    const uint8_t *staged = (const uint8_t *)s_staging;

    ora_result_t r;
    if (len >= size) {
        // The command covered the whole chip.  Staging holds the last byte
        // written to each offset.
        r = s_reprogram(s_dev.active_ram, 0u, staged, size, 1u);
    } else {
        uint32_t first = min_u32(len, size - start);
        r = s_reprogram(s_dev.active_ram, start, staged + start, first, 1u);
        if ((r == ORA_RESULT_OK) && (len > first)) {
            r = s_reprogram(s_dev.active_ram, 0u, staged, len - first, 1u);
        }
    }
    if (r != ORA_RESULT_OK) {
        s_err("Ostrich: write to offset 0x%04X failed", (unsigned)start);
        return false;
    }

    if (s_save.active) {
        s_log("Ostrich: edit during save, save abandoned");
        s_save.active = false;
    }
    s_save.dirty = true;
    s_save.last_edit_ms = now_ms();
    return true;
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

static void bank_command(uint8_t op, uint8_t arg) {
    bool select = (arg == OST_BANK);
    switch (op) {
    case 'R':
        // BRR reads the read/write bank.  BR n selects it.
        if (arg == 'R') {
            reply_byte(OST_BANK);
            return;
        }
        break;
    case 'E':
        // BER and BEE read the emulation bank.  BES reads the stored one.
        if ((arg == 'R') || (arg == 'E') || (arg == 'S')) {
            reply_byte(OST_BANK);
            return;
        }
        break;
    case 'S':
        break;
    default:
        select = false;
        break;
    }
    reply_byte(select ? OST_OK : OST_FAIL);
}

// Run the command just received.  Its checksum has matched.
static void execute(void) {
    if (!s_dev.supported) {
        reply_byte(OST_FAIL);
        return;
    }

    switch (s_cmd.kind) {
    case CK_VERSION:
        reply_version();
        break;
    case CK_READ:
        reply_image(s_cmd.addr, s_cmd.data_len);
        break;
    case CK_WRITE:
        reply_byte(apply_write() ? OST_OK : OST_FAIL);
        break;
    case CK_WRITE_X:
        if (apply_write()) {
            const uint8_t sum[2] = {
                (uint8_t)(s_cmd.data_sum >> 8),
                (uint8_t)s_cmd.data_sum,
            };
            (void)reply(sum, sizeof(sum));
        } else {
            reply_byte(OST_FAIL);
        }
        break;
    case CK_BULK:
        if (s_cmd.hdr[0] == 'R') {
            reply_image(s_cmd.addr, s_cmd.data_len);
        } else {
            reply_byte(apply_write() ? OST_OK : OST_FAIL);
        }
        break;
    case CK_BANK:
        bank_command(s_cmd.hdr[0], s_cmd.hdr[1]);
        break;
    case CK_SERIAL:
        reply_serial();
        break;
    case CK_FAIL:
        reply_byte(OST_FAIL);
        break;
    }
}

static void parser_idle(void) {
    s_cmd.state = PS_IDLE;
}

// n = 0 means 256 in every command that has a count.
static uint32_t count_or_256(uint8_t n) {
    return (n == 0u) ? 256u : (uint32_t)n;
}

static void begin_command(uint8_t b) {
    s_cmd.sum      = b;
    s_cmd.data_sum = 0u;
    s_cmd.hdr_got  = 0u;
    s_cmd.data_len = 0u;
    s_cmd.data_got = 0u;
    s_cmd.addr     = 0u;
    s_cmd.state    = PS_HEADER;

    switch (b) {
    case 'V':
        s_cmd.kind = CK_VERSION;
        s_cmd.hdr_need = 1u;
        break;
    case 'R':
    case 'r':
        s_cmd.kind = CK_READ;
        s_cmd.hdr_need = 3u;
        break;
    case 'W':
    case 'w':
        s_cmd.kind = CK_WRITE;
        s_cmd.hdr_need = 3u;
        break;
    case 'X':
        s_cmd.kind = CK_WRITE_X;
        s_cmd.hdr_need = 3u;
        break;
    case 'Z':
        s_cmd.kind = CK_BULK;
        s_cmd.hdr_need = 4u;
        break;
    case 'B':
        s_cmd.kind = CK_BANK;
        s_cmd.hdr_need = 2u;
        break;
    case 'N':
        s_cmd.kind = CK_SERIAL;
        s_cmd.hdr_need = 1u;
        break;
    case 'T':
        // Trace has three option bytes, two counts and two 3-byte addresses
        s_cmd.kind = CK_FAIL;
        s_cmd.hdr_need = 11u;
        break;
    case 'S':
        // Baud rate change
        s_cmd.kind = CK_FAIL;
        s_cmd.hdr_need = 1u;
        break;
    default:
        reply_byte(OST_FAIL);
        parser_idle();
        break;
    }
}

static void header_done(void) {
    switch (s_cmd.kind) {
    case CK_VERSION:
        // VV doesn't have a checksum
        if (s_cmd.hdr[0] == 'V') {
            execute();
        } else {
            reply_byte(OST_FAIL);
        }
        parser_idle();
        return;
    case CK_READ:
    case CK_WRITE:
    case CK_WRITE_X:
        s_cmd.addr = ((uint32_t)s_cmd.hdr[1] << 8) | s_cmd.hdr[2];
        s_cmd.data_len = count_or_256(s_cmd.hdr[0]);
        break;
    case CK_BULK:
        // ZW and ZR address 256-byte blocks.  MMSB applies only in bank 8,
        // and only bank 0 exists.
        s_cmd.addr = (uint32_t)s_cmd.hdr[3] << 8;
        s_cmd.data_len = count_or_256(s_cmd.hdr[1]) * 256u;
        break;
    case CK_BANK:
    case CK_SERIAL:
    case CK_FAIL:
        break;
    }

    bool has_data = (s_cmd.kind == CK_WRITE) || (s_cmd.kind == CK_WRITE_X) ||
                    ((s_cmd.kind == CK_BULK) && (s_cmd.hdr[0] == 'W'));
    s_cmd.state = has_data ? PS_DATA : PS_CHECKSUM;
}

static void header_byte(uint8_t b) {
    if (s_cmd.hdr_got < HDR_MAX) {
        s_cmd.hdr[s_cmd.hdr_got] = b;
    }
    s_cmd.hdr_got++;

    if (s_cmd.hdr_got == 1u) {
        if ((s_cmd.kind == CK_BULK) && (b != 'R') && (b != 'W')) {
            // An unrecognised Z command's length is unknown, so its remaining
            // bytes are parsed as commands.
            reply_byte(OST_FAIL);
            parser_idle();
            return;
        }
        if ((s_cmd.kind == CK_SERIAL) && (b != 'S')) {
            // Nx and Nn change the vendor byte or the serial number.  Eight
            // bytes follow.
            s_cmd.kind = CK_FAIL;
            s_cmd.hdr_need = 9u;
        }
    }

    if (s_cmd.hdr_got == s_cmd.hdr_need) {
        header_done();
    }
}

static void data_byte(uint8_t b) {
    if (s_dev.supported) {
        uint32_t offset = (s_cmd.addr + s_cmd.data_got) & (s_dev.chip_size - 1u);
        s_staging[offset] = b;
    }
    s_cmd.data_sum = (uint16_t)(s_cmd.data_sum + b);
    s_cmd.data_got++;
    if (s_cmd.data_got == s_cmd.data_len) {
        s_cmd.state = PS_CHECKSUM;
    }
}

static void parse_byte(uint8_t b) {
    switch (s_cmd.state) {
    case PS_IDLE:
        begin_command(b);
        return;
    case PS_HEADER:
        s_cmd.sum = (uint8_t)(s_cmd.sum + b);
        header_byte(b);
        return;
    case PS_DATA:
        s_cmd.sum = (uint8_t)(s_cmd.sum + b);
        data_byte(b);
        return;
    case PS_CHECKSUM:
        parser_idle();
        if (b == s_cmd.sum) {
            execute();
        } else {
            reply_byte(OST_FAIL);
        }
        return;
    }
}

// ---------------------------------------------------------------------------
// Flash
// ---------------------------------------------------------------------------

static void *bootrom_fn(char a, char b) {
    uint32_t code = ((uint32_t)(uint8_t)b << 8) | (uint32_t)(uint8_t)a;
    return ORA_BOOTROM_LOOKUP(code, ORA_BOOTROM_FLAG_ARM_SEC);
}

// Erase erase_size bytes at erase_offs and program program_size bytes from
// data at program_offs in one exclusive mode window.  Both offsets are from
// the start of flash.  data must not be in flash.
//
// Not inlined, so its frame and the routine's copy on it exist only while it
// runs.
static bool __attribute__((noinline)) flash_write(
    uint32_t       erase_offs,
    uint32_t       program_offs,
    const uint8_t *data,
    uint32_t       erase_size,
    uint32_t       program_size
) {
    connect_internal_flash_fn_t connect = bootrom_fn('I', 'F');
    flash_exit_xip_fn_t exit_xip = bootrom_fn('E', 'X');
    flash_range_erase_fn_t erase = bootrom_fn('R', 'E');
    flash_range_program_fn_t program = bootrom_fn('R', 'P');
    flash_flush_cache_fn_t flush = bootrom_fn('F', 'C');
    flash_select_xip_read_mode_fn_t select_xip = bootrom_fn('X', 'M');
    if ((connect == NULL) || (exit_xip == NULL) || (erase == NULL) ||
        (program == NULL) || (flush == NULL) || (select_xip == NULL)) {
        s_err("Ostrich: bootrom flash function missing");
        return false;
    }

    ora_enter_exclusive_mode_fn_t enter = s_lookup(ORA_ID_ENTER_EXCLUSIVE_MODE);
    ora_exit_exclusive_mode_fn_t leave = s_lookup(ORA_ID_EXIT_EXCLUSIVE_MODE);
    if ((enter == NULL) || (leave == NULL)) {
        s_err("Ostrich: exclusive mode unavailable");
        return false;
    }

    uint32_t fn_size = ORA_STAGED_FN_SIZE(__flash_write_fn_start, __flash_write_fn_end);
    if (fn_size > FLASH_WRITE_FN_MAX) {
        // Only a host build reaches this.  A device build fails to link.
        s_err("Ostrich: flash routine %u exceeds %u", (unsigned)fn_size,
              (unsigned)FLASH_WRITE_FN_MAX);
        return false;
    }
    uint32_t code[FLASH_WRITE_FN_MAX / 4u];
    volatile uint8_t *code_bytes = (volatile uint8_t *)code;
    for (uint32_t i = 0u; i < fn_size; i++) {
        code_bytes[i] = __flash_write_fn_start[i];
    }
#if !defined(ORA_HOST_TEST)
    // The copy was written as data.  Make it visible to instruction fetch.
    __asm volatile ("dsb \n\t isb" ::: "memory");
#endif
    flash_write_critical_fn_t write_fn =
        ORA_STAGED_FN_PTR(flash_write_critical_fn_t, (uintptr_t)code);

    if (enter() != ORA_RESULT_OK) {
        s_err("Ostrich: couldn't enter exclusive mode");
        return false;
    }

    // Interrupts are masked before connect_internal_flash, which changes the
    // flash settings XIP runs with.  An interrupt handler on this core runs
    // from flash.
    uint32_t primask = flash_irq_disable();
    connect();
    uint8_t clkdiv = ORA_XIP_CLKDIV();
    write_fn(exit_xip, erase, program, flush, select_xip, erase_offs,
             program_offs, data, erase_size, program_size, clkdiv);
    flash_irq_restore(primask);

    (void)leave();
    return true;
}

// Compare a pair slot's flash with the served image.  The slot is read into
// the spare RAM slot.
static bool verify_slot(uint8_t slot) {
    ora_copy_flash_slot_to_ram_slot_fn_t copy = s_lookup(ORA_ID_COPY_FLASH_SLOT_TO_RAM_SLOT);
    if ((copy == NULL) ||
        (copy(slot, ORA_FLASH_SLOT_FLAG_EXCLUDE_PLUGINS, s_dev.spare_ram, 0u) != ORA_RESULT_OK)) {
        s_err("Ostrich: read back of slot %u failed", (unsigned)slot);
        return false;
    }
    const volatile uint8_t *served = ORA_SRAM_PTR(s_dev.active_base);
    for (uint32_t i = 0u; i < s_dev.region_size; i++) {
        if (s_staging[i] != served[i]) {
            s_err("Ostrich: slot %u differs at 0x%X", (unsigned)slot, (unsigned)i);
            return false;
        }
    }
    return true;
}

// Make `slot` the slot One ROM boots.
static bool write_boot_slot(uint8_t slot) {
    // The page is built in the spare RAM slot, which doesn't hold staged data
    // between commands.
    for (uint32_t i = 0u; i < FLASH_PAGE_SIZE; i++) {
        s_staging[i] = 0xFFu;
    }
    s_staging[NV_BOOT_SLOT] = slot;
    uint32_t offs = ORA_FLASH_OFFSET(__nv_storage_start);
    return flash_write(offs, offs, (const uint8_t *)s_staging, NV_STORAGE_SIZE,
                       FLASH_PAGE_SIZE);
}

// The slot One ROM boots, from the boot slot byte.  The firmware boots slot 0
// for a value that isn't a ROM slot.
static uint8_t boot_slot(void) {
    uint8_t slot = __nv_storage_start[NV_BOOT_SLOT];
    return (slot < PAIR_SLOTS) ? slot : 0u;
}

// A save that couldn't finish is tried again after the next quiet period.
static void save_retry(uint32_t now) {
    s_save.active = false;
    s_save.dirty = true;
    s_save.last_edit_ms = now;
}

// Advance the save by one exclusive mode window.  Called only between
// commands.
//
// The slot that booted or was last saved isn't written, so a power cut at any
// point leaves One ROM booting a complete image.  The boot slot byte changes
// only once the other slot is written and verified.
static void save_step(uint32_t now) {
    if (!s_save.active) {
        if (!s_save.dirty || ((now - s_save.last_edit_ms) < SAVE_QUIET_MS)) {
            return;
        }
        s_save.dirty  = false;
        s_save.active = true;
        s_save.sector = 0u;
        s_save.target = (uint8_t)(1u - s_dev.active_flash);
        s_log("Ostrich: saving to slot %u", (unsigned)s_save.target);
        return;
    }

    uint32_t sectors = s_dev.region_size / FLASH_SECTOR_SIZE;
    if (s_save.sector < sectors) {
        uint32_t within = (uint32_t)s_save.sector * FLASH_SECTOR_SIZE;
        uint32_t offs = s_dev.flash_offs[s_save.target] + within;
        const uint8_t *src = (const uint8_t *)ORA_SRAM_PTR(s_dev.active_base) + within;
        if (!flash_write(offs, offs, src, FLASH_SECTOR_SIZE, FLASH_SECTOR_SIZE)) {
            save_retry(now);
            return;
        }
        s_save.sector++;
        return;
    }

    s_save.active = false;
    if (!verify_slot(s_save.target)) {
        // Not retried until the next edit so a failing flash isn't erased
        // repeatedly.
        return;
    }
    if (!write_boot_slot(s_save.target)) {
        save_retry(now);
        return;
    }
    // One ROM boots the slot the byte reads as now, whatever was written.  The
    // next save goes to the other slot.
    s_dev.active_flash = boot_slot();
    if (s_dev.active_flash != s_save.target) {
        s_err("Ostrich: boot slot byte reads 0x%02X, not %u",
              (unsigned)__nv_storage_start[NV_BOOT_SLOT], (unsigned)s_save.target);
        return;
    }
    s_log("Ostrich: saved to slot %u", (unsigned)s_save.target);
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

static bool is_power_of_two(uint32_t v) {
    return (v != 0u) && ((v & (v - 1u)) == 0u);
}

// Check the device can serve Ostrich and record its layout.  Returns NULL, or
// why it can't.
//
// Not inlined, so its frame is gone before the main loop runs.
static const char * __attribute__((noinline)) setup_device(const ora_entry_args_t *entry_args) {
    ora_get_ram_slot_count_fn_t ram_count = s_lookup(ORA_ID_GET_RAM_SLOT_COUNT);
    ora_get_ram_slot_info_fn_t ram_info = s_lookup(ORA_ID_GET_RAM_SLOT_INFO);
    ora_get_active_ram_slot_fn_t active_ram = s_lookup(ORA_ID_GET_ACTIVE_RAM_SLOT);
    ora_get_flash_slot_count_fn_t flash_count = s_lookup(ORA_ID_GET_FLASH_SLOT_COUNT);
    ora_get_flash_slot_info_fn_t flash_info = s_lookup(ORA_ID_GET_FLASH_SLOT_INFO);
    ora_get_flash_slot_ext_info_fn_t flash_ext = s_lookup(ORA_ID_GET_FLASH_SLOT_EXT_INFO);
    ora_get_flash_slot_addr_fn_t flash_addr = s_lookup(ORA_ID_GET_FLASH_SLOT_ADDR);
    ora_get_metadata_uint_fn_t metadata = s_lookup(ORA_ID_GET_METADATA_UINT);
    if ((ram_count == NULL) || (ram_info == NULL) || (active_ram == NULL) ||
        (flash_count == NULL) || (flash_info == NULL) || (flash_ext == NULL) ||
        (flash_addr == NULL) || (metadata == NULL) ||
        (s_reprogram == NULL) || (s_read_slot == NULL)) {
        return "plugin API call missing";
    }

    if ((entry_args->flags & ORA_ENTRY_FLAG_EXCLUSIVE_SERIAL_IN) == 0u) {
        return "serial port not exclusive to this plugin";
    }

    if (ram_count() < 2u) {
        return "no spare RAM slot";
    }
    if (active_ram(&s_dev.active_ram) != ORA_RESULT_OK) {
        return "no RAM slot served";
    }
    s_dev.spare_ram = staging_ram_slot(s_dev.active_ram);
    if ((ram_info(s_dev.active_ram, &s_dev.active_base, &s_dev.region_size, NULL) != ORA_RESULT_OK) ||
        (ram_info(s_dev.spare_ram, &s_dev.spare_base, NULL, NULL) != ORA_RESULT_OK)) {
        return "RAM slot unavailable";
    }

    const uint32_t only_roms = ORA_FLASH_SLOT_FLAG_EXCLUDE_PLUGINS;
    // A boot slot byte left partly written by a power cut can hold any value,
    // and the firmware boots slot 0 for one that isn't a ROM slot.  With
    // exactly two ROM slots every value boots a complete image.
    if (flash_count(only_roms) != PAIR_SLOTS) {
        return "exactly two ROM slots required";
    }
    uint32_t type[PAIR_SLOTS];
    uint32_t chip[PAIR_SLOTS];
    for (uint8_t slot = 0u; slot < PAIR_SLOTS; slot++) {
        uint8_t roms = 0u;
        if ((flash_info(slot, only_roms, NULL, &type[slot], &roms) != ORA_RESULT_OK) ||
            (flash_ext(slot, 0u, only_roms, NULL, NULL, &chip[slot], NULL) != ORA_RESULT_OK)) {
            return "ROM slot unavailable";
        }
        if (roms != 1u) {
            return "ROM slots 0 and 1 must each hold one ROM";
        }

        uint32_t addr = 0u;
        uint32_t size = 0u;
        if (flash_addr(slot, only_roms, &addr, &size) != ORA_RESULT_OK) {
            return "ROM slot address unavailable";
        }
        if (((addr % FLASH_SECTOR_SIZE) != 0u) || ((size % FLASH_SECTOR_SIZE) != 0u)) {
            return "ROM slot not in whole 4KB flash sectors";
        }
        if (size != s_dev.region_size) {
            return "ROM slot size differs from RAM slot size";
        }
        s_dev.flash_offs[slot] = addr - ORA_FLASH_BASE_ADDR;
    }
    if ((type[0] != type[1]) || (chip[0] != chip[1])) {
        return "ROM slots 0 and 1 differ in chip type";
    }
    s_dev.chip_size = chip[0];
    if ((s_dev.chip_size > OST_ADDR_SPACE) || !is_power_of_two(s_dev.chip_size)) {
        return "chip size unsupported";
    }

    // The firmware's slot index counts the plugin slots ahead of the ROMs.
    uint32_t rom_slot_index = 0u;
    if (metadata(ORA_METADATA_KEY_ROM_SLOT_INDEX, &rom_slot_index) != ORA_RESULT_OK) {
        return "booted slot unknown";
    }
    uint32_t plugin_slots = (uint32_t)flash_count(0u) - flash_count(only_roms);
    s_dev.active_flash = (uint8_t)(rom_slot_index - plugin_slots);

    s_staging = ORA_SRAM_PTR(s_dev.spare_base);
    return NULL;
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

static void idle_forever(void) {
    for (;;) {
        if (s_yield != NULL) {
            (void)s_yield(NULL);
        }
        datalog_service();
        ORA_TEST_YIELD();
    }
}

void ostrich_main(
    ora_lookup_fn_t         ora_lookup_fn,
    ora_plugin_type_t       plugin_type,
    const ora_entry_args_t *entry_args
) {
    (void)plugin_type;
    init_data_bss();
    init_state();
    datalog_start(ora_lookup_fn);

    s_lookup    = ora_lookup_fn;
    s_log       = ora_lookup_fn(ORA_ID_LOG);
    s_err       = ora_lookup_fn(ORA_ID_ERR_LOG);
    s_yield     = ora_lookup_fn(ORA_ID_YIELD);
    s_uptime    = ora_lookup_fn(ORA_ID_GET_PLUGIN_UPTIME_MS);
    s_read_in   = ora_lookup_fn(ORA_ID_LOG_READ);
    s_write_out = ora_lookup_fn(ORA_ID_LOG_WRITE);
    s_query     = ora_lookup_fn(ORA_ID_LOG_QUERY);
    s_reprogram = ora_lookup_fn(ORA_ID_REPROGRAM_RAM_ROM_SLOT);
    s_read_slot = ora_lookup_fn(ORA_ID_READ_RAM_ROM_SLOT);
    ora_log_open_read_fn_t open_read = ora_lookup_fn(ORA_ID_LOG_OPEN_READ);
    ora_log_open_write_fn_t open_write = ora_lookup_fn(ORA_ID_LOG_OPEN_WRITE);

    if ((s_log == NULL) || (s_err == NULL) || (s_yield == NULL) ||
        (s_uptime == NULL) || (s_read_in == NULL) || (s_write_out == NULL) ||
        (s_query == NULL) || (open_read == NULL) || (open_write == NULL)) {
        idle_forever();
    }

    // Host bytes arrive on channel 1 and replies go out on channel 0.
    if ((open_read(ORA_LOG_CHANNEL_1) != ORA_RESULT_OK) ||
        (open_write(ORA_LOG_CHANNEL_0, "ostrich") != ORA_RESULT_OK)) {
        s_err("Ostrich: serial channels in use");
        idle_forever();
    }

    const char *why = setup_device(entry_args);
    if (why != NULL) {
        s_err("Ostrich: unsupported, every command fails: %s", why);
    } else {
        s_dev.supported = true;
        s_log("Ostrich: chip %uKB, booted slot %u", (unsigned)(s_dev.chip_size / 1024u),
              (unsigned)s_dev.active_flash);
    }

    for (;;) {
        (void)s_yield(NULL);
        datalog_service();

        uint32_t got = 0u;
        (void)s_read_in(ORA_LOG_CHANNEL_1, s_in, sizeof(s_in), &got);
        uint32_t now = now_ms();

        if (got > 0u) {
            for (uint32_t i = 0u; i < got; i++) {
                parse_byte(s_in[i]);
            }
            // The time is taken after the bytes are handled because a reply
            // may take seconds.
            s_cmd.last_ms = now_ms();
        } else if (s_cmd.state != PS_IDLE) {
            if ((now - s_cmd.last_ms) >= CMD_TIMEOUT_MS) {
                s_log("Ostrich: incomplete command discarded");
                parser_idle();
            }
        } else if (s_dev.supported) {
            save_step(now);
        }

        ORA_TEST_YIELD();
    }
}
