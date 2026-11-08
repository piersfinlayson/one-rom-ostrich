# Ostrich Plugin for One ROM

## Disclaimer

<em>One ROM was originally designed to replace ROMs in retro computers, and
was not designed to be run in automotive applications like car ECUs. There
is no warranty provided for One ROM or this plugin. Use at your own
risk.</em>

## Background

This is a plugin that can be added to [One ROM](https://onerom.org) to emulate
a Moates Ostrich 2.0 device over USB.

It is provided as a reference plugin to demonstrate best practice in using the
One ROM plugin API, allowing the Ostrich function to be added to One ROM in a
forwards compatible way, so that it should work with future versions of the One
ROM firmware and hardware.

It does not claim to be a complete Ostrich implementation, nor should it be
considered a reference implementation of one.

## Introduction

ECU tuning software reads and edits a ROM image while the car's engine runs,
updating the engine map. It can also view a live datalog from the ECU.  This
plugin extends One ROM to provide this functionality with tuning software that
supports Ostrich 2.0, including:
- TunerPro
- CROME
- Honda Tuning Suite
- ORSS

An example target is a Honda OBD1 ECU with a 27C256 ROM.

## Plugin Requirements

- Any One ROM Fire 28.  Also supports One ROM Fire 24 rev C onwards.
- One ROM firmware v0.8.1 onwards.
- The One ROM USB plugin.
- One ROM configured with two ROM slots of the same ROM type.

## Programming

### Web

The easiest way to program the plugin is using the
[Web programmer](https://onerom.org/web):
- Open in Chrome and select the "One ROM Builder" tab.
- Plug in your One ROM using USB and press "Detect" to populate the Device
  Configuration.
- Upload a starting map file for your ECU in Slot 0.
- Select the ROM type your ECU supports.  This is likely to be 27C256 (32KB)
  or 27C512 (64KB).
- Add a ROM Slot and configure Slot 1 as you did Slot 0 - the same map file and
  the same ROM type.
- Under plugins select:
  - System Plugin: One ROM USB (...)
  - User Plugin: Ostrich (...)
- Press "Build Firmware".
- Press "Program".
- Follow any on-screen instructions.

### Command Line Interface

You can also program using the [One ROM CLI](https://onerom.org/cli).

If your starting map file is called `map.bin` and your ECU's ROM type is a
27C256 use:

```bash
onerom program --plugin usb \
               --plugin ostrich \
               --slot file=map.bin,type=27C256 \
               --slot file=map.bin,type=27C256
```

On Windows use `onerom.exe` instead of `onerom`.

## Usage

### Installing One ROM into the ECU

Install One ROM in the ECU's ROM socket, ensuring that pin 1 of One ROM is
in pin 1 of the ROM socket.  There is a white marker at the top left of One
ROM (at the opposite end to its USB port), indicating pin 1.

### Tuning Software

With this plugin One ROM exposes two serial ports (known as COM ports on
Windows).  These are:
- One ROM Ostrich - used by your tuning software to control One ROM
- One ROM Ostrich Datalog - the ECU's datalog

The ECU's datalog serial settings must be 38400 8N1.

### Datalog Wiring

For datalogging you must wire three wires from your ECU to One ROM:
- ECU TX
- ECU RX
- GND

Where you wire these to on One ROM depends on your One ROM model.  To identify
your One ROM model visit [this page](https://onerom.org/identify).

Fire 28 A/B:
- ECU TX connects to SEL B
  - <em>IMPORTANT</em> - The ECU TX line must be connected to One ROM using a
    logic level shifter or resistive divider, as these pins on a Fire 28 A/B
    can only tolerate 3.3V, not the 5V the ECU supplies.
- ECU RX connects to SEL A
- GND connects to a GND pin

Fire 28 C onwards:
- ECU TX connects to X1
- ECU RX connects to X2
- GND connects to a GND pin

Fire 24 C onwards:
- ECU TX connects to X1
- ECU RX connects to X2
- GND connects to a GND pin

You may be able to omit the GND connection, as the ECU already provides a GND
to One ROM via pin 14 (One ROM 28)/pin 12 (One ROM 24).

To identify which pin is which on your One ROM, install the
[One ROM CLI](https://onerom.org/cli) and, with your One ROM plugged into USB,
run:

```bash
onerom inspect header
```

### Saving Map Changes

When editing engine maps using your tuning software, One ROM immediately
serves the changes to your ECU.  It saves them to One ROM's non-volatile
storage 3s after the last change.  This reduces wear and tear on One ROM's
storage.

One ROM holds two copies of the map.  It boots the copy saved last and saves
changes to the other copy.

If One ROM loses power while saving, it may boot the old copy next time. If
this happens you will need to apply your changes again.

One ROM's image select jumpers do not select the image for One ROM to serve
when using the Ostrich plugin.

## Problems

If you hit a problem, the first thing you should do is read One ROM's log
using the [CLI](https://onerom.org/cli) and command:

```bash
onerom monitor log
```

On Windows use `onerom.exe` instead of `onerom`.

## Building and Development

To build the Ostrich plugin or do other development on it, see
[DEVELOPMENT.md](./DEVELOPMENT.md).

## License

This plugin is licensed under the [MIT License](./LICENSE) and no warranty of
any kind is provided.