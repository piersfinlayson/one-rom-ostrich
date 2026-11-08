//! Comparisons that produce a test's evidence on failure.

use std::time::Duration;

/// Fail with `what` unless `cond` holds.
pub fn check(cond: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(what()) }
}

/// Compare a reply with the bytes wanted. `trailing` is what arrived after
/// the reply should have ended.
pub fn reply(want: &[u8], got: &[u8], trailing: &[u8]) -> Result<(), String> {
    let mut evidence = Vec::new();
    if let Some(at) = first_difference(want, got) {
        evidence.push(format!(
            "{} of {} bytes arrived, first difference at byte {at}: got {}, want {}",
            got.len(),
            want.len(),
            window(got, at),
            window(want, at),
        ));
    }
    if !trailing.is_empty() {
        evidence.push(format!(
            "then {} more: {}",
            trailing.len(),
            window(trailing, 0)
        ));
    }
    if evidence.is_empty() {
        Ok(())
    } else {
        Err(evidence.join(", "))
    }
}

/// Compare an image read back with the one wanted, `base` being the first
/// byte's address.
pub fn image(what: &str, base: u32, want: &[u8], got: &[u8]) -> Result<(), String> {
    match first_difference(want, got) {
        None => Ok(()),
        Some(at) if at < want.len().min(got.len()) => Err(format!(
            "{what} differs at 0x{:04X}: got {}, want {}",
            base as usize + at,
            window(got, at),
            window(want, at)
        )),
        Some(_) => Err(format!(
            "{what} is {} bytes, want {}",
            got.len(),
            want.len()
        )),
    }
}

/// Fail unless `elapsed` is under `limit`.
pub fn within(what: &str, elapsed: Duration, limit: Duration) -> Result<(), String> {
    check(elapsed < limit, || {
        format!(
            "{what} took {}ms, limit {}ms",
            elapsed.as_millis(),
            limit.as_millis()
        )
    })
}

/// Fail unless `elapsed` is from `earliest` to `latest`.
pub fn between(
    what: &str,
    elapsed: Duration,
    earliest: Duration,
    latest: Duration,
) -> Result<(), String> {
    check(elapsed >= earliest && elapsed <= latest, || {
        format!(
            "{what} after {}ms, want {} to {}ms",
            elapsed.as_millis(),
            earliest.as_millis(),
            latest.as_millis()
        )
    })
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// Up to 8 bytes from `at`, in hex.
fn window(bytes: &[u8], at: usize) -> String {
    let end = (at + 8).min(bytes.len());
    let shown = bytes.get(at..end).unwrap_or_default();
    let more = if end < bytes.len() { " .." } else { "" };
    format!("{shown:02X?}{more}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_reply_passes() {
        assert_eq!(reply(b"\x14\x09O", b"\x14\x09O", &[]), Ok(()));
    }

    #[test]
    fn a_wrong_byte_is_located() {
        let err = reply(&[1, 2, 3, 4], &[1, 2, 9, 4], &[]).unwrap_err();
        assert!(err.contains("byte 2"), "{err}");
        assert!(err.contains("[09, 04]"), "{err}");
        assert!(err.contains("[03, 04]"), "{err}");
    }

    #[test]
    fn a_short_reply_fails_where_it_ends() {
        let err = reply(&[1, 2, 3], &[1, 2], &[]).unwrap_err();
        assert!(err.contains("byte 2"), "{err}");
    }

    #[test]
    fn bytes_after_the_reply_fail() {
        let err = reply(b"O", b"O", b"?").unwrap_err();
        assert!(err.contains("[3F]"), "{err}");
    }

    #[test]
    fn an_image_difference_includes_its_address() {
        let err = image("slot 1", 0x100, &[0, 1, 2], &[0, 7, 2]).unwrap_err();
        assert!(err.contains("0x0101"), "{err}");
        assert!(image("slot 1", 0, &[0, 1], &[0, 1, 2]).is_err());
        assert_eq!(image("slot 1", 0, &[5; 4], &[5; 4]), Ok(()));
    }

    #[test]
    fn times_are_checked_against_their_bounds() {
        let ms = Duration::from_millis;
        assert!(within("ZR", ms(399), ms(400)).is_ok());
        assert!(within("ZR", ms(400), ms(400)).is_err());
        assert!(between("save", ms(3000), ms(2980), ms(3200)).is_ok());
        assert!(between("save", ms(2979), ms(2980), ms(3200)).is_err());
        assert!(between("save", ms(3201), ms(2980), ms(3200)).is_err());
    }
}
