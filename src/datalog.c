// The ECU's serial port, bridged to the USB plugin's serial port 2.
//
// The ECU's transmit and receive wires are connected to two header pins, and a
// hardware UART on those pins runs at a fixed 38400 baud.  A DMA channel copies
// received bytes into a ring in a RAM slot, and the plugin copies them from
// there to log channel 2, which the USB plugin sends to the PC.  The USB plugin
// writes the PC's bytes to log channel 3, and the plugin copies them to the
// UART.

#include <stdint.h>
#include <stdbool.h>

#include <plugin.h>

#include "datalog.h"
#include "datalog_hw.h"
#include "ram_slots.h"

#define DATALOG_BAUD        38400u

#define CH_TO_PC            ORA_LOG_CHANNEL_2
#define CH_FROM_PC          ORA_LOG_CHANNEL_3

// The ring holds ECU bytes while the plugin can't copy them.  The longest stop
// is a save's 4KB sector write with interrupts masked.  4096 bytes is 1066ms at
// the ECU's 3840 bytes a second.  A 4KB erase and program takes at most 448ms
// in the W25Q16JV's datasheet.
#define RING_BITS           12u
#define RING_SIZE           (1u << RING_BITS)

// Bytes the DMA channel copies before it restarts.  A multiple of the ring
// size, so the count of bytes received modulo the ring size is the position in
// the ring.
#define DMA_COUNT           (1u << 27)
#define DMA_COUNT_MASK      (DMA_COUNT - 1u)

#define CHUNK               32u

// Lost bytes are logged at most this often, as a PC that isn't reading loses
// every byte.
#define LOST_LOG_MS         5000u

// UART function selects.  See the GPIO function table in the RP2350
// datasheet.
#define FUNCSEL_UART        2u
#define FUNCSEL_UART_AUX    11u
#define GPIO_COUNT          48u

typedef struct {
    bool              active;
    uint8_t           uart;
    uint8_t           dma_ch;
    volatile uint8_t *ring;
    // Bytes taken from the ring, modulo DMA_COUNT
    uint32_t          read;
    uint32_t          lost;
    uint32_t          lost_logged;
    uint32_t          lost_log_ms;
} datalog_t;

static ora_log_fn_t                  s_log;
static ora_get_plugin_uptime_ms_fn_t s_uptime;
static ora_log_read_fn_t             s_read;
static ora_log_write_fn_t            s_write;
static ora_log_query_fn_t            s_query;

static datalog_t s_dl;

static uint32_t min_u32(uint32_t a, uint32_t b) {
    return (a < b) ? a : b;
}

// GPIO % 4 selects the UART function and direction, and the UART instance
// alternates every 8 GPIOs from GPIO 4.
bool datalog_pins(uint8_t gpio_a, uint8_t gpio_b, datalog_pins_t *out) {
    const uint8_t gpio[2] = { gpio_a, gpio_b };
    uint8_t uart[2];
    bool tx[2];
    uint8_t funcsel[2];
    for (uint32_t i = 0u; i < 2u; i++) {
        if (gpio[i] >= GPIO_COUNT) {
            return false;
        }
        uint8_t func = gpio[i] % 4u;
        funcsel[i] = (func < 2u) ? FUNCSEL_UART : FUNCSEL_UART_AUX;
        tx[i] = (func % 2u) == 0u;
        uart[i] = (uint8_t)(((gpio[i] + 4u) >> 3) & 1u);
    }
    if ((uart[0] != uart[1]) || (tx[0] == tx[1])) {
        return false;
    }

    uint32_t t = tx[0] ? 0u : 1u;
    out->uart       = uart[0];
    out->tx_gpio    = gpio[t];
    out->tx_funcsel = funcsel[t];
    out->rx_gpio    = gpio[1u - t];
    out->rx_funcsel = funcsel[1u - t];
    return true;
}

// The board's X1 and X2 pins, or sel_a and sel_b on a board without both.
static bool board_pins(ora_get_metadata_uint_at_fn_t get_at, uint8_t *a, uint8_t *b) {
    uint32_t x1 = ORA_GPIO_NONE;
    uint32_t x2 = ORA_GPIO_NONE;
    if ((get_at(ORA_METADATA_KEY_GPIO_X1, 0u, &x1) == ORA_RESULT_OK) &&
        (get_at(ORA_METADATA_KEY_GPIO_X2, 0u, &x2) == ORA_RESULT_OK) &&
        (x1 != ORA_GPIO_NONE) && (x2 != ORA_GPIO_NONE)) {
        *a = (uint8_t)x1;
        *b = (uint8_t)x2;
        return true;
    }

    uint32_t sel_a = ORA_GPIO_NONE;
    uint32_t sel_b = ORA_GPIO_NONE;
    if ((get_at(ORA_METADATA_KEY_GPIO_SEL, 0u, &sel_a) != ORA_RESULT_OK) ||
        (get_at(ORA_METADATA_KEY_GPIO_SEL, 1u, &sel_b) != ORA_RESULT_OK) ||
        (sel_a == ORA_GPIO_NONE) || (sel_b == ORA_GPIO_NONE)) {
        return false;
    }
    *a = (uint8_t)sel_a;
    *b = (uint8_t)sel_b;
    return true;
}

static bool gpio_free(ora_gpio_query_fn_t query, uint8_t gpio) {
    ora_gpio_info_t info = {
        .size = sizeof(ora_gpio_info_t),
        .use = 0u,
        .level = 0u,
        .is_output = 0u,
    };
    return (query(gpio, &info) == ORA_RESULT_OK) && (info.use == ORA_GPIO_USE_FREE);
}

// The ring is at the start of a RAM slot.  The DMA channel wraps its write
// address on a boundary of the ring's size, so the slot must be aligned to it.
// Returns NULL, or why there isn't a ring.
static const char *ring_addr(ora_lookup_fn_t lookup, uint32_t *addr) {
    ora_get_ram_slot_count_fn_t ram_count = lookup(ORA_ID_GET_RAM_SLOT_COUNT);
    ora_get_ram_slot_info_fn_t ram_info = lookup(ORA_ID_GET_RAM_SLOT_INFO);
    ora_get_active_ram_slot_fn_t active_ram = lookup(ORA_ID_GET_ACTIVE_RAM_SLOT);
    if ((ram_count == NULL) || (ram_info == NULL) || (active_ram == NULL)) {
        return "plugin API call missing";
    }
    if (ram_count() < 3u) {
        return "fewer than three RAM slots";
    }
    uint8_t active = 0u;
    if (active_ram(&active) != ORA_RESULT_OK) {
        return "no RAM slot served";
    }
    uint32_t size = 0u;
    if (ram_info(datalog_ram_slot(active), addr, &size, NULL) != ORA_RESULT_OK) {
        return "RAM slot unavailable";
    }
    if ((size < RING_SIZE) || ((*addr % RING_SIZE) != 0u)) {
        return "RAM slot can't hold the ring";
    }
    return NULL;
}

// The UART's divisors in 64ths, rounded to the nearest.  See the UART's baud
// rate calculation in the RP2350 datasheet.
static void baud_divisors(uint32_t clk_hz, uint32_t *ibrd, uint32_t *fbrd) {
    uint32_t div128 = ((8u * clk_hz) / DATALOG_BAUD) + 1u;
    *ibrd = div128 >> 7;
    *fbrd = (div128 & 0x7Fu) >> 1;
}

// Returns NULL, or why the datalog can't start.
//
// Not inlined, so its frame is gone before the main loop runs.
static const char * __attribute__((noinline)) setup(ora_lookup_fn_t lookup) {
    ora_get_metadata_uint_at_fn_t get_at = lookup(ORA_ID_GET_METADATA_UINT_AT);
    ora_get_metadata_uint_fn_t get_uint = lookup(ORA_ID_GET_METADATA_UINT);
    ora_claim_hw_fn_t claim_hw = lookup(ORA_ID_CLAIM_HW);
    ora_claim_gpio_fn_t claim_gpio = lookup(ORA_ID_CLAIM_GPIO);
    ora_gpio_query_fn_t gpio_query = lookup(ORA_ID_GPIO_QUERY);
    ora_log_open_write_fn_t open_write = lookup(ORA_ID_LOG_OPEN_WRITE);
    ora_log_open_read_fn_t open_read = lookup(ORA_ID_LOG_OPEN_READ);
    if ((get_at == NULL) || (get_uint == NULL) || (claim_hw == NULL) ||
        (claim_gpio == NULL) || (gpio_query == NULL) || (open_write == NULL) ||
        (open_read == NULL) || (s_uptime == NULL) || (s_read == NULL) ||
        (s_write == NULL) || (s_query == NULL)) {
        return "plugin API call missing";
    }

    uint8_t a = 0u;
    uint8_t b = 0u;
    datalog_pins_t pins;
    if (!board_pins(get_at, &a, &b)) {
        return "board pins unknown";
    }
    if (!datalog_pins(a, b, &pins)) {
        return "pins can't be a UART's TX and RX";
    }
    if (!gpio_free(gpio_query, a) || !gpio_free(gpio_query, b)) {
        return "pins in use";
    }
    uint32_t ring = 0u;
    const char *why = ring_addr(lookup, &ring);
    if (why != NULL) {
        return why;
    }

    uint32_t clk_peri_hz = 0u;
    if (get_uint(ORA_METADATA_KEY_CLK_PERI_HZ, &clk_peri_hz) != ORA_RESULT_OK) {
        return "clk_peri's frequency unknown";
    }

    // The claims below can't be released, so the checks above come first.
    if ((open_write(CH_TO_PC, "datalog") != ORA_RESULT_OK) ||
        (open_read(CH_FROM_PC) != ORA_RESULT_OK)) {
        return "log channels 2 and 3 unavailable";
    }
    uint32_t uart = pins.uart;
    if (claim_hw(ORA_HW_UART, &uart, 0u) != ORA_RESULT_OK) {
        return "UART in use";
    }
    uint32_t ch = ORA_HW_ANY;
    if (claim_hw(ORA_HW_DMA_CHANNEL, &ch, 0u) != ORA_RESULT_OK) {
        return "no DMA channel free";
    }
    if ((claim_gpio(a, 0u) != ORA_RESULT_OK) || (claim_gpio(b, 0u) != ORA_RESULT_OK)) {
        return "pins in use";
    }

    uint32_t ibrd = 0u;
    uint32_t fbrd = 0u;
    baud_divisors(clk_peri_hz, &ibrd, &fbrd);

    s_dl.uart   = pins.uart;
    s_dl.dma_ch = (uint8_t)ch;
    s_dl.ring   = ORA_SRAM_PTR(ring);

    // The UART drives TX high once started, so TX doesn't glitch low when
    // its pin is connected.
    datalog_hw_uart_start(pins.uart, ibrd, fbrd);
    datalog_hw_rx_dma_start(s_dl.dma_ch, pins.uart, ring, RING_BITS, DMA_COUNT);
    datalog_hw_pin(pins.tx_gpio, pins.tx_funcsel, false);
    datalog_hw_pin(pins.rx_gpio, pins.rx_funcsel, true);

    s_log("Ostrich: datalog on UART%u, TX GPIO %u, RX GPIO %u", (unsigned)pins.uart,
          (unsigned)pins.tx_gpio, (unsigned)pins.rx_gpio);
    return NULL;
}

void datalog_start(ora_lookup_fn_t lookup) {
    s_dl.active      = false;
    s_dl.uart        = 0u;
    s_dl.dma_ch      = 0u;
    s_dl.ring        = NULL;
    s_dl.read        = 0u;
    s_dl.lost        = 0u;
    s_dl.lost_logged = 0u;
    s_dl.lost_log_ms = 0u;

    s_log    = lookup(ORA_ID_LOG);
    s_uptime = lookup(ORA_ID_GET_PLUGIN_UPTIME_MS);
    s_read   = lookup(ORA_ID_LOG_READ);
    s_write  = lookup(ORA_ID_LOG_WRITE);
    s_query  = lookup(ORA_ID_LOG_QUERY);
    ora_err_log_fn_t err = lookup(ORA_ID_ERR_LOG);
    if ((s_log == NULL) || (err == NULL)) {
        return;
    }

    const char *why = setup(lookup);
    if (why != NULL) {
        err("Ostrich: no datalog: %s", why);
        return;
    }
    s_dl.lost_log_ms = s_uptime();
    s_dl.active = true;
}

// Bytes received, modulo DMA_COUNT
static uint32_t received(void) {
    return (DMA_COUNT - datalog_hw_rx_dma_remaining(s_dl.dma_ch)) & DMA_COUNT_MASK;
}

static void ring_to_pc(void) {
    uint8_t buf[CHUNK];
    for (;;) {
        // The ring holds only the newest RING_SIZE bytes.
        uint32_t unread = (received() - s_dl.read) & DMA_COUNT_MASK;
        if (unread > RING_SIZE) {
            s_dl.lost += unread - RING_SIZE;
            s_dl.read = (s_dl.read + unread - RING_SIZE) & DMA_COUNT_MASK;
            unread = RING_SIZE;
        }

        uint32_t room = 0u;
        (void)s_query(CH_TO_PC, NULL, &room, NULL);
        uint32_t n = min_u32(min_u32(unread, room), CHUNK);
        if (n == 0u) {
            return;
        }
        for (uint32_t i = 0u; i < n; i++) {
            buf[i] = s_dl.ring[(s_dl.read + i) & (RING_SIZE - 1u)];
        }

        // The oldest bytes copied may have been overwritten during the copy.
        uint32_t ahead = (received() - s_dl.read) & DMA_COUNT_MASK;
        uint32_t stale = (ahead > RING_SIZE) ? min_u32(ahead - RING_SIZE, n) : 0u;
        s_dl.read = (s_dl.read + n) & DMA_COUNT_MASK;
        s_dl.lost += stale;
        if ((n > stale) &&
            (s_write(CH_TO_PC, buf + stale, n - stale) != ORA_RESULT_OK)) {
            s_dl.lost += n - stale;
        }
    }
}

static void pc_to_ecu(void) {
    uint8_t buf[CHUNK];
    for (;;) {
        uint32_t n = min_u32(datalog_hw_tx_room(s_dl.uart), CHUNK);
        uint32_t got = 0u;
        if ((n == 0u) ||
            (s_read(CH_FROM_PC, buf, n, &got) != ORA_RESULT_OK) || (got == 0u)) {
            return;
        }
        for (uint32_t i = 0u; i < got; i++) {
            datalog_hw_tx_write(s_dl.uart, buf[i]);
        }
    }
}

void datalog_service(void) {
    if (!s_dl.active) {
        return;
    }
    ring_to_pc();
    pc_to_ecu();

    uint32_t now = s_uptime();
    if ((s_dl.lost != s_dl.lost_logged) && ((now - s_dl.lost_log_ms) >= LOST_LOG_MS)) {
        s_log("Ostrich: datalog lost %u bytes, PC not reading",
              (unsigned)(s_dl.lost - s_dl.lost_logged));
        s_dl.lost_logged = s_dl.lost;
        s_dl.lost_log_ms = now;
    }
}

uint32_t datalog_lost(void) {
    return s_dl.lost;
}
