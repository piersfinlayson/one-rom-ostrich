# Builds the Ostrich user plugin for One ROM.
#
# Usage:
#   make TOOLCHAIN=<arm-gnu-toolchain>/bin
#   make one-rom
#   make one-rom ONEROM_DIR=<one-rom checkout>
#
# The plugin builds against the One ROM checkout at build/one-rom, and the
# tester and live harness use the same checkout.  It is a link to the first one
# of the following that is detected:
# - the One ROM checkout around this plugin if it located at
#   plugins/user/ostrich
# - the ONEROM_DIR env variable
# - One ROM commit ONEROM_REF, fetched into build/one-rom-<ONEROM_REF>
# The first build creates the link and make one-rom can be used to recreate it.

TOOLCHAIN ?=
CC      := $(if $(TOOLCHAIN),$(TOOLCHAIN)/arm-none-eabi-gcc,arm-none-eabi-gcc)
OBJCOPY := $(if $(TOOLCHAIN),$(TOOLCHAIN)/arm-none-eabi-objcopy,arm-none-eabi-objcopy)
OBJDUMP := $(if $(TOOLCHAIN),$(TOOLCHAIN)/arm-none-eabi-objdump,arm-none-eabi-objdump)

MAJOR_VERSION := 0
MINOR_VERSION := 1
PATCH_VERSION := 0
BUILD_VERSION := 0

# Bytes of stack the flash write routine is copied into.  The linker script
# checks the routine fits.
FLASH_WRITE_FN_MAX := 80

# Bytes of static data.  The rest of the plugin's 1KB of RAM is stack.
PLUGIN_STATIC_MAX := 256

ONEROM_REF  ?= 1b189260d85a5feced19e969399986f5f1d1cabe
ONEROM_URL  := https://github.com/piersfinlayson/one-rom
ONEROM_LINK := build/one-rom
ONEROM_TREE := $(abspath ../../..)
# wildcard expands a leading ~
ONEROM_DIR_ABS := $(abspath $(firstword $(wildcard $(ONEROM_DIR))))
ORA_DIR     := $(ONEROM_LINK)/firmware/ora
USB_DIR     := $(ONEROM_LINK)/plugins/system/usb/include

BUILD_DIR     := build
PLUGIN_PREFIX := ostrich_plugin

ELF := $(BUILD_DIR)/$(PLUGIN_PREFIX).elf
BIN := $(BUILD_DIR)/$(PLUGIN_PREFIX).bin

SRC := src/ostrich_main.c src/datalog.c src/datalog_hw.c

FLASH_WRITE_SRC := src/flash_write.c
FLASH_WRITE_OBJ := $(BUILD_DIR)/src/flash_write.o

VERSION_FLAGS := -DMAJOR_VERSION=$(MAJOR_VERSION) -DMINOR_VERSION=$(MINOR_VERSION) \
                 -DPATCH_VERSION=$(PATCH_VERSION) -DBUILD_VERSION=$(BUILD_VERSION) \
                 -DFLASH_WRITE_FN_MAX=$(FLASH_WRITE_FN_MAX)u

# Without a C library, loops must not become memcpy or memset calls.
CFLAGS := -mcpu=cortex-m33 -mthumb -mfloat-abi=hard -mfpu=fpv5-sp-d16 \
          -std=c11 -fstack-usage -fconserve-stack \
          -fno-tree-loop-distribute-patterns \
          -O2 -Wall -Wextra -Werror \
          -ffunction-sections -fdata-sections \
          -MMD -MP \
          $(VERSION_FLAGS) \
          -I$(ORA_DIR) -I$(USB_DIR) -Isrc

LDFLAGS := -nostdlib \
           -L $(ORA_DIR) -T ostrich_plugin.ld \
           -Wl,--defsym=FLASH_WRITE_FN_MAX=$(FLASH_WRITE_FN_MAX) \
           -Wl,--defsym=PLUGIN_STATIC_MAX=$(PLUGIN_STATIC_MAX) \
           -Wl,--fatal-warnings \
           -Wl,--no-warn-rwx-segments \
           -Wl,-Map=$(BUILD_DIR)/$(PLUGIN_PREFIX).map

PLUGIN_OBJ := $(patsubst %.c,$(BUILD_DIR)/%.o,$(SRC))
ALL_OBJ    := $(PLUGIN_OBJ) $(FLASH_WRITE_OBJ)

# Host build
#
# Compiles the plugin natively for the emulator tester in tester/.  The result
# is objects, not a plugin binary.  The tester provides datalog_hw.h's
# functions.  -fshort-enums matches the firmware's native test build, which the
# objects link against.

HOST_CC           ?= cc
HOST_BUILD_DIR    ?= build-host
HOST_EXTRA_CFLAGS ?=
HOST_SRC          := src/ostrich_main.c src/datalog.c $(FLASH_WRITE_SRC)
HOST_ALL_OBJ      := $(patsubst src/%.c,$(HOST_BUILD_DIR)/%.o,$(HOST_SRC))

HOST_CFLAGS := -DORA_HOST_TEST=1 -fshort-enums \
               -std=c11 -O1 -g -Wall -Wextra -Werror \
               $(VERSION_FLAGS) \
               -I$(ORA_DIR) -I$(USB_DIR) -Isrc \
               -MMD -MP $(HOST_EXTRA_CFLAGS)

# Objects are rebuilt when their flags change.  A stamp is rewritten only when
# the flags differ from the last build.
CFLAGS_STAMP := $(BUILD_DIR)/.cflags
HOST_CFLAGS_STAMP := $(HOST_BUILD_DIR)/.cflags
ifneq ($(filter host,$(MAKECMDGOALS)),)
$(shell mkdir -p $(HOST_BUILD_DIR); \
        printf '%s' '$(HOST_CC) $(HOST_CFLAGS)' | cmp -s - $(HOST_CFLAGS_STAMP) || \
        printf '%s' '$(HOST_CC) $(HOST_CFLAGS)' > $(HOST_CFLAGS_STAMP))
else
$(shell mkdir -p $(BUILD_DIR); \
        printf '%s' '$(CC) $(CFLAGS)' | cmp -s - $(CFLAGS_STAMP) || \
        printf '%s' '$(CC) $(CFLAGS)' > $(CFLAGS_STAMP))
endif

.PHONY: all clean distclean host clean-host one-rom

all: $(BIN)

host: $(HOST_ALL_OBJ)

one-rom:
	@rm -f $(ONEROM_LINK)
	@$(MAKE) --no-print-directory $(ONEROM_LINK)

$(ONEROM_LINK):
	@mkdir -p $(dir $@)
	@rm -f $@
ifneq ($(wildcard $(ONEROM_TREE)/firmware/ora/plugin.h),)
	@ln -s $(ONEROM_TREE) $@
else ifneq ($(ONEROM_DIR),)
	@test -n "$(ONEROM_DIR_ABS)" || { echo "ONEROM_DIR $(ONEROM_DIR) not found"; exit 1; }
	@ln -s $(ONEROM_DIR_ABS) $@
else
	@if [ ! -d build/one-rom-$(ONEROM_REF) ]; then \
		echo "Fetching One ROM $(ONEROM_REF)..."; \
		rm -rf build/one-rom-$(ONEROM_REF).partial; \
		git init --quiet build/one-rom-$(ONEROM_REF).partial && \
		git -C build/one-rom-$(ONEROM_REF).partial fetch --quiet --depth 1 \
			$(ONEROM_URL) $(ONEROM_REF) && \
		git -C build/one-rom-$(ONEROM_REF).partial -c advice.detachedHead=false \
			checkout --quiet FETCH_HEAD && \
		mv build/one-rom-$(ONEROM_REF).partial build/one-rom-$(ONEROM_REF); \
	fi
	@ln -s $(abspath build/one-rom-$(ONEROM_REF)) $@
endif
	@echo "One ROM checkout: $$(readlink $@)"

$(HOST_BUILD_DIR):
	@mkdir -p $@

$(HOST_BUILD_DIR)/%.o: src/%.c $(HOST_CFLAGS_STAMP) | $(HOST_BUILD_DIR) $(ONEROM_LINK)
	@echo "Compiling $< for host..."
	@$(HOST_CC) $(HOST_CFLAGS) -c $< -o $@

clean-host:
	@rm -rf $(HOST_BUILD_DIR)

# The routine is copied onto the stack and run there so it is compiled
# position independent.
$(FLASH_WRITE_OBJ): $(FLASH_WRITE_SRC) $(CFLAGS_STAMP) | $(BUILD_DIR) $(ONEROM_LINK)
	@mkdir -p $(dir $@)
	@echo "Compiling $< (position independent)..."
	@$(CC) $(CFLAGS) -fPIC -fno-plt -c $< -o $@

$(BUILD_DIR)/%.o: %.c $(CFLAGS_STAMP) | $(BUILD_DIR) $(ONEROM_LINK)
	@mkdir -p $(dir $@)
	@echo "Compiling $<..."
	@$(CC) $(CFLAGS) -c $< -o $@

$(ELF): $(ALL_OBJ) ostrich_plugin.ld $(ORA_DIR)/onerom_linker_constants_generated.ld
	@echo "Linking..."
	@$(CC) $(CFLAGS) $(LDFLAGS) $(ALL_OBJ) -lgcc -o $@

$(BIN): $(ELF)
	@echo "Converting ELF to binary..."
	@$(OBJCOPY) -O binary $< $@
	@$(OBJDUMP) -s -j .plugin_header $(ELF) > $(BUILD_DIR)/$(PLUGIN_PREFIX).dis
	@$(OBJDUMP) -d -S $(ELF) >> $(BUILD_DIR)/$(PLUGIN_PREFIX).dis

$(BUILD_DIR):
	@mkdir -p $@

clean: clean-host
	@rm -rf $(BUILD_DIR)

distclean: clean

-include $(ALL_OBJ:.o=.d) $(HOST_ALL_OBJ:.o=.d)
