//! Clé de licence : normalisation de la saisie et caractère de contrôle.
//!
//! Format : `OCAD-XXXX-XXXX-XXXX-XXXX`. Les 16 symboles viennent d'un alphabet
//! de 32 caractères sans `I`, `O`, `0` ni `1` (confusions à la saisie). Les 15
//! premiers sont aléatoires, le 16ᵉ est un caractère de contrôle Luhn mod 32 :
//! une faute de frappe ou l'inversion de deux caractères voisins est détectée
//! **avant** tout appel réseau. L'algorithme et les vecteurs de test sont
//! identiques côté serveur (voir docs/licences-passation.md, section 5.4).

/// Les 32 symboles autorisés, dans l'ordre qui définit leur valeur.
pub const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const PREFIX: &str = "OCAD";
const BODY_LEN: usize = 16;

/// Raison du rejet d'une saisie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    Empty,
    WrongLength,
    InvalidSymbol,
    BadChecksum,
}

fn value(symbol: u8) -> Option<u32> {
    ALPHABET.iter().position(|&a| a == symbol).map(|p| p as u32)
}

/// Caractère de contrôle Luhn mod 32 des 15 premiers symboles. `None` si un
/// symbole n'appartient pas à l'alphabet.
pub fn check_char(body: &[u8]) -> Option<u8> {
    const N: u32 = 32;
    let mut factor = 2u32;
    let mut sum = 0u32;
    for &symbol in body.iter().rev() {
        let code_point = value(symbol)?;
        let mut addend = factor * code_point;
        factor = if factor == 2 { 1 } else { 2 };
        addend = addend / N + addend % N;
        sum += addend;
    }
    Some(ALPHABET[((N - (sum % N)) % N) as usize])
}

/// Normalise une saisie libre (minuscules, espaces, tirets, préfixe facultatif)
/// en `OCAD-XXXX-XXXX-XXXX-XXXX`, ou explique pourquoi elle n'est pas valide.
/// Même règle que `normalizeKey` du serveur (src/domain/licenseKey.ts) : le
/// préfixe n'est retiré que s'il reste alors exactement 16 symboles.
pub fn normalize(input: &str) -> Result<String, KeyError> {
    let compact: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase();
    if compact.is_empty() {
        return Err(KeyError::Empty);
    }
    let body = match compact.strip_prefix(PREFIX) {
        Some(rest) if rest.len() == BODY_LEN => rest,
        _ => compact.as_str(),
    };
    let bytes = body.as_bytes();
    if bytes.len() != BODY_LEN {
        return Err(KeyError::WrongLength);
    }
    if bytes.iter().any(|&c| value(c).is_none()) {
        return Err(KeyError::InvalidSymbol);
    }
    if check_char(&bytes[..BODY_LEN - 1]) != Some(bytes[BODY_LEN - 1]) {
        return Err(KeyError::BadChecksum);
    }
    Ok(format!(
        "{PREFIX}-{}-{}-{}-{}",
        &body[0..4],
        &body[4..8],
        &body[8..12],
        &body[12..16]
    ))
}

/// Les quatre derniers symboles, seule partie de la clé affichée en clair.
pub fn suffix(key: &str) -> String {
    let chars: Vec<char> = key.chars().filter(|c| *c != '-').collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vecteurs partagés avec le serveur : toute divergence casserait
    /// l'activation de clés pourtant valides.
    const VECTORS: &[(&str, char, &str)] = &[
        ("ABCDEFGHJKLMNPQ", '9', "OCAD-ABCD-EFGH-JKLM-NPQ9"),
        ("222222222222222", 'S', "OCAD-2222-2222-2222-222S"),
        ("ZZZZZZZZZZZZZZZ", 'H', "OCAD-ZZZZ-ZZZZ-ZZZZ-ZZZH"),
        ("7XQ2M9KDT4HW3RC", 'P', "OCAD-7XQ2-M9KD-T4HW-3RCP"),
        ("AAAAAAAAAAAAAAA", 'A', "OCAD-AAAA-AAAA-AAAA-AAAA"),
        ("23456789ABCDEFG", 'V', "OCAD-2345-6789-ABCD-EFGV"),
    ];

    #[test]
    fn check_char_matches_shared_vectors() {
        for (body, check, _) in VECTORS {
            assert_eq!(check_char(body.as_bytes()), Some(*check as u8), "{body}");
        }
    }

    #[test]
    fn normalize_accepts_vectors_in_any_layout() {
        for (_, _, key) in VECTORS {
            assert_eq!(normalize(key).as_deref(), Ok(*key));
            let loose = key.to_lowercase().replace('-', " ");
            assert_eq!(normalize(&loose).as_deref(), Ok(*key));
            let without_prefix = key.trim_start_matches("OCAD-").replace('-', "");
            assert_eq!(normalize(&without_prefix).as_deref(), Ok(*key));
        }
    }

    #[test]
    fn normalize_rejects_typos_and_transpositions() {
        assert_eq!(normalize(""), Err(KeyError::Empty));
        assert_eq!(normalize("OCAD-7XQ2-M9KD"), Err(KeyError::WrongLength));
        assert_eq!(
            normalize("OCAD-7XQ2-M9KD-T4HW-3RC0"),
            Err(KeyError::InvalidSymbol),
            "0 n'appartient pas à l'alphabet"
        );
        assert_eq!(
            normalize("OCAD-7XQ2-M9KD-T4HW-3CRP"),
            Err(KeyError::BadChecksum),
            "inversion de deux caractères voisins"
        );
        assert_eq!(
            normalize("OCAD-7XQ2-M9KD-T4HW-3RCQ"),
            Err(KeyError::BadChecksum)
        );
    }

    #[test]
    fn the_prefix_is_only_removed_when_16_symbols_remain() {
        // « OCAD » + 12 symboles : le préfixe n'est pas retiré (il ne resterait
        // que 12 symboles), et le O n'appartient pas à l'alphabet.
        assert_eq!(
            normalize("OCAD-7XQ2-M9KD-T4HW"),
            Err(KeyError::InvalidSymbol)
        );
        assert_eq!(
            normalize("ocad7xq2m9kdt4hw3rcp").as_deref(),
            Ok("OCAD-7XQ2-M9KD-T4HW-3RCP")
        );
    }

    #[test]
    fn suffix_keeps_the_last_four_symbols() {
        assert_eq!(suffix("OCAD-7XQ2-M9KD-T4HW-3RCP"), "3RCP");
    }
}
