const BASE: u64 = 62;

/// Most digits `u64::MAX` takes in this base.
const MAX_LEN: usize = 11;

/// Alphabet order: `0-9`, `a-z`, `A-Z`. The `match` in [`decode`] mirrors it —
/// changing one without the other invalidates every shortcode ever issued.
const ALPHABET: &[u8; 62] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

pub fn encode(n: u64) -> String {
    let mut n = n;
    let mut buf = Vec::with_capacity(MAX_LEN);

    // do-while — zero also has a digit, so the body must run at least once.
    // The `while let` condition never fails (`% BASE` always fits the
    // alphabet); what ends the loop is the `n == 0` at the bottom.
    while let Some(&byte) = usize::try_from(n % BASE).ok().and_then(|i| ALPHABET.get(i)) {
        buf.push(byte);

        n /= BASE;
        if n == 0 {
            break;
        }
    }

    // successive division yields least significant digit first.
    buf.reverse();
    buf.iter().map(|&b| char::from(b)).collect()
}

pub fn decode(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }

    let mut n: u64 = 0;

    // bytes, not chars — anything outside ASCII falls into the reject arm.
    for byte in s.bytes() {
        let digit = match byte {
            b'0'..=b'9' => byte.checked_sub(b'0')?,
            b'a'..=b'z' => byte.checked_sub(b'a')?.checked_add(10)?,
            b'A'..=b'Z' => byte.checked_sub(b'A')?.checked_add(36)?,
            _ => return None,
        };
        n = n.checked_mul(BASE)?.checked_add(u64::from(digit))?;
    }

    Some(n)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn encode_known_values() {
        assert_eq!(encode(0), "0");
        assert_eq!(encode(1), "1");
        assert_eq!(encode(9), "9");
        assert_eq!(encode(10), "a");
        assert_eq!(encode(35), "z");
        assert_eq!(encode(36), "A");
        assert_eq!(encode(61), "Z");
        assert_eq!(encode(62), "10");
        assert_eq!(encode(125), "21");
        assert_eq!(encode(3843), "ZZ");
    }

    #[test]
    fn decode_known_values() {
        assert_eq!(decode("0"), Some(0));
        assert_eq!(decode("9"), Some(9));
        assert_eq!(decode("a"), Some(10));
        assert_eq!(decode("z"), Some(35));
        assert_eq!(decode("A"), Some(36));
        assert_eq!(decode("Z"), Some(61));
        assert_eq!(decode("10"), Some(62));
        assert_eq!(decode("21"), Some(125));
    }

    #[test]
    fn decode_is_case_sensitive() {
        assert_ne!(decode("a"), decode("A"));
    }

    #[test]
    fn decode_ignores_leading_zeros() {
        // shortcodes are padded to 7 chars; padding must not change the value.
        assert_eq!(decode("0000001"), Some(1));
        assert_eq!(decode("0000000"), Some(0));
    }

    #[test]
    fn roundtrip_edge_cases() {
        for n in [0, 1, 9, 10, 35, 36, 61, 62, 63, 3843, u64::MAX] {
            assert_eq!(decode(&encode(n)), Some(n), "roundtrip failed for {n}");
        }
    }

    #[test]
    fn roundtrip_spread_sample() {
        // powers of a prime span several magnitudes until u64 overflows.
        let mut n: u64 = 1;
        while let Some(next) = n.checked_mul(7919) {
            assert_eq!(decode(&encode(n)), Some(n), "roundtrip failed for {n}");
            n = next;
        }
    }

    #[test]
    fn decode_rejects_invalid_chars() {
        for s in [
            "-", "_", " ", "!", "+", "/", "=", "a-b", "1 2", "ção", "日本",
        ] {
            assert_eq!(decode(s), None, "should have rejected {s:?}");
        }
    }

    #[test]
    fn decode_rejects_empty_string() {
        // "" is not a valid shortcode — the accumulator's identity leaking out.
        assert_eq!(decode(""), None);
    }

    #[test]
    fn decode_rejects_overflow() {
        // u64::MAX takes 11 base62 digits; 12 at the top of the alphabet overflows.
        assert_eq!(decode("ZZZZZZZZZZZZ"), None);
        assert_eq!(decode("ZZZZZZZZZZZZZZZZZZZZZZZZ"), None);
    }

    #[test]
    fn encode_only_emits_the_alphabet() {
        let mut n: u64 = 1;
        while let Some(next) = n.checked_mul(1_000_003) {
            let s = encode(n);
            assert!(
                s.chars().all(|c| c.is_ascii_alphanumeric()),
                "encode({n}) = {s:?} has a char outside the alphabet"
            );
            n = next;
        }
    }

    #[test]
    fn encode_never_returns_empty() {
        for n in [0, 1, u64::MAX] {
            assert!(!encode(n).is_empty(), "encode({n}) returned empty");
        }
    }
}
