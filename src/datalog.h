// The ECU's serial port, bridged to the USB plugin's serial port 2.

#if !defined(DATALOG_H)
#define DATALOG_H

#include <stdint.h>
#include <stdbool.h>

#include <plugin.h>

// A UART and the GPIOs connected to its TX and RX
typedef struct {
    uint8_t uart;
    uint8_t tx_gpio;
    uint8_t tx_funcsel;
    uint8_t rx_gpio;
    uint8_t rx_funcsel;
} datalog_pins_t;

// Whether one of the GPIOs can be a UART's TX and the other the same UART's
// RX, in either order.
bool datalog_pins(uint8_t gpio_a, uint8_t gpio_b, datalog_pins_t *out);

// Start the datalog, or log why it can't start.  Called once at entry, before
// any other datalog call.
void datalog_start(ora_lookup_fn_t lookup);

// Move ECU bytes to the PC and the PC's bytes to the ECU.  Does nothing if the
// datalog didn't start.
void datalog_service(void);

// Bytes from the ECU dropped because the PC didn't read them in time
uint32_t datalog_lost(void);

#endif // DATALOG_H
