/* Ostrich user plugin linker script. */

INCLUDE onerom_linker_constants_generated.ld

USER_PLUGIN_BASE = ORA_FLASH_CS0_BASE_ADDR + ORA_USER_PLUGIN_OFFSET;

/* 1KB of RAM shared between static data and the stack, which grows down from
 * the top. */
USER_PLUGIN_DATA_BASE = 0x20081C00;
USER_PLUGIN_STACK_TOP = 0x20082000;

SECTIONS
{
    . = USER_PLUGIN_BASE;

    .plugin_header : {
        KEEP(*(.plugin_header))
        . = ALIGN(4);
    }

    ASSERT(SIZEOF(.plugin_header) > 0, "ERROR: .plugin_header section is empty")

    .peer_data : {
        KEEP(*(.peer_data))
    }

    .control_port_desc : {
        KEEP(*(.control_port_desc))
    }

    .datalog_port_desc : {
        KEEP(*(.datalog_port_desc))
    }

    /* The C source expects the peer data at the end of the 256-byte header and
     * each name at the end of the section before it. */
    ASSERT(ADDR(.peer_data) == USER_PLUGIN_BASE + 256,
           "ERROR: the peer data isn't at the address in peer_data")
    ASSERT(ADDR(.control_port_desc) == ADDR(.peer_data) + SIZEOF(.peer_data),
           "ERROR: serial port 1's description isn't at the address in the peer data block")
    ASSERT(ADDR(.datalog_port_desc) == ADDR(.control_port_desc) + SIZEOF(.control_port_desc),
           "ERROR: serial port 2's description isn't at the address in the peer data block")

    .text : {
        *(.text*)
        *(.rodata*)
        . = ALIGN(4);
        __flash_write_fn_start = .;
        KEEP(*(.flash_write_fn*))
        . = ALIGN(4);
        __flash_write_fn_end = .;
        __text_end = .;
    }

    /* 0xFF from the end of the plugin to the end of the user plugin region so
     * programming the plugin leaves its NV storage erased.  The linker doesn't
     * output a section holding only fill, so the last byte is written
     * explicitly. */
    .padding (__text_end + SIZEOF(.data)) : {
        FILL(0xFF);
        . = ABSOLUTE(USER_PLUGIN_BASE + ORA_USER_PLUGIN_SIZE - 1);
        BYTE(0xFF)
    }

    .data USER_PLUGIN_DATA_BASE : AT(__text_end) {
        __data_start = .;
        *(.data*)
        . = ALIGN(4);
        __data_end = .;
    }
    __data_load = LOADADDR(.data);

    .bss (NOLOAD) : {
        __bss_start = .;
        *(.bss*)
        *(COMMON)
        . = ALIGN(4);
        __bss_end = .;
    }

    /DISCARD/ : {
        *(.ARM.exidx*)
        *(.ARM.extab*)
    }
}

/* The firmware reads the boot slot byte at the address in the plugin header,
 * which the C source computes from the same constants. */
__nv_storage_start = USER_PLUGIN_BASE + ORA_USER_PLUGIN_SIZE - 4K;
ASSERT(__text_end + SIZEOF(.data) <= __nv_storage_start,
       "ERROR: plugin code and data overlap NV storage")

ASSERT(__flash_write_fn_end - __flash_write_fn_start > 0,
       "ERROR: flash write routine is empty")

/* The Makefile also defines FLASH_WRITE_FN_MAX for the compiler.  It sets the
 * size of the stack buffer the routine is copied into. */
ASSERT(__flash_write_fn_end - __flash_write_fn_start <= FLASH_WRITE_FN_MAX,
       "ERROR: flash write routine exceeds FLASH_WRITE_FN_MAX")

/* PLUGIN_STATIC_MAX comes from the Makefile.  The rest of the 1KB is stack. */
ASSERT(__bss_end - USER_PLUGIN_DATA_BASE <= PLUGIN_STATIC_MAX,
       "ERROR: static data exceeds PLUGIN_STATIC_MAX")
ASSERT(__bss_end <= USER_PLUGIN_STACK_TOP,
       "ERROR: static data exceeds the plugin's RAM")
