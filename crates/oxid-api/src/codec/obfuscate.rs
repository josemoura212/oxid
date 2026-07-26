/// EN: Domain size — every shortcode fits in 7 Base62 characters.
/// PT: Tamanho do domínio — todo shortcode cabe em 7 caracteres Base62.
pub const DOMAIN: u64 = 62u64.pow(7);

/// EN: Bijection multiplier. Prime, hence coprime to `DOMAIN = 2^7 · 31^7`.
/// EN: Picked near `DOMAIN / φ` (Fibonacci hashing) so neighbouring ids land as
/// EN: far apart as possible.
/// PT: Multiplicador da bijeção. Primo, logo coprimo de `DOMAIN = 2^7 · 31^7`.
/// PT: Escolhido próximo de `DOMAIN / φ` (Fibonacci hashing) para que ids
/// PT: vizinhos caiam o mais longe possível um do outro.
const K: u64 = 2_176_477_521_929;

/// EN: Multiplicative inverse of [`K`] mod [`DOMAIN`]: `K · K_INV ≡ 1 (mod DOMAIN)`.
/// PT: Inverso multiplicativo de [`K`] módulo [`DOMAIN`]: `K · K_INV ≡ 1 (mod DOMAIN)`.
const K_INV: u64 = 294_289_236_153;

/// EN: `(x · factor) mod DOMAIN`, product in `u128` — `u64` would overflow:
/// EN: both factors reach ~2^42, so the product reaches ~2^84.
/// PT: `(x · factor) mod DOMAIN`, com o produto em `u128` — em `u64` estouraria:
/// PT: ambos os fatores chegam a ~2^42, então o produto chega a ~2^84.
fn mul_mod(x: u64, factor: u64) -> Option<u64> {
    if x >= DOMAIN {
        return None;
    }

    let product = u128::from(x).checked_mul(u128::from(factor))?;
    let remainder = product.checked_rem(u128::from(DOMAIN))?;
    u64::try_from(remainder).ok()
}

pub fn obfuscate(id: u64) -> Option<u64> {
    mul_mod(id, K)
}

pub fn deobfuscate(obfuscated_id: u64) -> Option<u64> {
    mul_mod(obfuscated_id, K_INV)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{DOMAIN, K, K_INV, deobfuscate, obfuscate};

    #[test]
    fn domain_is_62_to_the_7th() {
        assert_eq!(DOMAIN, 3_521_614_606_208);
    }

    #[test]
    fn k_inv_is_the_inverse_of_k() {
        // EN: proves in one shot that K is coprime to DOMAIN and the constant is
        // EN: right. Without it the modular multiplication stops being a bijection.
        // PT: prova de uma vez que K é coprimo de DOMAIN e que a constante está
        // PT: certa. Sem isso, a multiplicação modular deixa de ser bijeção.
        let product = u128::from(K).wrapping_mul(u128::from(K_INV));
        assert_eq!(product % u128::from(DOMAIN), 1);
    }

    #[test]
    fn k_is_inside_the_domain() {
        const {
            assert!(K < DOMAIN);
            assert!(K_INV < DOMAIN);
        }
    }

    #[test]
    fn rejects_input_outside_the_domain() {
        assert_eq!(obfuscate(DOMAIN), None);
        assert_eq!(deobfuscate(DOMAIN), None);
        assert_eq!(obfuscate(u64::MAX), None);
        assert_eq!(deobfuscate(u64::MAX), None);
    }

    #[test]
    fn accepts_the_domain_upper_bound() {
        let last = DOMAIN.saturating_sub(1);
        assert!(obfuscate(last).is_some());
        assert!(deobfuscate(last).is_some());
    }

    #[test]
    fn image_always_inside_the_domain() {
        // EN: this property is what guarantees shortcodes of at most 7 chars.
        // PT: é essa propriedade que garante shortcode de no máximo 7 chars.
        let sample = [
            0,
            1,
            2,
            61,
            62,
            127,
            128,
            1_000_000,
            DOMAIN.saturating_sub(1),
        ];
        for id in sample {
            let output = obfuscate(id).expect("id inside the domain");
            assert!(
                output < DOMAIN,
                "obfuscate({id}) = {output} left the domain"
            );
        }
    }

    #[test]
    fn image_inside_the_domain_in_bulk() {
        for id in 0..100_000u64 {
            let output = obfuscate(id).expect("id inside the domain");
            assert!(
                output < DOMAIN,
                "obfuscate({id}) = {output} left the domain"
            );
        }
    }

    #[test]
    fn roundtrip_edge_cases() {
        let sample = [
            0,
            1,
            2,
            61,
            62,
            12_345,
            999_999_999,
            DOMAIN.saturating_sub(1),
        ];
        for id in sample {
            let output = obfuscate(id).expect("id inside the domain");
            assert_eq!(deobfuscate(output), Some(id), "roundtrip failed for {id}");
        }
    }

    #[test]
    fn roundtrip_in_bulk() {
        for id in 0..100_000u64 {
            let output = obfuscate(id).expect("id inside the domain");
            assert_eq!(deobfuscate(output), Some(id), "roundtrip failed for {id}");
        }
    }

    #[test]
    fn is_a_bijection_without_collisions() {
        let seen: HashSet<u64> = (0..200_000u64).filter_map(obfuscate).collect();
        assert_eq!(seen.len(), 200_000, "collision found");
    }

    #[test]
    fn consecutive_ids_land_far_apart() {
        // EN: with modular multiplication the delta is always K (mod DOMAIN).
        // EN: what we reject here is a small, predictable increment.
        // PT: com multiplicação modular a diferença é sempre K (mod DOMAIN).
        // PT: o que rejeitamos aqui é o incremento pequeno e previsível.
        for (a, b) in (0..2_000u64).zip(1..2_001u64) {
            let x = obfuscate(a).expect("id inside the domain");
            let y = obfuscate(b).expect("id inside the domain");
            let delta = x.abs_diff(y);
            assert!(
                delta > 1_000_000_000,
                "obfuscate({a}) and obfuscate({b}) landed {delta} apart"
            );
        }
    }
}
