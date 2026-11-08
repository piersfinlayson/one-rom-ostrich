// The plugin copies this routine onto its stack and runs it there because
// flash can't be read while it is erased or programmed.  It is compiled with
// -fPIC -fno-plt and uses only its arguments so the copy runs at any address.
// After a change, check build/ostrich_plugin.dis for an absolute address in
// the routine.

#include <stdint.h>

#include <plugin.h>

#include "flash_write.h"

// 4KB sector erase.  The bootrom uses the block command only for a range
// aligned to the block size, so passing the sector size and command keeps
// every erase to whole 4KB sectors.
#define FLASH_SECTOR_SIZE       4096u
#define FLASH_SECTOR_ERASE_CMD  0x20u

// The XIP read mode for EBh quad I/O.
#define FLASH_XIP_MODE_QUAD     3u

// The erase and the program run between one exit from XIP and one restore of
// it.  The bootrom's program function requires flash in serial command mode
// as the erase does.
//
// Interrupts stay masked throughout because every handler on this core is in
// flash.  The caller has already masked them.  Masking again here keeps the
// routine safe to call on its own.
ORA_SECTION(".flash_write_fn") __attribute__((noinline))
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
) {
    uint32_t primask = flash_irq_disable();
    exit_xip();
    range_erase(erase_offs, erase_size, FLASH_SECTOR_SIZE, FLASH_SECTOR_ERASE_CMD);
    range_program(program_offs, data, program_size);
    select_xip(FLASH_XIP_MODE_QUAD, clkdiv);
    flush_cache();
    flash_irq_restore(primask);
}
