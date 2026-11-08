// Erase and program flash from a routine copied into RAM.

#if !defined(FLASH_WRITE_H)
#define FLASH_WRITE_H

#include <stdint.h>

typedef void (*flash_exit_xip_fn_t)(void);
typedef void (*flash_range_erase_fn_t)(uint32_t offs, uint32_t count, uint32_t block_size, uint8_t block_cmd);
typedef void (*flash_flush_cache_fn_t)(void);
typedef void (*flash_select_xip_read_mode_fn_t)(uint8_t mode, uint8_t clkdiv);
typedef void (*flash_range_program_fn_t)(uint32_t offs, const uint8_t *data, uint32_t count);
typedef void (*connect_internal_flash_fn_t)(void);

// Mask this core's interrupts and restore them.  PRIMASK is saved and
// restored so masked regions can nest.
//
// A host build doesn't have cpsid or cpsie, so the test harness provides
// these.  It counts the depth and records a flash call made outside a masked
// region.
#if defined(ORA_HOST_TEST)
uint32_t ora_host_test_irq_disable(void);
void ora_host_test_irq_restore(uint32_t primask);
static inline uint32_t flash_irq_disable(void) {
    return ora_host_test_irq_disable();
}
static inline void flash_irq_restore(uint32_t primask) {
    ora_host_test_irq_restore(primask);
}
#else
static inline uint32_t flash_irq_disable(void) {
    uint32_t primask;
    __asm volatile ("mrs %0, primask \n\t"
                    "cpsid i"
                    : "=r" (primask) :: "memory");
    return primask;
}
static inline void flash_irq_restore(uint32_t primask) {
    __asm volatile ("msr primask, %0" :: "r" (primask) : "memory");
}
#endif

typedef void (*flash_write_critical_fn_t)(
    flash_exit_xip_fn_t             exit_xip,
    flash_range_erase_fn_t          range_erase,
    flash_range_program_fn_t        range_program,
    flash_flush_cache_fn_t          flush_cache,
    flash_select_xip_read_mode_fn_t select_xip,
    uint32_t                        erase_offs,
    uint32_t                        program_offs,
    const uint8_t                  *data,
    uint32_t                        erase_size,
    uint32_t                        program_size,
    uint8_t                         clkdiv
);

void flash_write_critical(
    flash_exit_xip_fn_t             exit_xip,
    flash_range_erase_fn_t          range_erase,
    flash_range_program_fn_t        range_program,
    flash_flush_cache_fn_t          flush_cache,
    flash_select_xip_read_mode_fn_t select_xip,
    uint32_t                        erase_offs,
    uint32_t                        program_offs,
    const uint8_t                  *data,
    uint32_t                        erase_size,
    uint32_t                        program_size,
    uint8_t                         clkdiv
);

#endif // FLASH_WRITE_H
