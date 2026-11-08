//! One ROM's USB serial ports, found from its USB serial number.

use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use nusb::MaybeFuture;
use serialport::{ClearBuffer, SerialPort, SerialPortType};

use crate::consts::{
    DATALOG_PORT_DESC, REPLY_DEADLINE, TRAILING_WINDOW, USB_PLUGIN_PID, USB_PLUGIN_VID,
};

/// A serial port the system lists, with its USB identity where it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub usb: Option<(u16, u16, Option<String>)>,
    /// The USB interface number the system lists for the port
    pub interface: Option<u8>,
}

/// One of a device's USB serial ports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Which {
    /// Serial port 1, Ostrich's. Lab's console is Lab's only port.
    Port1,
    /// Serial port 2, the datalog
    Port2,
}

/// The `which` port of the device with One ROM's VID and PID and `serial`.
/// `port2` is the interface number of serial port 2's first interface, where
/// the device has serial port 2.
///
/// A USB serial port is two interfaces. macOS lists the second interface's
/// number and Linux and Windows the first's. Serial port 2 is the port listed
/// with either of its interface numbers. Serial port 1 is any other.
///
/// macOS lists each port twice, as `/dev/tty.*` and `/dev/cu.*`. The `cu`
/// device opens without waiting for carrier detect, so the `tty` one is
/// dropped.
pub fn select(
    candidates: &[Candidate],
    serial: &str,
    port2: Option<u8>,
    which: Which,
) -> Result<String, String> {
    let ids = (USB_PLUGIN_VID, USB_PLUGIN_PID);
    let is_port2 = |c: &Candidate| match (port2, c.interface) {
        (Some(first), Some(n)) => n == first || n == first.wrapping_add(1),
        _ => false,
    };
    let found: Vec<&str> = candidates
        .iter()
        .filter(|c| !c.name.starts_with("/dev/tty."))
        .filter(|c| match &c.usb {
            Some((vid, pid, Some(s))) => (*vid, *pid) == ids && s == serial,
            _ => false,
        })
        .filter(|c| is_port2(c) == (which == Which::Port2))
        .map(|c| c.name.as_str())
        .collect();
    let port = match which {
        Which::Port1 => "serial port 1",
        Which::Port2 => "serial port 2",
    };
    match found[..] {
        [one] => Ok(one.to_string()),
        [] => Err(format!("USB serial number {serial} doesn't have {port}")),
        _ => Err(format!(
            "several serial ports are {port} of USB serial number {serial}: {}",
            found.join(", ")
        )),
    }
}

/// The ports the system lists.
pub fn candidates() -> Result<Vec<Candidate>, String> {
    let ports = serialport::available_ports().map_err(|e| format!("listing serial ports: {e}"))?;
    Ok(ports
        .into_iter()
        .map(|p| match p.port_type {
            SerialPortType::UsbPort(u) => Candidate {
                name: p.port_name,
                usb: Some((u.vid, u.pid, u.serial_number)),
                interface: u.interface,
            },
            _ => Candidate {
                name: p.port_name,
                usb: None,
                interface: None,
            },
        })
        .collect())
}

/// The number of the USB interface whose description is `DATALOG_PORT_DESC` on
/// the device with `serial`, where it has one.
pub fn port2_interface(serial: &str) -> Result<Option<u8>, String> {
    let device = nusb::list_devices()
        .wait()
        .map_err(|e| format!("listing USB devices: {e}"))?
        .find(|d| {
            d.vendor_id() == USB_PLUGIN_VID
                && d.product_id() == USB_PLUGIN_PID
                && d.serial_number() == Some(serial)
        });
    Ok(device.and_then(|d| {
        d.interfaces()
            .find(|i| i.interface_string() == Some(DATALOG_PORT_DESC))
            .map(|i| i.interface_number())
    }))
}

/// The `which` serial port of the device with `serial`.
pub fn find(serial: &str, which: Which) -> Result<String, String> {
    select(&candidates()?, serial, port2_interface(serial)?, which)
}

/// A reply and when its bytes arrived.
pub struct Reply {
    pub bytes: Vec<u8>,
    /// Bytes that arrived after the reply's expected length
    pub trailing: Vec<u8>,
    pub sent: Instant,
    pub last: Instant,
}

impl Reply {
    /// From sending the request to the reply's last byte.
    pub fn elapsed(&self) -> Duration {
        self.last - self.sent
    }
}

pub struct Port {
    pub name: String,
    port: Box<dyn SerialPort>,
}

impl Port {
    /// Open the port with DTR deasserted, as .NET's `SerialPort` does for ORSS.
    pub fn open(name: &str) -> Result<Self, String> {
        Self::open_with(name, 115_200, false)
    }

    pub fn open_with(name: &str, baud: u32, dtr: bool) -> Result<Self, String> {
        let port = serialport::new(name, baud)
            .dtr_on_open(dtr)
            .timeout(TRAILING_WINDOW)
            .open()
            .map_err(|e| format!("opening {name}: {e}"))?;
        port.clear(ClearBuffer::All)
            .map_err(|e| format!("clearing {name}: {e}"))?;
        Ok(Self {
            name: name.to_string(),
            port,
        })
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<Instant, String> {
        let at = Instant::now();
        self.port
            .write_all(bytes)
            .and_then(|()| self.port.flush())
            .map_err(|e| format!("writing to {}: {e}", self.name))?;
        Ok(at)
    }

    /// Read up to `len` bytes, stopping at `deadline`.
    pub fn read_until(&mut self, len: usize, deadline: Instant) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0u8; len];
        let mut got = 0;
        while got < len {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            self.port
                .set_timeout(deadline - now)
                .map_err(|e| format!("setting the timeout on {}: {e}", self.name))?;
            match self.port.read(&mut bytes[got..]) {
                Ok(n) => got += n,
                Err(e) if e.kind() == ErrorKind::TimedOut => break,
                Err(e) => return Err(format!("reading {}: {e}", self.name)),
            }
        }
        bytes.truncate(got);
        Ok(bytes)
    }

    /// Read whatever arrives until `quiet` passes without a byte.
    pub fn drain(&mut self, quiet: Duration) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        loop {
            let more = self.read_until(4096, Instant::now() + quiet)?;
            if more.is_empty() {
                return Ok(bytes);
            }
            bytes.extend(more);
        }
    }

    /// Send `request` and read a reply of `len` bytes, then any bytes that
    /// follow it.
    pub fn exchange(&mut self, request: &[u8], len: usize) -> Result<Reply, String> {
        let sent = self.send(request)?;
        let bytes = self.read_until(len, sent + REPLY_DEADLINE)?;
        let last = Instant::now();
        let trailing = self.drain(TRAILING_WINDOW)?;
        Ok(Reply {
            bytes,
            trailing,
            sent,
            last,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VID: u16 = USB_PLUGIN_VID;
    const PID: u16 = USB_PLUGIN_PID;

    fn usb(name: &str, vid: u16, serial: &str) -> Candidate {
        Candidate {
            name: name.to_string(),
            usb: Some((vid, PID, Some(serial.to_string()))),
            interface: None,
        }
    }

    fn on(interface: u8, name: &str) -> Candidate {
        Candidate {
            interface: Some(interface),
            ..usb(name, VID, "AAAA")
        }
    }

    fn port1(ports: &[Candidate], serial: &str) -> Result<String, String> {
        select(ports, serial, None, Which::Port1)
    }

    #[test]
    fn the_cu_port_with_the_serial_number_is_chosen() {
        let ports = [
            usb("/dev/tty.usbmodem1101", VID, "AAAA"),
            usb("/dev/cu.usbmodem1101", VID, "AAAA"),
            usb("/dev/cu.usbmodem2101", VID, "BBBB"),
            Candidate {
                name: "/dev/cu.Bluetooth".to_string(),
                usb: None,
                interface: None,
            },
        ];
        assert_eq!(port1(&ports, "AAAA").unwrap(), "/dev/cu.usbmodem1101");
        assert_eq!(port1(&ports, "BBBB").unwrap(), "/dev/cu.usbmodem2101");
    }

    #[test]
    fn a_linux_port_is_chosen() {
        let ports = [usb("/dev/ttyACM0", VID, "AAAA")];
        assert_eq!(port1(&ports, "AAAA").unwrap(), "/dev/ttyACM0");
    }

    #[test]
    fn another_vendors_port_is_never_chosen() {
        let ports = [usb("/dev/cu.usbmodem1101", VID ^ 1, "AAAA")];
        assert!(port1(&ports, "AAAA").is_err());
    }

    #[test]
    fn a_missing_or_repeated_serial_number_fails() {
        let ports = [
            usb("/dev/cu.usbmodem1101", VID, "AAAA"),
            usb("/dev/cu.usbmodem1102", VID, "AAAA"),
        ];
        assert!(port1(&ports, "AAAA").is_err());
        assert!(port1(&ports, "CCCC").is_err());
    }

    // One ROM's serial ports are interfaces 2 and 3, and 4 and 5.
    #[test]
    fn serial_port_2_is_chosen_by_either_of_its_interfaces() {
        for (one, two) in [(3, 5), (2, 4)] {
            let ports = [
                on(one, "/dev/cu.usbmodem11303"),
                on(two, "/dev/cu.usbmodem11305"),
            ];
            let pick = |which| select(&ports, "AAAA", Some(4), which).unwrap();
            assert_eq!(pick(Which::Port1), "/dev/cu.usbmodem11303");
            assert_eq!(pick(Which::Port2), "/dev/cu.usbmodem11305");
        }
    }

    #[test]
    fn without_serial_port_2_its_only_port_is_serial_port_1() {
        let ports = [on(3, "/dev/cu.usbmodem11303")];
        assert_eq!(port1(&ports, "AAAA").unwrap(), "/dev/cu.usbmodem11303");
        assert!(select(&ports, "AAAA", None, Which::Port2).is_err());
    }
}
