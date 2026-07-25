mod base62;
mod obfuscate;

pub use base62::{decode, encode};
pub use obfuscate::{DOMAIN, deobfuscate, obfuscate};

#[cfg(test)]
mod tests {
    use super::{DOMAIN, decode, deobfuscate, encode, obfuscate};

    /// Pipeline completo de escrita: id do banco -> shortcode de 7 chars.
    fn shortcode(id: u64) -> Option<String> {
        obfuscate(id).map(|x| format!("{:0>7}", encode(x)))
    }

    /// Pipeline completo de leitura: shortcode -> id do banco.
    fn resolver(code: &str) -> Option<u64> {
        decode(code).and_then(deobfuscate)
    }

    #[test]
    fn shortcode_tem_exatamente_7_chars() {
        let amostra = [0, 1, 2, 127, 1_000_000, DOMAIN.saturating_sub(1)];
        for id in amostra {
            let code = shortcode(id).expect("id dentro do domínio");
            assert_eq!(code.len(), 7, "id {id} gerou {code:?}");
        }
    }

    #[test]
    fn shortcode_tem_exatamente_7_chars_em_massa() {
        for id in 0..50_000u64 {
            let code = shortcode(id).expect("id dentro do domínio");
            assert_eq!(code.len(), 7, "id {id} gerou {code:?}");
        }
    }

    #[test]
    fn roundtrip_completo() {
        let amostra = [0, 1, 42, 999_999, DOMAIN.saturating_sub(1)];
        for id in amostra {
            let code = shortcode(id).expect("id dentro do domínio");
            assert_eq!(resolver(&code), Some(id), "falhou para {id} via {code:?}");
        }
    }

    #[test]
    fn roundtrip_completo_em_massa() {
        for id in 0..50_000u64 {
            let code = shortcode(id).expect("id dentro do domínio");
            assert_eq!(resolver(&code), Some(id), "falhou para {id} via {code:?}");
        }
    }

    #[test]
    fn shortcodes_sao_unicos() {
        use std::collections::HashSet;

        let vistos: HashSet<String> = (0..100_000u64).filter_map(shortcode).collect();
        assert_eq!(vistos.len(), 100_000, "dois ids geraram o mesmo shortcode");
    }

    #[test]
    fn ids_consecutivos_nao_geram_codigos_parecidos() {
        for (a, b) in (0..2_000u64).zip(1..2_001u64) {
            let ca = shortcode(a).expect("id dentro do domínio");
            let cb = shortcode(b).expect("id dentro do domínio");
            let iguais = ca.chars().zip(cb.chars()).filter(|(x, y)| x == y).count();
            assert!(
                iguais <= 3,
                "ids {a} e {b} geraram {ca:?} e {cb:?} — {iguais} de 7 posições iguais"
            );
        }
    }

    #[test]
    fn resolver_rejeita_codigo_invalido() {
        for code in ["", "-------", "abc def", "!!!!!!!", "ZZZZZZZZZZZZ"] {
            assert_eq!(resolver(code), None, "deveria rejeitar {code:?}");
        }
    }
}
