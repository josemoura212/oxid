const BASE: u64 = 62;

/// Maior quantidade de dígitos que `u64::MAX` ocupa nesta base.
const MAX_LEN: usize = 11;

/// Ordem do alfabeto: `0-9`, `a-z`, `A-Z`. O `match` do [`decode`] espelha esta
/// ordem — mudar uma sem a outra invalida todo shortcode já emitido.
const ALPHABET: &[u8; 62] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

pub fn encode(n: u64) -> String {
    let mut n = n;
    let mut buf = Vec::with_capacity(MAX_LEN);

    // do-while: zero também tem um dígito, então o corpo roda ao menos uma vez.
    // a condição do `while let` nunca falha — o resto de `% BASE` sempre cabe no
    // alfabeto. Quem encerra o laço é o `n == 0` no fim do corpo.
    while let Some(&byte) = usize::try_from(n % BASE).ok().and_then(|i| ALPHABET.get(i)) {
        buf.push(byte);

        n /= BASE;
        if n == 0 {
            break;
        }
    }

    // a divisão sucessiva produz do dígito menos significativo para o mais.
    buf.reverse();
    buf.iter().map(|&b| char::from(b)).collect()
}

pub fn decode(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }

    let mut n: u64 = 0;

    // bytes, não chars: qualquer coisa fora do ASCII cai no braço de rejeição.
    for byte in s.bytes() {
        let digito = match byte {
            b'0'..=b'9' => byte.checked_sub(b'0')?,
            b'a'..=b'z' => byte.checked_sub(b'a')?.checked_add(10)?,
            b'A'..=b'Z' => byte.checked_sub(b'A')?.checked_add(36)?,
            _ => return None,
        };
        n = n.checked_mul(BASE)?.checked_add(u64::from(digito))?;
    }

    Some(n)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn encode_valores_conhecidos() {
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
    fn decode_valores_conhecidos() {
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
    fn decode_e_case_sensitive() {
        assert_ne!(decode("a"), decode("A"));
    }

    #[test]
    fn decode_ignora_zeros_a_esquerda() {
        // shortcodes são padded para 7 chars; o pad não pode alterar o valor.
        assert_eq!(decode("0000001"), Some(1));
        assert_eq!(decode("0000000"), Some(0));
    }

    #[test]
    fn roundtrip_extremos() {
        for n in [0, 1, 9, 10, 35, 36, 61, 62, 63, 3843, u64::MAX] {
            assert_eq!(decode(&encode(n)), Some(n), "roundtrip falhou para {n}");
        }
    }

    #[test]
    fn roundtrip_amostra_espacada() {
        // potências de um primo cobrem várias ordens de grandeza até estourar u64.
        let mut n: u64 = 1;
        while let Some(proximo) = n.checked_mul(7919) {
            assert_eq!(decode(&encode(n)), Some(n), "roundtrip falhou para {n}");
            n = proximo;
        }
    }

    #[test]
    fn decode_rejeita_caracteres_invalidos() {
        for s in [
            "-", "_", " ", "!", "+", "/", "=", "a-b", "1 2", "ção", "日本",
        ] {
            assert_eq!(decode(s), None, "deveria rejeitar {s:?}");
        }
    }

    #[test]
    fn decode_rejeita_string_vazia() {
        // DECISÃO: "" não é shortcode válido. Hoje o decode devolve Some(0).
        assert_eq!(decode(""), None);
    }

    #[test]
    fn decode_rejeita_overflow() {
        // u64::MAX tem 11 dígitos em base62; 12 no topo do alfabeto estoura.
        assert_eq!(decode("ZZZZZZZZZZZZ"), None);
        assert_eq!(decode("ZZZZZZZZZZZZZZZZZZZZZZZZ"), None);
    }

    #[test]
    fn encode_produz_apenas_o_alfabeto() {
        let mut n: u64 = 1;
        while let Some(proximo) = n.checked_mul(1_000_003) {
            let s = encode(n);
            assert!(
                s.chars().all(|c| c.is_ascii_alphanumeric()),
                "encode({n}) = {s:?} tem char fora do alfabeto"
            );
            n = proximo;
        }
    }

    #[test]
    fn encode_nunca_devolve_vazio() {
        for n in [0, 1, u64::MAX] {
            assert!(!encode(n).is_empty(), "encode({n}) devolveu vazio");
        }
    }
}
