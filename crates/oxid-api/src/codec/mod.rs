mod base62;
mod obfuscate;

pub use base62::{decode, encode};
pub use obfuscate::{DOMAIN, deobfuscate, obfuscate};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{DOMAIN, decode, deobfuscate, encode, obfuscate};

    /// EN: Full write pipeline: database id -> 7-char shortcode.
    /// PT: Pipeline completo de escrita: id do banco -> shortcode de 7 chars.
    fn shortcode(id: u64) -> Option<String> {
        obfuscate(id).map(|x| format!("{:0>7}", encode(x)))
    }

    /// EN: Full read pipeline: shortcode -> database id.
    /// PT: Pipeline completo de leitura: shortcode -> id do banco.
    fn resolve(code: &str) -> Option<u64> {
        decode(code).and_then(deobfuscate)
    }

    #[test]
    fn shortcode_is_exactly_7_chars() {
        let sample = [0, 1, 2, 127, 1_000_000, DOMAIN.saturating_sub(1)];
        for id in sample {
            let code = shortcode(id).expect("id inside the domain");
            assert_eq!(code.len(), 7, "id {id} produced {code:?}");
        }
    }

    #[test]
    fn shortcode_is_exactly_7_chars_in_bulk() {
        for id in 0..50_000u64 {
            let code = shortcode(id).expect("id inside the domain");
            assert_eq!(code.len(), 7, "id {id} produced {code:?}");
        }
    }

    #[test]
    fn full_roundtrip() {
        let sample = [0, 1, 42, 999_999, DOMAIN.saturating_sub(1)];
        for id in sample {
            let code = shortcode(id).expect("id inside the domain");
            assert_eq!(resolve(&code), Some(id), "failed for {id} via {code:?}");
        }
    }

    #[test]
    fn full_roundtrip_in_bulk() {
        for id in 0..50_000u64 {
            let code = shortcode(id).expect("id inside the domain");
            assert_eq!(resolve(&code), Some(id), "failed for {id} via {code:?}");
        }
    }

    #[test]
    fn shortcodes_are_unique() {
        let seen: HashSet<String> = (0..100_000u64).filter_map(shortcode).collect();
        assert_eq!(seen.len(), 100_000, "two ids produced the same shortcode");
    }

    #[test]
    fn consecutive_ids_do_not_produce_similar_codes() {
        for (a, b) in (0..2_000u64).zip(1..2_001u64) {
            let code_a = shortcode(a).expect("id inside the domain");
            let code_b = shortcode(b).expect("id inside the domain");
            let matching = code_a
                .chars()
                .zip(code_b.chars())
                .filter(|(x, y)| x == y)
                .count();

            assert!(
                matching <= 3,
                "ids {a} and {b} produced {code_a:?} and {code_b:?} — \
                 {matching} of 7 positions match"
            );
        }
    }

    #[test]
    fn resolve_rejects_invalid_codes() {
        for code in ["", "-------", "abc def", "!!!!!!!", "ZZZZZZZZZZZZ"] {
            assert_eq!(resolve(code), None, "should have rejected {code:?}");
        }
    }
}
