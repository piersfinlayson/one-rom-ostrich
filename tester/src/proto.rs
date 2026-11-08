//! Ostrich 2.0 requests and replies, written from Moates' command table
//! independently of the plugin.

pub const OK: u8 = b'O';
pub const FAIL: u8 = b'?';

/// The plugin's reply to VV.
pub const VERSION: [u8; 3] = [0x14, 0x09, b'O'];

/// The first byte of the plugin's reply to NS.
pub const VENDOR_ID: u8 = 0x00;

/// 8-bit sum of the bytes, overflow discarded.
pub fn sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b))
}

/// The bytes followed by their checksum.
pub fn checked(bytes: &[u8]) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v.push(sum(bytes));
    v
}

/// The bytes followed by a checksum that doesn't match.
pub fn bad_checksum(bytes: &[u8]) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v.push(sum(bytes).wrapping_add(1));
    v
}

/// A count byte, where 0 means 256.
fn count(len: usize) -> u8 {
    assert!((1..=256).contains(&len), "a count covers 1 to 256");
    (len % 256) as u8
}

/// R or r for `len` bytes from `addr`.
pub fn read(op: u8, addr: u16, len: usize) -> Vec<u8> {
    let [msb, lsb] = addr.to_be_bytes();
    checked(&[op, count(len), msb, lsb])
}

/// W, w or X, without the checksum.
pub fn write_unchecked(op: u8, addr: u16, data: &[u8]) -> Vec<u8> {
    let [msb, lsb] = addr.to_be_bytes();
    let mut v = vec![op, count(data.len()), msb, lsb];
    v.extend_from_slice(data);
    v
}

pub fn write(op: u8, addr: u16, data: &[u8]) -> Vec<u8> {
    checked(&write_unchecked(op, addr, data))
}

/// ZR for blocks of 256 bytes from msb:00.  0 blocks means 256.
pub fn bulk_read(blocks: u8, mmsb: u8, msb: u8) -> Vec<u8> {
    checked(&[b'Z', b'R', blocks, mmsb, msb])
}

/// ZW without the checksum.  The data is whole blocks of 256 bytes.
pub fn bulk_write_unchecked(mmsb: u8, msb: u8, data: &[u8]) -> Vec<u8> {
    assert!(!data.is_empty() && data.len().is_multiple_of(256) && data.len() <= 0x10000);
    let blocks = ((data.len() / 256) % 256) as u8;
    let mut v = vec![b'Z', b'W', blocks, mmsb, msb];
    v.extend_from_slice(data);
    v
}

pub fn bulk_write(mmsb: u8, msb: u8, data: &[u8]) -> Vec<u8> {
    checked(&bulk_write_unchecked(mmsb, msb, data))
}

/// The reply to a read: the data then its checksum.
pub fn read_reply(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    v.push(sum(data));
    v
}

/// Bytes that differ from each other and from a typical image.
pub fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32).wrapping_mul(0x9D).wrapping_add(u32::from(seed)) as u8 ^ 0xA5)
        .collect()
}
