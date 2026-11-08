// The RP2350 registers the datalog uses.  See datalog_hw.h.  A host build
// doesn't compile this file.

#include <stdint.h>
#include <stdbool.h>

#include "datalog_hw.h"

#define REG(addr)               (*(volatile uint32_t *)(uintptr_t)(addr))

// A write to a register's address plus SET_ALIAS or CLR_ALIAS sets or clears
// only the bits written.  Core 1 changes other bits of RESETS_RESET.
#define SET_ALIAS               0x2000u
#define CLR_ALIAS               0x3000u

#define RESETS_RESET            0x40020000u
#define RESETS_DONE             0x40020008u
#define RESETS_UART0            (1u << 26)

#define GPIO_CTRL(gpio)         (0x40028004u + ((uint32_t)(gpio) * 8u))
#define GPIO_CTRL_FUNCSEL_MASK  0x1Fu

#define PAD(gpio)               (0x40038004u + ((uint32_t)(gpio) * 4u))
#define PAD_PDE                 (1u << 2)
#define PAD_PUE                 (1u << 3)
#define PAD_IE                  (1u << 6)
#define PAD_OD                  (1u << 7)
#define PAD_ISO                 (1u << 8)

#define UART_BASE(uart)         (0x40070000u + ((uint32_t)(uart) * 0x8000u))
#define UART_DR                 0x00u
#define UART_FR                 0x18u
#define UART_IBRD               0x24u
#define UART_FBRD               0x28u
#define UART_LCR_H              0x2Cu
#define UART_CR                 0x30u
#define UART_DMACR              0x48u
#define UART_FR_TXFF            (1u << 5)
#define UART_FR_TXFE            (1u << 7)
#define UART_LCR_H_FEN          (1u << 4)
#define UART_LCR_H_WLEN_8       (3u << 5)
#define UART_CR_UARTEN          (1u << 0)
#define UART_CR_TXE             (1u << 8)
#define UART_CR_RXE             (1u << 9)
#define UART_DMACR_RXDMAE       (1u << 0)
#define UART_FIFO_DEPTH         32u

#define DMA_CH(ch)              (0x50000000u + ((uint32_t)(ch) * 0x40u))
#define DMA_READ_ADDR           0x00u
#define DMA_WRITE_ADDR          0x04u
#define DMA_TRANS_COUNT         0x08u
#define DMA_CTRL_TRIG           0x0Cu
#define DMA_TRANS_COUNT_SELF    (1u << 28)
#define DMA_TRANS_COUNT_MASK    0x0FFFFFFFu
#define DMA_CTRL_EN             (1u << 0)
#define DMA_CTRL_INCR_WRITE     (1u << 6)
#define DMA_CTRL_RING_SIZE(n)   ((uint32_t)(n) << 8)
#define DMA_CTRL_RING_SEL_WRITE (1u << 12)
#define DMA_CTRL_CHAIN_TO(ch)   ((uint32_t)(ch) << 13)
#define DMA_CTRL_TREQ_SEL(dreq) ((uint32_t)(dreq) << 17)
#define DMA_CTRL_IRQ_QUIET      (1u << 23)
#define DREQ_UART_RX(uart)      (29u + ((uint32_t)(uart) * 2u))

void datalog_hw_uart_start(uint8_t uart, uint32_t ibrd, uint32_t fbrd) {
    uint32_t bit = RESETS_UART0 << uart;
    REG(RESETS_RESET + SET_ALIAS) = bit;
    REG(RESETS_RESET + CLR_ALIAS) = bit;
    while (!(REG(RESETS_DONE) & bit)) {}

    // The divisors take effect on the write to LCR_H.
    uint32_t base = UART_BASE(uart);
    REG(base + UART_IBRD) = ibrd;
    REG(base + UART_FBRD) = fbrd;
    REG(base + UART_LCR_H) = UART_LCR_H_WLEN_8 | UART_LCR_H_FEN;
    REG(base + UART_CR) = UART_CR_UARTEN | UART_CR_TXE | UART_CR_RXE;
    REG(base + UART_DMACR) = UART_DMACR_RXDMAE;
}

// Any input override the firmware set on the GPIO is kept.
void datalog_hw_pin(uint8_t gpio, uint8_t funcsel, bool rx) {
    uint32_t pad = REG(PAD(gpio)) & ~(PAD_ISO | PAD_OD);
    if (rx) {
        pad = (pad & ~PAD_PDE) | PAD_IE | PAD_PUE;
    }
    REG(PAD(gpio)) = pad;
    REG(GPIO_CTRL(gpio)) = (REG(GPIO_CTRL(gpio)) & ~GPIO_CTRL_FUNCSEL_MASK) | funcsel;
}

void datalog_hw_rx_dma_start(
    uint8_t  ch,
    uint8_t  uart,
    uint32_t ring_addr,
    uint8_t  ring_bits,
    uint32_t count
) {
    uint32_t base = DMA_CH(ch);
    REG(base + DMA_READ_ADDR) = UART_BASE(uart) + UART_DR;
    REG(base + DMA_WRITE_ADDR) = ring_addr;
    REG(base + DMA_TRANS_COUNT) = DMA_TRANS_COUNT_SELF | count;
    REG(base + DMA_CTRL_TRIG) = DMA_CTRL_EN
                              | DMA_CTRL_INCR_WRITE
                              | DMA_CTRL_RING_SIZE(ring_bits)
                              | DMA_CTRL_RING_SEL_WRITE
                              | DMA_CTRL_CHAIN_TO(ch)
                              | DMA_CTRL_TREQ_SEL(DREQ_UART_RX(uart))
                              | DMA_CTRL_IRQ_QUIET;
}

uint32_t datalog_hw_rx_dma_remaining(uint8_t ch) {
    return REG(DMA_CH(ch) + DMA_TRANS_COUNT) & DMA_TRANS_COUNT_MASK;
}

uint32_t datalog_hw_tx_room(uint8_t uart) {
    uint32_t fr = REG(UART_BASE(uart) + UART_FR);
    if (fr & UART_FR_TXFE) {
        return UART_FIFO_DEPTH;
    }
    return (fr & UART_FR_TXFF) ? 0u : 1u;
}

void datalog_hw_tx_write(uint8_t uart, uint8_t b) {
    REG(UART_BASE(uart) + UART_DR) = b;
}
