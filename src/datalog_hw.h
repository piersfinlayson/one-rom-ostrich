// The RP2350 hardware the datalog uses.  src/datalog_hw.c implements these on
// a device.  The tester implements them with a model of the UART and the DMA
// channel.

#if !defined(DATALOG_HW_H)
#define DATALOG_HW_H

#include <stdint.h>
#include <stdbool.h>

// Reset the UART and start it 8N1 with the baud rate divisors provided and its
// RX DMA request enabled.
void datalog_hw_uart_start(uint8_t uart, uint32_t ibrd, uint32_t fbrd);

// Connect a GPIO to a UART.  An RX GPIO's input is enabled and pulled up.
void datalog_hw_pin(uint8_t gpio, uint8_t funcsel, bool rx);

// Start a DMA channel copying the UART's received bytes into the ring.  The
// ring is 1 << ring_bits bytes and aligned to its size.  The channel restarts
// itself after count bytes.
void datalog_hw_rx_dma_start(
    uint8_t  ch,
    uint8_t  uart,
    uint32_t ring_addr,
    uint8_t  ring_bits,
    uint32_t count
);

// Bytes the DMA channel has left to copy before it restarts
uint32_t datalog_hw_rx_dma_remaining(uint8_t ch);

// Bytes that fit in the UART's TX FIFO now.  The UART's flags indicate only
// whether the FIFO is empty or full, so this is the whole FIFO, 1 or 0.
uint32_t datalog_hw_tx_room(uint8_t uart);

void datalog_hw_tx_write(uint8_t uart, uint8_t b);

#endif // DATALOG_HW_H
