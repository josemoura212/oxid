/// Tamanho do domínio: todo shortcode cabe em 7 caracteres Base62.
pub const DOMAIN: u64 = 62u64.pow(7);

/// Multiplicador da bijeção. Primo, logo coprimo de `DOMAIN = 2^7 · 31^7`.
/// Escolhido próximo de `DOMAIN / φ` (Fibonacci hashing) para que ids vizinhos
/// caiam o mais longe possível um do outro.
const K: u64 = 2_176_477_521_929;

/// Inverso multiplicativo de [`K`] módulo [`DOMAIN`]: `K · K_INV ≡ 1 (mod DOMAIN)`.
const K_INV: u64 = 294_289_236_153;

/// `(x · fator) mod DOMAIN`, com o produto em `u128` — em `u64` ele estouraria:
/// ambos os fatores chegam a ~2^42, então o produto chega a ~2^84.
fn mul_mod(x: u64, fator: u64) -> Option<u64> {
    if x >= DOMAIN {
        return None;
    }

    let produto = u128::from(x).checked_mul(u128::from(fator))?;
    let resto = produto.checked_rem(u128::from(DOMAIN))?;
    u64::try_from(resto).ok()
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
    fn dominio_e_62_elevado_a_7() {
        assert_eq!(DOMAIN, 3_521_614_606_208);
    }

    #[test]
    fn k_inv_e_o_inverso_de_k() {
        // prova de uma vez que K é coprimo de DOMAIN e que a constante está certa.
        // sem isso, a multiplicação modular deixa de ser bijeção.
        let produto = u128::from(K).wrapping_mul(u128::from(K_INV));
        assert_eq!(produto % u128::from(DOMAIN), 1);
    }

    #[test]
    fn k_esta_dentro_do_dominio() {
        const {
            assert!(K < DOMAIN);
            assert!(K_INV < DOMAIN);
        }
    }

    #[test]
    fn rejeita_entrada_fora_do_dominio() {
        assert_eq!(obfuscate(DOMAIN), None);
        assert_eq!(deobfuscate(DOMAIN), None);
        assert_eq!(obfuscate(u64::MAX), None);
        assert_eq!(deobfuscate(u64::MAX), None);
    }

    #[test]
    fn aceita_o_limite_superior_do_dominio() {
        let ultimo = DOMAIN.saturating_sub(1);
        assert!(obfuscate(ultimo).is_some());
        assert!(deobfuscate(ultimo).is_some());
    }

    #[test]
    fn imagem_sempre_dentro_do_dominio() {
        // é essa propriedade que garante shortcode de no máximo 7 chars.
        let amostra = [
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
        for id in amostra {
            let saida = obfuscate(id).expect("id dentro do domínio");
            assert!(saida < DOMAIN, "obfuscate({id}) = {saida} saiu do domínio");
        }
    }

    #[test]
    fn imagem_dentro_do_dominio_em_massa() {
        for id in 0..100_000u64 {
            let saida = obfuscate(id).expect("id dentro do domínio");
            assert!(saida < DOMAIN, "obfuscate({id}) = {saida} saiu do domínio");
        }
    }

    #[test]
    fn roundtrip_extremos() {
        let amostra = [
            0,
            1,
            2,
            61,
            62,
            12_345,
            999_999_999,
            DOMAIN.saturating_sub(1),
        ];
        for id in amostra {
            let saida = obfuscate(id).expect("id dentro do domínio");
            assert_eq!(deobfuscate(saida), Some(id), "roundtrip falhou para {id}");
        }
    }

    #[test]
    fn roundtrip_em_massa() {
        for id in 0..100_000u64 {
            let saida = obfuscate(id).expect("id dentro do domínio");
            assert_eq!(deobfuscate(saida), Some(id), "roundtrip falhou para {id}");
        }
    }

    #[test]
    fn e_bijecao_sem_colisao() {
        let vistos: HashSet<u64> = (0..200_000u64).filter_map(obfuscate).collect();
        assert_eq!(vistos.len(), 200_000, "houve colisão");
    }

    #[test]
    fn ids_consecutivos_ficam_distantes() {
        // com multiplicação modular a diferença é sempre K (mod DOMAIN).
        // o que rejeitamos aqui é o incremento pequeno e previsível.
        for (a, b) in (0..2_000u64).zip(1..2_001u64) {
            let x = obfuscate(a).expect("id dentro do domínio");
            let y = obfuscate(b).expect("id dentro do domínio");
            let delta = x.abs_diff(y);
            assert!(
                delta > 1_000_000_000,
                "obfuscate({a}) e obfuscate({b}) ficaram a {delta} de distância"
            );
        }
    }
}
