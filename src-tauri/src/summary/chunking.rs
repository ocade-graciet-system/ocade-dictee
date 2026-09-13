//! Estimation de tokens et découpage d'une transcription en tranches qui
//! tiennent dans le contexte du modèle, aux frontières de phrases.

/// 1 token ≈ 3,5 caractères en français (tokenizer Mistral/Qwen) …
pub const CHARS_PER_TOKEN: f64 = 3.5;
/// … avec 20 % de marge, car l'estimation est optimiste sur les noms propres
/// et les chiffres.
pub const SAFETY_MARGIN: f64 = 1.2;
/// Tranche maximale : 9 000 tokens estimés. Le serveur tourne avec un contexte
/// de 12 288 tokens (`super::assets::CTX_SIZE`) et la plus longue sortie
/// demandée vaut 1 800 tokens (compte-rendu ; 1 400 pour les notes) : une
/// tranche de 9 000 tokens, les prompts et 1 800 tokens de sortie tiennent
/// sous 12 288.
pub const MAX_CHUNK_TOKENS: usize = 9_000;

/// Nombre de tokens estimé, marge comprise.
pub fn estimate_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        return 0;
    }
    (chars as f64 / CHARS_PER_TOKEN * SAFETY_MARGIN).ceil() as usize
}

/// Nombre maximal de caractères pour rester sous `max_tokens` estimés.
pub fn max_chars_for(max_tokens: usize) -> usize {
    (max_tokens as f64 * CHARS_PER_TOKEN / SAFETY_MARGIN).floor() as usize
}

/// Découpe `text` en tranches de `max_tokens` estimés au plus, en coupant
/// après une fin de phrase (`.`, `!`, `?`, `…`) ou un saut de ligne. Une
/// phrase plus longue qu'une tranche est coupée sur une espace. Un texte qui
/// tient en une tranche est renvoyé tel quel (trim) ; un texte vide → aucune
/// tranche.
pub fn split_into_chunks(text: &str, max_tokens: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if estimate_tokens(text) <= max_tokens {
        return vec![text.to_string()];
    }
    let max_chars = max_chars_for(max_tokens).max(1);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for unit in split_units(text) {
        for piece in hard_split(unit, max_chars) {
            let piece_len = piece.chars().count();
            if current_len > 0 && current_len + 1 + piece_len > max_chars {
                chunks.push(std::mem::take(&mut current));
                current_len = 0;
            }
            if current_len > 0 {
                current.push(' ');
                current_len += 1;
            }
            current.push_str(piece);
            current_len += piece_len;
        }
    }
    if current_len > 0 {
        chunks.push(current);
    }
    chunks
}

fn is_sentence_end(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

/// Unités indivisibles : phrases (terminées par une ponctuation finale suivie
/// d'une espace) ou lignes. Les espaces de séparation sont retirées.
fn split_units(text: &str) -> Vec<&str> {
    let mut units = Vec::new();
    let mut start = 0;
    let mut prev_end = false;
    for (idx, c) in text.char_indices() {
        if c == '\n' || (prev_end && c.is_whitespace()) {
            let unit = text[start..idx].trim();
            if !unit.is_empty() {
                units.push(unit);
            }
            start = idx + c.len_utf8();
        }
        prev_end = is_sentence_end(c);
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        units.push(last);
    }
    units
}

/// Coupe une unité trop longue sur des espaces (ou brutalement, sans espace).
fn hard_split(unit: &str, max_chars: usize) -> Vec<&str> {
    if unit.chars().count() <= max_chars {
        return vec![unit];
    }
    let mut pieces = Vec::new();
    let mut rest = unit;
    while rest.chars().count() > max_chars {
        let limit_byte = rest
            .char_indices()
            .nth(max_chars)
            .map(|(b, _)| b)
            .unwrap_or(rest.len());
        let cut = rest[..limit_byte]
            .rfind(char::is_whitespace)
            .filter(|&b| b > 0)
            .unwrap_or(limit_byte);
        pieces.push(rest[..cut].trim_end());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_estimate_uses_ratio_and_margin() {
        assert_eq!(estimate_tokens(""), 0);
        // 35 caractères → 10 tokens × 1,2 = 12.
        assert_eq!(estimate_tokens(&"a".repeat(35)), 12);
        assert_eq!(max_chars_for(MAX_CHUNK_TOKENS), 26_250);
        assert!(estimate_tokens(&"é".repeat(26_250)) <= MAX_CHUNK_TOKENS);
        assert!(estimate_tokens(&"é".repeat(26_251)) > MAX_CHUNK_TOKENS);
    }

    #[test]
    fn short_text_is_a_single_untouched_chunk() {
        assert_eq!(
            split_into_chunks("  Bonjour à tous. Merci.  ", 100),
            vec!["Bonjour à tous. Merci."]
        );
        assert!(split_into_chunks("   \n ", 100).is_empty());
    }

    #[test]
    fn long_text_is_cut_at_sentence_boundaries() {
        let sentence =
            "Le conseil a validé le budget de trente mille euros pour le second semestre. ";
        let text = sentence.repeat(1500); // ≈ 115 000 caractères
        let chunks = split_into_chunks(&text, MAX_CHUNK_TOKENS);
        assert!(chunks.len() >= 5, "{} tranches", chunks.len());
        for chunk in &chunks {
            assert!(estimate_tokens(chunk) <= MAX_CHUNK_TOKENS);
            assert!(
                chunk.ends_with('.'),
                "coupe en fin de phrase: …{}",
                &chunk[chunk.len() - 20..]
            );
            assert!(chunk.starts_with("Le conseil"));
        }
        let rebuilt: String = chunks.join(" ");
        assert_eq!(
            rebuilt.split_whitespace().count(),
            text.split_whitespace().count()
        );
    }

    #[test]
    fn paragraphs_are_boundaries_too() {
        let paragraph = "sans ponctuation mais avec des mots ".repeat(400); // ≈ 14 400 caractères
        let text = format!("{paragraph}\n{paragraph}\n{paragraph}");
        let chunks = split_into_chunks(&text, MAX_CHUNK_TOKENS);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| !c.contains('\n')));
    }

    #[test]
    fn oversized_sentence_is_hard_split_on_spaces() {
        let text = "mot ".repeat(20_000); // 80 000 caractères sans ponctuation
        let chunks = split_into_chunks(&text, MAX_CHUNK_TOKENS);
        assert!(chunks.len() >= 4);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= max_chars_for(MAX_CHUNK_TOKENS));
            assert!(!chunk.starts_with(' ') && !chunk.ends_with(' '));
        }
    }
}
