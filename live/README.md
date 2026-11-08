# Ostrich plugin live tests

This test harness sends Ostrich commands to One ROM and checks every reply
byte. It checks the served image and the saved ROM slots with `onerom`, and
the plugin's boot slot byte with probe-rs.

The tests overwrite the served image and both ROM slots.

The `interrupt` suite halts a save with a hardware breakpoint, resets the
RP2350 through the debug probe and checks that One ROM boots a complete image.
The reset doesn't cut the flash chip's power, so a cut inside a flash erase or
program isn't tested.

The `datalog` suite tests the datalog with a One ROM Lab as the ECU, including
during a save and while Lab reads the One ROM. `datalog.reads` is skipped
unless run with `--test datalog.reads`. A fire-28-c's USB can fail during this
test until the board is power cycled.

## Requirements

- A One ROM Fire programmed from the One ROM checkout at `build/one-rom`,
  with its firmware built by `make PLUGIN_LOGGING=1`. The save tests read the
  plugin's log. From the plugin's directory:

  ```bash
  onerom --serial <serial> program \
    --base-firmware build/one-rom/firmware/build/onerom-rp235x.bin \
    --plugin file=build/one-rom/plugins/system/usb/build/usb_system_plugin.bin \
    --plugin file=build/ostrich_plugin.bin \
    --slot file=a.bin,type=27256 --slot file=a.bin,type=27256
  ```

- A debug probe on the One ROM's SWD pins.
- For the `datalog` suite, a One ROM Lab with the One ROM plugged into its
  socket. Lab's `sel_a` connects to the One ROM pin for the ECU's transmit
  wire, and Lab's `sel_b` to the pin for its receive wire. The plugin's
  [README](../README.md#datalog) lists those pins.

- The One ROM checkout at `build/one-rom`, which the plugin's `make one-rom`
  creates. The harness reads values from its source when it builds, and runs
  `rust/target/debug/onerom` from it unless `--onerom` is set.

## Running

From `live/`:

```bash
cargo run -- --serial 0123456789ABCDEF --board fire-28-c
```

`--serial` is One ROM's USB serial number from `onerom scan`. `--probe`
selects a debug probe where more than one is attached. `--lab` is the Lab's
USB serial number, and the `datalog` suite is skipped without it.

Each test prints `PASS`, `FAIL` with the evidence, or `SKIP` with the reason,
and the run ends with a count. The exit status is 1 if any test failed.

To run one test:

```bash
cargo run -- --serial 0123456789ABCDEF --board fire-28-c --test save.reboot
```

`--test` also accepts a suite, for example `--test save`, and can be repeated.
`--list` lists the tests.
