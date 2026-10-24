//! Corrections appliquées par le code après la rédaction. Le banc d'essai l'a
//! montré : le modèle local recopie le vocabulaire de la transcription malgré
//! la charte (7 à 10 mots interdits par compte rendu) et invente l'année des
//! dates ; une table de remplacement et la date de l'enregistrement les
//! corrigent à coup sûr. Le reste de la charte (ton, longueur des phrases)
//! reste l'affaire du modèle.

use regex::{Captures, Regex};

/// Remplacement lu dans la charte : `forbidden` (minuscules) devient
/// `replacement` ; un remplacement vide supprime le mot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconRule {
    pub forbidden: String,
    pub replacement: String,
}

fn strip_decoration(text: &str) -> String {
    text.trim()
        .trim_start_matches(['-', '•'])
        .trim()
        .trim_matches(|c: char| {
            matches!(
                c,
                '«' | '»' | '"' | '“' | '”' | '\'' | '’' | '*' | '_' | '`'
            ) || c.is_whitespace()
        })
        .to_string()
}

fn fold(text: &str) -> String {
    text.to_lowercase()
        .replace(['é', 'è', 'ê'], "e")
        .replace(['à', 'â'], "a")
}

/// Un mot ou une courte expression : pas une phrase d'explication.
fn is_term(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= 40 && text.split_whitespace().count() <= 4
}

/// Première proposition d'une cellule « À écrire » : « appel ou
/// visioconférence » → « appel ». « (à supprimer) » → suppression.
fn replacement_of(cell: &str) -> Option<String> {
    if fold(cell).contains("supprimer") {
        return Some(String::new());
    }
    let first = cell
        .split(" ou ")
        .next()
        .and_then(|s| s.split([',', '/', ';']).next())
        .map(strip_decoration)?;
    is_term(&first).then_some(first)
}

fn push_rules(rules: &mut Vec<LexiconRule>, forbidden_cell: &str, replacement: &str) {
    for item in forbidden_cell.split([',', '/', ';']) {
        let item = strip_decoration(item);
        if is_term(&item) && !rules.iter().any(|r| r.forbidden == item.to_lowercase()) {
            rules.push(LexiconRule {
                forbidden: item.to_lowercase(),
                replacement: replacement.to_string(),
            });
        }
    }
}

fn table_cells(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    let inner = line.strip_prefix('|')?;
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    Some(inner.split('|').map(|c| c.trim().to_string()).collect())
}

/// Règles de vocabulaire de la charte : tableaux « Interdit | À écrire »
/// (en-têtes reconnus : interdit, à éviter, proscrit / à écrire, remplacer,
/// privilégier, préférer, plutôt) et lignes « deadline → échéance ».
pub fn parse_lexicon(charter: &str) -> Vec<LexiconRule> {
    let mut rules = Vec::new();
    let lines: Vec<&str> = charter.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let header = table_cells(lines[i]);
        let is_rule_row = lines
            .get(i + 1)
            .and_then(|l| table_cells(l))
            .is_some_and(|cells| {
                cells
                    .iter()
                    .all(|c| c.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
            });
        if let (Some(header), true) = (header, is_rule_row) {
            let keys: Vec<String> = header.iter().map(|h| fold(h)).collect();
            let forbidden_col = keys.iter().position(|k| {
                ["interdit", "eviter", "proscrit", "bannir", "ne pas"]
                    .iter()
                    .any(|w| k.contains(w))
            });
            let replacement_col = keys.iter().position(|k| {
                [
                    "ecrire",
                    "remplac",
                    "privilegier",
                    "preferer",
                    "plutot",
                    "utiliser",
                ]
                .iter()
                .any(|w| k.contains(w))
            });
            i += 2;
            while let Some(cells) = lines.get(i).and_then(|l| table_cells(l)) {
                if let (Some(f), Some(r)) = (forbidden_col, replacement_col) {
                    if let (Some(forbidden), Some(replacement)) =
                        (cells.get(f), cells.get(r).and_then(|c| replacement_of(c)))
                    {
                        push_rules(&mut rules, forbidden, &replacement);
                    }
                }
                i += 1;
            }
            continue;
        }
        for arrow in ["→", "->", "=>"] {
            if let Some((left, right)) = lines[i].split_once(arrow) {
                if let Some(replacement) = replacement_of(right) {
                    push_rules(&mut rules, left, &replacement);
                }
                break;
            }
        }
        i += 1;
    }
    rules
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Pluriel du premier mot : « lettre d'information » → « lettres d'information ».
fn pluralize(text: &str) -> String {
    match text.split_once(' ') {
        Some((first, rest)) => format!("{} {rest}", pluralize(first)),
        None if text.ends_with(['s', 'x', 'z']) => text.to_string(),
        None => format!("{text}s"),
    }
}

fn starts_with_vowel(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| "aeiouyhéèêëàâîïôûAEIOUYHÉÈÊÀÂÎÔÛ".contains(c))
}

/// Applique les règles, les plus longues d'abord (« e-mail » avant « mail »).
/// Mot entier, sans tenir compte de la casse ; majuscule initiale, pluriel
/// simple (« Newsletters » → « Lettres d'information ») et élision de
/// l'article (« la deadline » → « l'échéance ») gérés. Une suppression
/// emporte la virgule qui suit (« Du coup, nous » → « Nous »).
pub fn apply_lexicon(text: &str, rules: &[LexiconRule]) -> String {
    let mut rules: Vec<&LexiconRule> = rules.iter().collect();
    rules.sort_by_key(|r| std::cmp::Reverse(r.forbidden.chars().count()));
    let mut text = text.to_string();
    for rule in rules {
        let word = regex::escape(&rule.forbidden);
        if rule.replacement.is_empty() {
            let Ok(pattern) = Regex::new(&format!(r"(?i)\b{word}s?\b[ \t]*,?[ \t]*(\p{{L}})?"))
            else {
                continue;
            };
            text = pattern
                .replace_all(&text, |caps: &Captures| {
                    let next = caps.get(1).map_or("", |m| m.as_str());
                    if caps[0].chars().next().is_some_and(char::is_uppercase) {
                        capitalize(next)
                    } else {
                        next.to_string()
                    }
                })
                .into_owned();
            continue;
        }
        // « shooting photo » → « séance photo », pas « séance photo photo » :
        // le dernier mot du remplacement, s'il suit déjà, est absorbé.
        let repeated = match rule.replacement.rsplit_once(' ') {
            Some((_, last)) if last.chars().count() > 2 => {
                format!(r"(?:\s+{}s?\b)?", regex::escape(last))
            }
            _ => String::new(),
        };
        let Ok(pattern) = Regex::new(&format!(
            r"(?i)(?:\b(le|la|de)\s+)?\b{word}(s?)\b{repeated}"
        )) else {
            continue;
        };
        text = pattern
            .replace_all(&text, |caps: &Captures| {
                let plural = !caps[2].is_empty() && !rule.forbidden.ends_with('s');
                let replacement = if plural {
                    pluralize(&rule.replacement)
                } else {
                    rule.replacement.clone()
                };
                match caps.get(1) {
                    Some(article) if !plural && starts_with_vowel(&replacement) => {
                        let elided = if article.as_str().eq_ignore_ascii_case("de") {
                            "d'"
                        } else {
                            "l'"
                        };
                        let elided = if article.as_str().starts_with(char::is_uppercase) {
                            capitalize(elided)
                        } else {
                            elided.to_string()
                        };
                        format!("{elided}{replacement}")
                    }
                    Some(article) => format!("{} {replacement}", article.as_str()),
                    None if caps[0].starts_with(char::is_uppercase) => capitalize(&replacement),
                    None => replacement,
                }
            })
            .into_owned();
    }
    // Espaces laissés par une suppression.
    text.lines()
        .map(|line| {
            let mut line = line.to_string();
            while line.contains("  ") {
                line = line.replace("  ", " ");
            }
            for punct in [",", ".", ")", ";"] {
                line = line.replace(&format!(" {punct}"), punct);
            }
            line.replace("( ", "(")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// La charte demande-t-elle les noms des personnes en gras ?
pub fn charter_wants_bold_names(charter: &str) -> bool {
    let folded = fold(charter);
    folded.contains("en gras") && (folded.contains("nom") || folded.contains("personne"))
}

/// Noms de la ligne « Participants : » (fonctions entre parenthèses
/// retirées) : au moins deux mots commençant par une majuscule.
fn participant_names(text: &str) -> Vec<String> {
    let pattern = Regex::new(
        r"(?i)^\s*(?:[-*]\s+)?(?:\*\*|__)?\s*participants?\s*(?:\*\*|__)?\s*:\s*(?:\*\*|__)?\s*(.*)$",
    )
    .expect("motif des participants valide");
    let parentheses = Regex::new(r"\([^)]*\)").expect("motif des parenthèses valide");
    let Some(line) = text
        .lines()
        .take(15)
        .find_map(|line| pattern.captures(line))
    else {
        return Vec::new();
    };
    let value = parentheses.replace_all(&line[1], "");
    value
        .split([',', ';'])
        .flat_map(|part| part.split(" et "))
        .map(|name| name.replace(['*', '_'], "").trim().to_string())
        .filter(|name| {
            let words: Vec<&str> = name.split_whitespace().collect();
            (2..=5).contains(&words.len())
                && words
                    .iter()
                    .all(|w| w.chars().next().is_some_and(char::is_uppercase))
        })
        .collect()
}

/// Met en gras chaque occurrence des participants, quand la charte le
/// demande : le modèle le fait une fois sur quatre (banc d'essai).
pub fn bold_participants(text: &str) -> String {
    let mut names = participant_names(text);
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut text = text.to_string();
    for name in names {
        let escaped = regex::escape(&name);
        // Gras existant retiré d'abord, pour ne jamais doubler les marqueurs.
        if let Ok(bold) = Regex::new(&format!(r"\*\*{escaped}\*\*")) {
            text = bold.replace_all(&text, name.as_str()).into_owned();
        }
        if let Ok(plain) = Regex::new(&format!(r"\b{escaped}\b")) {
            text = plain
                .replace_all(&text, format!("**{name}**").as_str())
                .into_owned();
        }
    }
    text
}

/// Sépare d'une ligne vide les lignes de l'en-tête (« Date : »,
/// « Participants : »), avant la première section : en Markdown, deux lignes
/// qui se suivent forment un seul paragraphe et s'afficheraient bout à bout.
pub fn separate_header_lines(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut in_header = true;
    for line in text.lines() {
        if line.starts_with("## ") {
            in_header = false;
        }
        let plain = |l: &str| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with('#') && !l.starts_with('|') && !l.starts_with("- ")
        };
        if in_header && plain(line) && out.last().is_some_and(|previous| plain(previous)) {
            out.push("");
        }
        out.push(line);
    }
    out.join("\n")
}

/// Remplace la valeur de la ligne « Date : » de l'en-tête par la date de
/// l'enregistrement, que le modèle invente parfois (« jeudi 22 octobre 2023 »).
pub fn fix_date_header(text: &str, date_label: &str) -> String {
    let pattern = Regex::new(
        r"(?i)^(\s*(?:[-*]\s+)?(?:\*\*|__)?\s*date(?:\s+de\s+la\s+r[ée]union)?\s*(?:\*\*|__)?\s*:\s*(?:\*\*|__)?)\s*.*$",
    )
    .expect("motif de date valide");
    let mut done = false;
    text.lines()
        .enumerate()
        .map(|(index, line)| {
            if !done && index < 15 {
                if let Some(caps) = pattern.captures(line) {
                    done = true;
                    return format!("{} {date_label}", caps[1].trim_end());
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHARTER: &str = "## Lexique\n\n| Interdit | À écrire |\n|---|---|\n| deadline | échéance |\n| call | appel ou visioconférence |\n| mail, e-mail | courriel |\n| newsletter | lettre d'information |\n| super, top | (à supprimer) |\n| du coup | (à supprimer) |\n\nAutres règles :\n- « ASAP » → « dès que possible »\n- process -> processus\n- Les phrases longues sont à éviter.";

    #[test]
    fn rules_are_read_from_tables_and_arrows() {
        let rules = parse_lexicon(CHARTER);
        let find = |w: &str| {
            rules
                .iter()
                .find(|r| r.forbidden == w)
                .map(|r| r.replacement.as_str())
        };
        assert_eq!(find("deadline"), Some("échéance"));
        assert_eq!(find("call"), Some("appel"));
        assert_eq!(find("e-mail"), Some("courriel"));
        assert_eq!(find("mail"), Some("courriel"));
        assert_eq!(find("super"), Some(""));
        assert_eq!(find("du coup"), Some(""));
        assert_eq!(find("asap"), Some("dès que possible"));
        assert_eq!(find("process"), Some("processus"));
        assert_eq!(rules.len(), 10);
    }

    #[test]
    fn a_table_without_rule_headers_is_ignored() {
        let rules = parse_lexicon("| Couleur | Code |\n|---|---|\n| rouge | #FF0000 |");
        assert!(rules.is_empty());
    }

    #[test]
    fn words_are_replaced_whole_with_case_plural_and_elision() {
        let rules = parse_lexicon(CHARTER);
        let text = "Newsletters envoyées par e-mail avant la deadline. Du coup, super réunion.\n| **Julie** | Envoyer le mail | ASAP |";
        assert_eq!(
            apply_lexicon(text, &rules),
            "Lettres d'information envoyées par courriel avant l'échéance. Réunion.\n| **Julie** | Envoyer le courriel | Dès que possible |"
        );
        // Jamais à l'intérieur d'un mot.
        assert_eq!(
            apply_lexicon("Le processus et le mailing", &rules),
            "Le processus et le mailing"
        );
        assert_eq!(apply_lexicon("C'est top, merci.", &rules), "C'est merci.");
    }

    #[test]
    fn a_word_already_following_the_replacement_is_not_doubled() {
        let rules = vec![LexiconRule {
            forbidden: "shooting".into(),
            replacement: "séance photo".into(),
        }];
        assert_eq!(
            apply_lexicon("Organisation du shooting photo et du shooting.", &rules),
            "Organisation du séance photo et du séance photo."
        );
        assert_eq!(
            apply_lexicon("Les shootings photos", &rules),
            "Les séances photo"
        );
    }

    #[test]
    fn participants_are_bolded_everywhere_when_the_charter_asks() {
        assert!(charter_wants_bold_names(
            "Les noms des personnes s'écrivent toujours en gras."
        ));
        assert!(!charter_wants_bold_names("Les titres sont en gras."));
        let text = "# CR\n**Participants :** Claire Moreau (agence), **Thomas Lefebvre** (chef de projet) et Inès Ben Ali\n## Plan\n| Claire Moreau | Rappeler |\nThomas Lefebvre envoie le planning à M. Vasseur.";
        assert_eq!(
            bold_participants(text),
            "# CR\n**Participants :** **Claire Moreau** (agence), **Thomas Lefebvre** (chef de projet) et **Inès Ben Ali**\n## Plan\n| **Claire Moreau** | Rappeler |\n**Thomas Lefebvre** envoie le planning à M. Vasseur."
        );
        let once = bold_participants(text);
        assert_eq!(bold_participants(&once), once, "idempotent");
        assert_eq!(bold_participants("# Sans en-tête"), "# Sans en-tête");
    }

    #[test]
    fn header_lines_are_separated_but_sections_are_untouched() {
        let text = "# T\n**Date :** x\n**Participants :** y\n## Synthèse\nUne.\nDeux.";
        assert_eq!(
            separate_header_lines(text),
            "# T\n**Date :** x\n\n**Participants :** y\n## Synthèse\nUne.\nDeux."
        );
        let done = separate_header_lines(text);
        assert_eq!(separate_header_lines(&done), done, "idempotent");
    }

    #[test]
    fn the_date_line_of_the_header_is_set_to_the_recording_date() {
        let text = "# Compte rendu\n\n**Date :** jeudi 22 octobre 2023\n**Participants :** A";
        assert_eq!(
            fix_date_header(text, "jeudi 22 octobre 2026"),
            "# Compte rendu\n\n**Date :** jeudi 22 octobre 2026\n**Participants :** A"
        );
        assert_eq!(
            fix_date_header(
                "Date : 22/10\n## Décisions\n- Date : lundi",
                "jeudi 22 octobre 2026"
            ),
            "Date : jeudi 22 octobre 2026\n## Décisions\n- Date : lundi"
        );
        let untouched = "# Titre\n## Décisions\n- Rien";
        assert_eq!(fix_date_header(untouched, "x"), untouched);
    }
}
