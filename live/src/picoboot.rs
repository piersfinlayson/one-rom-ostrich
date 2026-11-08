//! How long another program holds One ROM's picoboot interface.
//!
//! The harness claims the interface repeatedly and records whether each claim
//! succeeded. A run of failed claims is the other program's hold. The CLI opens
//! the device and claims the interface the same way, so a claim fails when the
//! CLI's would.

use std::time::{Duration, Instant};

use nusb::DeviceInfo;
use nusb::MaybeFuture;
use nusb::descriptors::TransferType;
use nusb::transfer::Direction;

use crate::consts::{USB_PLUGIN_PID, USB_PLUGIN_VID};

/// The picoboot interface's class and subclass, as the picoboot crate matches it.
const PICOBOOT_CLASS: u8 = 0xFF;
const PICOBOOT_SUBCLASS: u8 = 0x00;

pub struct Picoboot {
    info: DeviceInfo,
    interface: u8,
}

impl Picoboot {
    /// The picoboot interface of the One ROM with USB serial number `serial`.
    pub fn find(serial: &str) -> Result<Self, String> {
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| format!("listing USB devices: {e}"))?
            .find(|d| {
                d.vendor_id() == USB_PLUGIN_VID
                    && d.product_id() == USB_PLUGIN_PID
                    && d.serial_number() == Some(serial)
            })
            .ok_or_else(|| format!("no One ROM on USB has serial number {serial}"))?;
        let device = info
            .open()
            .wait()
            .map_err(|e| format!("opening {serial}: {e}"))?;
        let config = device
            .active_configuration()
            .map_err(|e| format!("reading {serial}'s configuration: {e}"))?;
        let interface = config
            .interface_alt_settings()
            .find(|i| {
                let bulk = |dir| {
                    i.endpoints()
                        .any(|e| e.transfer_type() == TransferType::Bulk && e.direction() == dir)
                };
                i.class() == PICOBOOT_CLASS
                    && i.subclass() == PICOBOOT_SUBCLASS
                    && bulk(Direction::In)
                    && bulk(Direction::Out)
            })
            .ok_or_else(|| format!("{serial} doesn't have a picoboot interface"))?
            .interface_number();
        Ok(Self { info, interface })
    }

    /// Open the device and claim the interface, then release both. Returns
    /// whether the claim succeeded.
    pub fn claim(&self) -> bool {
        self.info
            .open()
            .wait()
            .and_then(|device| device.claim_interface(self.interface).wait())
            .is_ok()
    }
}

/// One claim: when it started and ended, and whether it succeeded.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub start: Instant,
    pub end: Instant,
    pub free: bool,
}

/// The longest each run of failed claims could have lasted, from the end of
/// the successful claim before it to the start of the one after. A run at
/// either end of `samples` is incomplete and left out.
pub fn holds(samples: &[Sample]) -> Vec<Duration> {
    let mut holds = Vec::new();
    let mut last_free: Option<Instant> = None;
    let mut busy = false;
    for s in samples {
        if s.free {
            if let (true, Some(free_end)) = (busy, last_free) {
                holds.push(s.start - free_end);
            }
            last_free = Some(s.end);
            busy = false;
        } else {
            busy = true;
        }
    }
    holds
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples at `pattern`'s millisecond offsets, each lasting 1ms.
    fn samples(base: Instant, pattern: &[(u64, bool)]) -> Vec<Sample> {
        pattern
            .iter()
            .map(|&(at, free)| Sample {
                start: base + Duration::from_millis(at),
                end: base + Duration::from_millis(at + 1),
                free,
            })
            .collect()
    }

    #[test]
    fn a_hold_runs_between_the_successful_claims_around_it() {
        let base = Instant::now();
        let s = samples(
            base,
            &[
                (0, true),
                (2, false),
                (4, false),
                (6, true),
                (8, true),
                (10, false),
                (12, true),
            ],
        );
        let ms = Duration::from_millis;
        assert_eq!(holds(&s), vec![ms(5), ms(3)]);
    }

    #[test]
    fn a_hold_at_either_end_is_left_out() {
        let base = Instant::now();
        let s = samples(base, &[(0, false), (2, true), (4, true), (6, false)]);
        assert!(holds(&s).is_empty());
    }
}
