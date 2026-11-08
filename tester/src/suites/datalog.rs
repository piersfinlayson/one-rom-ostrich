//! The datalog bridges the ECU's serial port to serial port 2, and Ostrich
//! works whether or not the datalog can start.

use std::ffi::CStr;

use onerom_config::hw::Board;
use onerom_fw_emulator::ffi as fw;
use onerom_plugin_tester::run::Outcome;

use super::commands::layout_or_skip;
use super::save::{edit, save};
use super::{check, scenario, scenario_arranged};
use crate::device::{CH_FROM_PC, CH_TO_PC, Device};
use crate::ffi::{self, DatalogHw, DatalogPins, OraValues, PluginSlot};
use crate::proto::{self, OK, VERSION};
use crate::{Ctx, Scenario};

pub const SCENARIOS: &[Scenario] = &[
    scenario(
        "datalog.board_pins",
        "the UART is on X1 and X2, or on sel_a and sel_b on a board without both, \
         unless One ROM uses either pin, both pins are claimed, \
         and fire-24-usb-b doesn't have the datalog",
        board_pins,
    ),
    scenario(
        "datalog.pin_rule",
        "a GPIO pair is accepted exactly when the GPIO function table has one UART's TX and RX on it",
        pin_rule,
    ),
    scenario(
        "datalog.uart",
        "the UART is claimed and runs 8N1 at 38400 baud from clk_peri",
        uart,
    ),
    scenario(
        "datalog.ring",
        "the ring is the first 4096 bytes of a RAM slot One ROM doesn't serve, and its DMA channel is claimed",
        ring,
    ),
    scenario(
        "datalog.header",
        "the header requests serial ports 1 and 2 with their descriptions and sets channels 2 and 3 to their maximum",
        header,
    ),
    scenario(
        "datalog.to_pc",
        "ECU bytes reach the PC in order across many passes of the ring",
        to_pc,
    ),
    scenario(
        "datalog.pc_full",
        "with the PC not reading, channel 2 and the ring hold ECU bytes without loss",
        pc_full,
    ),
    scenario(
        "datalog.overrun",
        "a ring overrun loses the oldest bytes and counts them",
        overrun,
    ),
    scenario(
        "datalog.overwritten_in_copy",
        "bytes the DMA channel overwrites while they are copied are counted lost, not sent",
        overwritten_in_copy,
    ),
    scenario(
        "datalog.to_ecu",
        "the PC's bytes reach the ECU in order, waiting in channel 3 while the TX FIFO is full",
        to_ecu,
    ),
    scenario(
        "datalog.during_reply",
        "ECU bytes reach the PC while a reply waits for the host to read",
        during_reply,
    ),
    scenario(
        "datalog.during_save",
        "ECU bytes arriving during a sector write reach the PC",
        during_save,
    ),
    scenario(
        "datalog.staging",
        "ECU bytes in the ring survive a whole-chip write and a save, which both use the staging slot",
        staging,
    ),
    scenario_arranged(
        "datalog.uart_claimed",
        "with both UARTs already claimed Ostrich works without the datalog",
        without_datalog,
        claim_uarts,
    ),
    scenario_arranged(
        "datalog.dma_claimed",
        "with every DMA channel already claimed Ostrich works without the datalog",
        without_datalog,
        claim_dma_channels,
    ),
    scenario_arranged(
        "datalog.pins_claimed",
        "with every GPIO already claimed Ostrich works without the datalog",
        without_datalog,
        claim_gpios,
    ),
    scenario_arranged(
        "datalog.bad_pins",
        "with X pins that can't be one UART's TX and RX Ostrich works without the datalog",
        without_datalog,
        bad_x_pins,
    ),
    scenario_arranged(
        "datalog.x_pins_in_use",
        "with X pins One ROM uses Ostrich works without the datalog, even where sel_a and sel_b are free",
        without_datalog,
        x_pins_in_use,
    ),
    scenario_arranged(
        "datalog.two_ram_slots",
        "with two RAM slots Ostrich works without the datalog",
        without_datalog,
        two_ram_slots,
    ),
    scenario_arranged(
        "datalog.missing_api",
        "on firmware without the hardware claim call Ostrich works without the datalog",
        without_datalog,
        missing_api,
    ),
];

const BAUD: u32 = 38400;

/// One GPIO's UART function in the RP2350 datasheet's GPIO function table:
/// the function select, the UART and whether it is TX.  Every GPIO has one,
/// under F2 or F11.
#[rustfmt::skip]
const UART_FUNCTIONS: [(u8, u8, bool); 48] = [
    (2, 0, true), (2, 0, false), (11, 0, true), (11, 0, false), // 0-3
    (2, 1, true), (2, 1, false), (11, 1, true), (11, 1, false), // 4-7
    (2, 1, true), (2, 1, false), (11, 1, true), (11, 1, false), // 8-11
    (2, 0, true), (2, 0, false), (11, 0, true), (11, 0, false), // 12-15
    (2, 0, true), (2, 0, false), (11, 0, true), (11, 0, false), // 16-19
    (2, 1, true), (2, 1, false), (11, 1, true), (11, 1, false), // 20-23
    (2, 1, true), (2, 1, false), (11, 1, true), (11, 1, false), // 24-27
    (2, 0, true), (2, 0, false), (11, 0, true), (11, 0, false), // 28-31
    (2, 0, true), (2, 0, false), (11, 0, true), (11, 0, false), // 32-35
    (2, 1, true), (2, 1, false), (11, 1, true), (11, 1, false), // 36-39
    (2, 1, true), (2, 1, false), (11, 1, true), (11, 1, false), // 40-43
    (2, 0, true), (2, 0, false), (11, 0, true), (11, 0, false), // 44-47
];

/// The UART and pins for a board, from its X1, X2, sel_a and sel_b GPIOs in
/// `rust/config/json`.
fn board_uart(board: Board) -> Option<DatalogPins> {
    let pins = |uart, tx_gpio, tx_funcsel, rx_gpio, rx_funcsel| DatalogPins {
        uart,
        tx_gpio,
        tx_funcsel,
        rx_gpio,
        rx_funcsel,
    };
    Some(match board {
        // X2 is GPIO 8 and X1 GPIO 9
        Board::Fire24A
        | Board::Fire24C
        | Board::Fire24D
        | Board::Fire24E
        | Board::Fire24F
        | Board::Fire24G
        | Board::Fire28C
        | Board::Fire28D => pins(1, 8, 2, 9, 2),
        // X1 is GPIO 14 and X2 GPIO 15, both in the address span
        Board::Fire24UsbB => pins(0, 14, 11, 15, 11),
        // X2 is GPIO 14 and X1 GPIO 15
        Board::Fire24Eadb01 => pins(0, 14, 11, 15, 11),
        // sel_a is GPIO 26 and sel_b GPIO 27
        Board::Fire28A | Board::Fire28B => pins(1, 26, 11, 27, 11),
        // sel_a is GPIO 38 and sel_b GPIO 39
        Board::Fire32A => pins(1, 38, 11, 39, 11),
        // sel_a is GPIO 40 and sel_b GPIO 41
        Board::Fire32B | Board::Fire32C | Board::Fire40A | Board::Fire40B | Board::Fire40C => {
            pins(1, 40, 2, 41, 2)
        }
        _ => return None,
    })
}

fn ora_values() -> OraValues {
    let mut v = OraValues::default();
    // SAFETY: `v` outlives the call.
    unsafe { ffi::ostrich_host_test_ora_values(&mut v) };
    v
}

fn user_plugin_slot() -> PluginSlot {
    let mut slot = PluginSlot::default();
    // SAFETY: `slot` outlives the call.
    unsafe { ffi::ostrich_host_test_plugin_slot(1, &mut slot) };
    slot
}

/// The board's UART and pins, or why the datalog can't start on this device.
fn expected(d: &Device, ctx: &Ctx) -> Result<DatalogPins, String> {
    let pins =
        board_uart(ctx.board).ok_or_else(|| format!("no expected pins for {}", ctx.board))?;
    for gpio in [pins.tx_gpio, pins.rx_gpio] {
        // The datalog's own claim makes its pins ORA_GPIO_USE_PLUGIN.
        let used = d.gpio_use(gpio);
        if used != Some(fw::ora_gpio_use_t_ORA_GPIO_USE_FREE)
            && used != Some(fw::ora_gpio_use_t_ORA_GPIO_USE_PLUGIN)
        {
            return Err(format!("One ROM uses GPIO {gpio}: {used:?}"));
        }
    }
    let count = d.ram_slot_count();
    if count < 3 {
        return Err(format!("{count} RAM slots"));
    }
    Ok(pins)
}

/// The datalog's hardware.  Where the datalog can't start, checks it didn't
/// and returns a skip from the scenario.
macro_rules! datalog_or_skip {
    ($d:expr, $ctx:expr) => {
        match expected($d, $ctx) {
            Ok(_) => started($d)?,
            Err(why) => {
                not_started($d)?;
                return Ok(Outcome::Skip(format!("no datalog: {why}")));
            }
        }
    };
}

fn not_started(d: &Device) -> Result<(), String> {
    let hw = d.datalog_hw();
    check(
        hw.uart_starts == 0 && hw.dma_starts == 0 && hw.pins == 0,
        || format!("datalog hardware set up: {hw:?}"),
    )
}

/// The datalog's hardware, which must have started once.
fn started(d: &Device) -> Result<DatalogHw, String> {
    let hw = d.datalog_hw();
    check(
        hw.uart_starts == 1 && hw.dma_starts == 1 && hw.pins == 2,
        || format!("datalog not started: {hw:?}"),
    )?;
    check(hw.bad_order == 0 && hw.bad_target == 0, || {
        format!("datalog hardware faults: {hw:?}")
    })?;
    Ok(hw)
}

fn ring_size(hw: &DatalogHw) -> usize {
    1usize << hw.ring_bits
}

/// Send ECU bytes.  The DMA channel must copy all of them.
fn ecu(d: &Device, bytes: &[u8]) -> Result<(), String> {
    let n = d.ecu_send(bytes);
    check(n == bytes.len(), || {
        format!("the DMA channel took {n} of {} ECU bytes", bytes.len())
    })
}

/// Send ECU bytes in chunks, running the plugin after each.
fn ecu_paced(d: &mut Device, bytes: &[u8], chunk: usize) -> Result<(), String> {
    for part in bytes.chunks(chunk) {
        ecu(d, part)?;
        d.step()?;
    }
    Ok(())
}

/// Run the plugin until the PC has `len` ECU bytes, and return them.
fn to_pc_bytes(d: &mut Device, len: usize) -> Result<Vec<u8>, String> {
    let mut got = Vec::new();
    for _ in 0..1000 {
        got.extend(d.take_to_pc());
        if got.len() >= len {
            break;
        }
        d.step()?;
    }
    check(got.len() == len, || {
        format!("the PC received {} ECU bytes, want {len}", got.len())
    })?;
    Ok(got)
}

fn check_lost(d: &Device, want: u32) -> Result<(), String> {
    let lost = d.datalog_lost();
    check(lost == want, || {
        format!("{lost} bytes counted lost, want {want}")
    })
}

fn check_same(what: &str, got: &[u8], want: &[u8]) -> Result<(), String> {
    check(got.len() == want.len(), || {
        format!("{what}: {} bytes, want {}", got.len(), want.len())
    })?;
    if let Some(n) = got.iter().zip(want).position(|(a, b)| a != b) {
        return Err(format!(
            "{what}: differs at byte {n}: got {:02X?}, want {:02X?}",
            &got[n..(n + 8).min(got.len())],
            &want[n..(n + 8).min(want.len())]
        ));
    }
    Ok(())
}

fn board_pins(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let expect = expected(d, ctx);
    if ctx.board == Board::Fire24UsbB {
        check(expect.is_err(), || {
            "fire-24-usb-b's datalog pins are free".to_string()
        })?;
    }
    let want = match expect {
        Ok(want) => want,
        Err(_) => return without_datalog(d, ctx),
    };
    let hw = started(d)?;
    let tx = (0..2).find(|&i| hw.pin_rx[i] == 0);
    let rx = (0..2).find(|&i| hw.pin_rx[i] != 0);
    let (Some(tx), Some(rx)) = (tx, rx) else {
        return Err(format!("not one TX and one RX pin: {hw:?}"));
    };
    let got = DatalogPins {
        uart: hw.uart as u8,
        tx_gpio: hw.pin_gpio[tx] as u8,
        tx_funcsel: hw.pin_funcsel[tx] as u8,
        rx_gpio: hw.pin_gpio[rx] as u8,
        rx_funcsel: hw.pin_funcsel[rx] as u8,
    };
    check(got == want, || {
        format!("{}: pins {got:?}, want {want:?}", ctx.board)
    })?;
    for gpio in [want.tx_gpio, want.rx_gpio] {
        let used = d.gpio_use(gpio);
        check(used == Some(fw::ora_gpio_use_t_ORA_GPIO_USE_PLUGIN), || {
            format!("GPIO {gpio} isn't claimed: {used:?}")
        })?;
    }
    check(hw.dma_uart == hw.uart, || {
        format!(
            "DMA paced by UART{}, datalog on UART{}",
            hw.dma_uart, hw.uart
        )
    })?;
    Ok(Outcome::Pass)
}

fn pin_rule(_d: &mut Device, _ctx: &Ctx) -> Result<Outcome, String> {
    let gpios: Vec<u8> = (0u8..48).chain([48, 49, 0xFF]).collect();
    for &a in &gpios {
        for &b in &gpios {
            let want = match (
                UART_FUNCTIONS.get(usize::from(a)),
                UART_FUNCTIONS.get(usize::from(b)),
            ) {
                (Some(&(fa, ua, ta)), Some(&(fb, ub, tb))) if ua == ub && ta != tb => {
                    let ((tx, tf), (rx, rf)) = if ta {
                        ((a, fa), (b, fb))
                    } else {
                        ((b, fb), (a, fa))
                    };
                    Some(DatalogPins {
                        uart: ua,
                        tx_gpio: tx,
                        tx_funcsel: tf,
                        rx_gpio: rx,
                        rx_funcsel: rf,
                    })
                }
                _ => None,
            };
            let mut out = DatalogPins::default();
            // SAFETY: `out` outlives the call.
            let ok = unsafe { ffi::datalog_pins(a, b, &mut out) };
            let got = ok.then_some(out);
            check(got == want, || {
                format!("GPIOs {a} and {b}: {got:?}, want {want:?}")
            })?;
        }
    }
    Ok(Outcome::Pass)
}

fn uart(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let hw = datalog_or_skip!(d, ctx);
    let claimed = d.hw_claims();
    check(claimed & (1 << hw.uart) != 0, || {
        format!("UART{} not claimed: 0x{claimed:02X}", hw.uart)
    })?;

    // The divisor is in 64ths.  See the RP2350 datasheet's baud rate
    // calculation.
    let clk_hz = f64::from(ora_values().clk_peri_hz);
    let divisor = f64::from(hw.ibrd) + f64::from(hw.fbrd) / 64.0;
    check(hw.fbrd < 64 && hw.ibrd > 0, || {
        format!("divisors {} and {} out of range", hw.ibrd, hw.fbrd)
    })?;
    let baud = clk_hz / (16.0 * divisor);
    let error = (baud - f64::from(BAUD)).abs() / f64::from(BAUD);
    check(error < 0.001, || {
        format!("divisors {} and {} make {baud:.0} baud", hw.ibrd, hw.fbrd)
    })?;
    Ok(Outcome::Pass)
}

fn ring(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let hw = datalog_or_skip!(d, ctx);
    let size = ring_size(&hw) as u32;
    check(size == 4096, || format!("ring of {size} bytes"))?;
    let slot = (0..d.ram_slot_count())
        .find(|&n| d.ram_slot(n).is_some_and(|(addr, _)| addr == hw.ring_addr));
    let Some(slot) = slot else {
        return Err(format!(
            "ring at 0x{:08X} isn't the start of a RAM slot",
            hw.ring_addr
        ));
    };
    check(Some(slot) != d.active_ram_slot(), || {
        format!("ring in RAM slot {slot}, which One ROM serves")
    })?;
    check(hw.ring_addr.is_multiple_of(size), || {
        format!("ring at 0x{:08X} not aligned to its size", hw.ring_addr)
    })?;

    check(hw.dma_count.is_multiple_of(size), || {
        format!(
            "DMA count 0x{:X} isn't a multiple of the ring size",
            hw.dma_count
        )
    })?;
    let claims = d.dma_channel_claims();
    check(claims & (1 << hw.dma_ch) != 0, || {
        format!("DMA channel {} not claimed: 0x{claims:04X}", hw.dma_ch)
    })?;
    Ok(Outcome::Pass)
}

fn header(d: &mut Device, _ctx: &Ctx) -> Result<Outcome, String> {
    let v = ora_values();
    let slot = user_plugin_slot();
    check(
        u32::from(slot.channel_2_size) == v.log_channel_2_max_size
            && u32::from(slot.channel_3_size) == v.log_channel_3_max_size,
        || {
            format!(
                "channel sizes {} and {}, want {} and {}",
                slot.channel_2_size,
                slot.channel_3_size,
                v.log_channel_2_max_size,
                v.log_channel_3_max_size
            )
        },
    )?;
    for (channel, want) in [
        (CH_TO_PC, v.log_channel_2_max_size),
        (CH_FROM_PC, v.log_channel_3_max_size),
    ] {
        let size = d.channel_size(channel);
        check(size == want, || {
            format!("channel {channel} is {size} bytes, want {want}")
        })?;
    }

    // The header points at the peer data at the end of the 256-byte header.
    // The peer data is a version 1 USBc block followed by the 0 word.  The
    // block is "USBc", its 24-byte size, the version, the flags for serial
    // ports 1 and 2 and the descriptions' addresses.  Serial port 1's description follows
    // the peer data and is padded to 32 bytes.
    let block_addr = v.user_plugin_base + 256;
    check(slot.peer_data == block_addr, || {
        format!("peer data at {:08X}, want {block_addr:08X}", slot.peer_data)
    })?;
    let want = [
        u32::from_le_bytes(*b"USBc"),
        24,
        1,
        0b11,
        block_addr + 28,
        block_addr + 28 + 32,
        0,
    ];
    // SAFETY: the peer data is `want.len()` words in the plugin.
    let block =
        unsafe { std::slice::from_raw_parts(ffi::ostrich_host_test_peer_data(), want.len()) };
    check(block == want, || {
        format!("peer data {block:08X?}, want {want:08X?}")
    })?;

    // SAFETY: both descriptions are NUL-terminated arrays in the plugin.
    let descs = unsafe {
        [
            (
                CStr::from_ptr(ffi::ostrich_host_test_control_port_desc()),
                &b"One ROM Ostrich"[..],
            ),
            (
                CStr::from_ptr(ffi::ostrich_host_test_datalog_port_desc()),
                &b"One ROM Ostrich Datalog"[..],
            ),
        ]
    };
    for (desc, want) in descs {
        let desc = desc.to_bytes();
        check(desc == want, || {
            format!("port description {:?}", String::from_utf8_lossy(desc))
        })?;
        check(
            desc.len() <= 31 && desc.iter().all(|c| (0x20..=0x7E).contains(c)),
            || "port description the USB plugin would replace with its default".to_string(),
        )?;
    }
    Ok(Outcome::Pass)
}

fn to_pc(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let hw = datalog_or_skip!(d, ctx);
    let sent = proto::pattern(ring_size(&hw) * 5 + 77, 3);
    ecu_paced(d, &sent, 100)?;
    let got = to_pc_bytes(d, sent.len())?;
    check_same("ECU bytes at the PC", &got, &sent)?;
    check_lost(d, 0)?;
    Ok(Outcome::Pass)
}

/// Bytes channel 2 holds, with the PC not reading.
fn channel_2_room(d: &Device) -> usize {
    d.channel(CH_TO_PC).0 as usize
}

fn pc_full(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let hw = datalog_or_skip!(d, ctx);
    d.set_pc_reading(false);
    let held = channel_2_room(d) + ring_size(&hw);
    let sent = proto::pattern(held, 5);
    ecu_paced(d, &sent, 64)?;
    d.step_n(10)?;
    check_lost(d, 0)?;
    let (free, _) = d.channel(CH_TO_PC);
    check(free == 0, || format!("channel 2 has {free} bytes free"))?;

    d.set_pc_reading(true);
    let got = to_pc_bytes(d, sent.len())?;
    check_same("ECU bytes at the PC", &got, &sent)?;
    check_lost(d, 0)?;
    Ok(Outcome::Pass)
}

fn overrun(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let hw = datalog_or_skip!(d, ctx);
    d.set_pc_reading(false);
    let in_channel = channel_2_room(d);
    let extra = 300;
    let sent = proto::pattern(in_channel + ring_size(&hw) + extra, 7);
    ecu_paced(d, &sent, 50)?;
    d.step_n(10)?;
    check_lost(d, extra as u32)?;

    // The newest ring's worth of bytes follow those already in channel 2.
    d.set_pc_reading(true);
    let mut want = sent[..in_channel].to_vec();
    want.extend_from_slice(&sent[in_channel + extra..]);
    let got = to_pc_bytes(d, want.len())?;
    check_same("ECU bytes at the PC", &got, &want)?;

    // ECU bytes reach the PC again once it reads.
    let more = proto::pattern(200, 9);
    ecu_paced(d, &more, 40)?;
    let got = to_pc_bytes(d, more.len())?;
    check_same("ECU bytes after the overrun", &got, &more)?;
    check_lost(d, extra as u32)?;
    Ok(Outcome::Pass)
}

fn overwritten_in_copy(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let hw = datalog_or_skip!(d, ctx);
    let ring = ring_size(&hw);

    // Fill channel 2 and then the ring, so the ring's oldest byte is next to
    // be overwritten.
    d.set_pc_reading(false);
    let in_channel = channel_2_room(d);
    let first = proto::pattern(in_channel + ring, 11);
    ecu_paced(d, &first, 64)?;
    d.step_n(4)?;
    check_lost(d, 0)?;

    // Make room in channel 2 and send bytes as the plugin checks its copy of
    // the ring's oldest bytes.  Its first read of the DMA count is before the
    // copy and its second after.
    d.set_pc_reading(true);
    let mut got = d.take_to_pc();
    check(got.len() == in_channel, || {
        format!("channel 2 held {} bytes, want {in_channel}", got.len())
    })?;
    let late = proto::pattern(10, 13);
    d.ecu_send_at_sample(&late, 2);
    d.step()?;
    check_lost(d, late.len() as u32)?;

    let mut want = first.clone();
    want.drain(in_channel..in_channel + late.len());
    want.extend_from_slice(&late);
    let rest = to_pc_bytes(d, want.len() - got.len())?;
    got.extend(rest);
    check_same("ECU bytes at the PC", &got, &want)?;
    Ok(Outcome::Pass)
}

fn to_ecu(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    datalog_or_skip!(d, ctx);
    let sent = proto::pattern(100, 17);
    let n = d.pc_send(&sent)?;
    check(n == sent.len(), || {
        format!("channel 3 took {n} of {} bytes", sent.len())
    })?;

    // Nothing reads the bytes off the wire, so the FIFO fills.
    d.step_n(5)?;
    let hw = d.datalog_hw();
    let (_, waiting) = d.channel(CH_FROM_PC);
    check(
        hw.tx_level == 32 && waiting as usize == sent.len() - 32,
        || {
            format!(
                "{} bytes in the TX FIFO and {waiting} in channel 3",
                hw.tx_level
            )
        },
    )?;

    let mut got = Vec::new();
    for _ in 0..100 {
        got.extend(d.ecu_receive());
        if got.len() >= sent.len() {
            break;
        }
        d.step()?;
    }
    check_same("PC bytes at the ECU", &got, &sent)?;
    let hw = d.datalog_hw();
    check(hw.bad_tx_overflow == 0, || {
        "a byte was written to a full TX FIFO".to_string()
    })?;
    Ok(Outcome::Pass)
}

fn during_reply(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    datalog_or_skip!(d, ctx);

    // A 64KB read with the host not reading keeps the plugin in its reply.
    d.set_draining(false);
    d.send(&proto::bulk_read(0, 0, 0))?;
    d.step_n(10)?;
    let (free, _) = d.channel(crate::device::CH_OUT);
    check(free == 0, || format!("reply channel has {free} bytes free"))?;

    let sent = proto::pattern(150, 19);
    ecu(d, &sent)?;
    d.step()?;
    let got = d.take_to_pc();
    check_same("ECU bytes at the PC during the reply", &got, &sent)?;

    d.set_draining(true);
    let reply = d.reply(0x10000 + 1)?;
    let image = d.served(0, layout.chip_size as usize)?;
    let data: Vec<u8> = image.iter().copied().cycle().take(0x10000).collect();
    check_same("the reply", &reply, &proto::read_reply(&data))?;
    Ok(Outcome::Pass)
}

fn during_save(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let hw = datalog_or_skip!(d, ctx);
    edit(d, 0x0100, 1)?;
    let sent = proto::pattern(ring_size(&hw), 23);
    d.ecu_send_at_erase(&sent);
    save(d, &layout)?;
    check(d.flash_state().ops > 0, || {
        "the save didn't write flash".to_string()
    })?;
    let got = to_pc_bytes(d, sent.len())?;
    check_same("ECU bytes sent during the save", &got, &sent)?;
    check_lost(d, 0)?;
    Ok(Outcome::Pass)
}

fn staging(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    let layout = layout_or_skip!(ctx);
    let hw = datalog_or_skip!(d, ctx);

    // The PC doesn't read until the end, so the ring stays full throughout.
    d.set_pc_reading(false);
    let sent = proto::pattern(channel_2_room(d) + ring_size(&hw), 31);
    ecu_paced(d, &sent, 64)?;

    let image = proto::pattern(layout.chip_size as usize, 37);
    d.expect("ZW", &proto::bulk_write(0, 0, &image), &[OK])?;
    save(d, &layout)?;
    check(d.served(0, image.len())? == image, || {
        "the write wasn't served".to_string()
    })?;

    d.set_pc_reading(true);
    let got = to_pc_bytes(d, sent.len())?;
    check_same("ECU bytes at the PC", &got, &sent)?;
    check_lost(d, 0)?;
    Ok(Outcome::Pass)
}

/// The datalog didn't start and Ostrich still edits and reads the image.
fn without_datalog(d: &mut Device, ctx: &Ctx) -> Result<Outcome, String> {
    not_started(d)?;
    check(d.ecu_send(&[0x55; 16]) == 0, || {
        "the DMA channel took ECU bytes".to_string()
    })?;
    let sent = d.pc_send(&[0xAA; 16])?;
    d.step_n(5)?;
    let (_, waiting) = d.channel(CH_FROM_PC);
    check(waiting as usize == sent, || {
        format!(
            "{} of the PC's {sent} bytes were taken",
            sent - waiting as usize
        )
    })?;

    layout_or_skip!(ctx);
    d.expect("VV", b"VV", &VERSION)?;
    let data = proto::pattern(16, 29);
    d.expect("W", &proto::write(b'W', 0x0100, &data), &[OK])?;
    d.expect(
        "R",
        &proto::read(b'R', 0x0100, 16),
        &proto::read_reply(&data),
    )?;
    check(d.take_to_pc().is_empty(), || {
        "bytes reached the PC".to_string()
    })?;
    Ok(Outcome::Pass)
}

fn claim_uarts() {
    // SAFETY: called before the plugin starts.
    unsafe { fw::ffi_set_hw_claims(0b11) };
}

fn claim_dma_channels() {
    // SAFETY: called before the plugin starts.
    unsafe { fw::ffi_set_dma_channel_claims(0xFFFF) };
}

fn claim_gpios() {
    // SAFETY: called before the plugin starts.
    unsafe { fw::ffi_set_gpio_claims((1u64 << 48) - 1) };
}

fn bad_x_pins() {
    // Both are UART1 TX.
    // SAFETY: called before the plugin starts.
    unsafe { ffi::ostrich_host_test_override_x_pins(8, 10) };
}

fn x_pins_in_use() {
    // Data pins on every board, and UART0's TX and RX.
    // SAFETY: called before the plugin starts.
    unsafe { ffi::ostrich_host_test_override_x_pins(0, 1) };
}

fn two_ram_slots() {
    // SAFETY: called before the plugin starts.
    unsafe { ffi::ostrich_host_test_override_ram_slot_count(2) };
}

fn missing_api() {
    let ids = [fw::api_id_t_ORA_ID_CLAIM_HW];
    // SAFETY: `ids` outlives the call, which copies it.
    unsafe {
        onerom_plugin_tester::ffi::ora_host_test_withhold_api(ids.as_ptr(), ids.len() as u32)
    };
}
