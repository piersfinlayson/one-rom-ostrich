// The RAM slots Ostrich uses besides the one One ROM serves.  Write data is
// staged in one and the datalog's ring is in another.  Both are chosen here
// so they can't be the same slot.

#if !defined(RAM_SLOTS_H)
#define RAM_SLOTS_H

#include <stdint.h>

static inline uint8_t staging_ram_slot(uint8_t active) {
    return (active == 0u) ? 1u : 0u;
}

// The lowest slot that is neither served nor the staging slot.  The device
// must have at least three RAM slots.
static inline uint8_t datalog_ram_slot(uint8_t active) {
    uint8_t staging = staging_ram_slot(active);
    uint8_t slot = 0u;
    while ((slot == active) || (slot == staging)) {
        slot++;
    }
    return slot;
}

#endif // RAM_SLOTS_H
