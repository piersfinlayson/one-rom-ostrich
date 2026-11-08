# Development

## Building

```bash
make TOOLCHAIN=<arm-gnu-toolchain>/bin
```

This builds `build/ostrich_plugin.bin`. Use the Arm GNU toolchain version in
One ROM's `ci/arm-toolchain-version`.

The plugin builds against the One ROM checkout at `build/one-rom`, which the
tester and `live/` also use. The first build creates it as a link to the first
of:

- the One ROM checkout around this plugin, at `plugins/user/ostrich`
- `ONEROM_DIR`
- One ROM commit `ONEROM_REF`, downloaded into `build/`

`make one-rom` recreates the link, for example
`make one-rom ONEROM_DIR=<one-rom checkout>`.

## Testing

`tester/` runs the plugin's C natively against the One ROM firmware emulator,
with models of the flash and of the datalog's UART and DMA. It uses the One
ROM checkout at `build/one-rom` and builds the firmware's C test library in
that location.

From this directory:

```bash
make one-rom
BOARD=fire-28-a CONFIG=$PWD/tester/configs/28-ostrich-27256.json \
    cargo run --manifest-path tester/Cargo.toml
```

Pass `--suite <name>` or `--scenario <substring>` after `--` to select
scenarios.

| Config | Device |
| --- | --- |
| `24-ostrich-2732.json` | Two 2732 slots |
| `28-ostrich-27256.json` | Two 27256 slots |
| `28-ostrich-27512.json` | Two 27512 slots |
| `28-ostrich-2764.json` | Two 2764 slots |
| `28-ostrich-mismatch.json` | Two slots of different chip types, unsupported |
| `28-ostrich-single.json` | One ROM slot, unsupported |
| `28-ostrich-three.json` | Three ROM slots, unsupported |
| `28-ostrich-231024.json` | Two 128KB slots, unsupported |
| `40-ostrich-27c200.json` | Two 27C200 slots and no spare RAM slot, unsupported |

`live/` tests the plugin on a One ROM over USB. See `live/README.md`.
