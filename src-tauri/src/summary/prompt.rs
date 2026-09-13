//! Gabarit et prompts (spec §6), en constantes. Toute modification du gabarit
//! doit rester cohérente avec `check_template` : ce que les prompts demandent
//! est exactement ce que le contrôle accepte.
//!
//! Le modèle local n'obéit pas au mot près : sur une vraie transcription,
//! Ministral rend ses titres en gras (`## **Résumé**`, `## Résumé :`) et
//! intercale des lignes `---`. `clean_output` normalise donc la sortie
//! (titres ramenés à leur forme canonique, séparateurs retirés) *avant* le
//! contrôle, plutôt que de rejeter un compte-rendu correct sur sa mise en
//! forme. Ce qui reste refusé relève du fond : plusieurs comptes-rendus
//! collés, une section inconnue, une section obligatoire absente.

pub const SYSTEM_PROMPT: &str = "Tu es un assistant de rédaction professionnel. Tu écris uniquement en français, dans un style clair et neutre. Tu reformules et organises le contenu fourni sans rien inventer : aucune information absente de la transcription, aucun commentaire, aucune introduction ni conclusion hors du gabarit demandé.";

/// Structure imposée du compte-rendu, commune au prompt direct et au prompt
/// final. Les contraintes « un seul compte-rendu », « un seul titre », « pas de
/// séparateur » et « pas de bloc de code » viennent de l'essai comparatif : le
/// modèle rendait sinon plusieurs comptes-rendus, un par partie, collés par une
/// ligne de séparation. La dernière phrase (titres exacts, sans gras ni
/// deux-points) vient de la passe réelle sur Ministral.
pub const TEMPLATE_INSTRUCTIONS: &str = "Rédige un seul compte-rendu global en Markdown, de 250 à 450 mots, en respectant exactement cette structure : une seule ligne `# ` suivie d'un titre court ; `## Résumé` : 3 à 6 phrases ; `## Points clés` : liste à puces des informations importantes (faits, chiffres, noms, dates) ; `## Décisions et actions` : liste à puces « qui, quoi, quand » — s'il n'y a ni décision ni action, n'écris ni la section ni une mention de son absence. N'ajoute aucune autre section, ne reproduis pas les parties une par une, n'écris aucune ligne de séparation `---` et n'utilise aucun bloc de code (pas de ```). Les titres de section s'écrivent exactement `## Résumé`, `## Points clés`, `## Décisions et actions`, sans gras, sans deux-points, sans ligne de séparation.";

/// Rappel ajouté au prompt lors de l'unique relance après un gabarit incomplet.
pub const STRICT_REMINDER: &str = "RAPPEL STRICT : la réponse précédente ne respectait pas la structure demandée. Réponds uniquement avec le compte-rendu, exactement dans cette structure et sans aucune autre section ni commentaire : une seule ligne `# ` avec un titre court, puis `## Résumé`, puis `## Points clés`, puis (seulement s'il y a des décisions ou des actions) `## Décisions et actions`. Les titres de section s'écrivent exactement `## Résumé`, `## Points clés`, `## Décisions et actions`, sans gras, sans deux-points, sans ligne de séparation. N'écris aucune ligne de séparation `---` et n'utilise aucun bloc de code.";

/// Rappel ajouté au prompt lorsque la réponse précédente a été tronquée faute
/// de place (sortie coupée au budget de tokens).
pub const CONCISE_REMINDER: &str = "La réponse précédente a été coupée car trop longue. Réponds avec le même gabarit, en 250 à 450 mots au maximum, sans rien ajouter d'autre.";

pub const SECTION_SUMMARY: &str = "## Résumé";
pub const SECTION_KEY_POINTS: &str = "## Points clés";
pub const SECTION_DECISIONS: &str = "## Décisions et actions";

/// Messages (système, utilisateur) d'un appel de complétion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessages {
    pub system: String,
    pub user: String,
}

/// Compte-rendu direct : la transcription tient en une tranche.
pub fn build_direct_messages(text: &str) -> ChatMessages {
    ChatMessages {
        system: SYSTEM_PROMPT.to_string(),
        user: format!("{TEMPLATE_INSTRUCTIONS} Transcription : {text}"),
    }
}

/// Notes intermédiaires pour la tranche `part_index` (1-based) sur `total`.
pub fn build_notes_messages(part: &str, part_index: u32, total: u32) -> ChatMessages {
    ChatMessages {
        system: SYSTEM_PROMPT.to_string(),
        user: format!(
            "Voici la partie {part_index} sur {total} de la transcription d'un enregistrement. Rédige des notes détaillées en puces (faits, chiffres, noms, dates, décisions, actions), dans l'ordre, sans rien inventer et sans résumer à outrance. 25 puces maximum, une ligne par puce. Transcription : {part}"
        ),
    }
}

/// Compte-rendu final à partir des notes de toutes les tranches : un seul
/// compte-rendu qui synthétise l'ensemble, pas une section par partie.
pub fn build_final_messages(notes: &[String]) -> ChatMessages {
    let joined = notes
        .iter()
        .enumerate()
        .map(|(i, n)| format!("Partie {} :\n{}", i + 1, n.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    ChatMessages {
        system: SYSTEM_PROMPT.to_string(),
        user: format!(
            "Voici les notes prises sur les {} parties d'un enregistrement. À partir de ces notes uniquement, {} Écris un seul compte-rendu global qui synthétise l'ensemble des parties, et non un compte-rendu ou une section par partie. Notes : {}",
            notes.len(),
            lowercase_first(TEMPLATE_INSTRUCTIONS),
            joined
        ),
    }
}

fn lowercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Écart au gabarit détecté par `check_template`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateIssue {
    MissingTitle,
    /// Plusieurs lignes `# ` : le modèle a rendu plusieurs comptes-rendus.
    MultipleTitles(usize),
    MissingSummary,
    MissingKeyPoints,
    UnexpectedSection(String),
    /// Ligne de séparation (`---`, `***`, `___`), signe de documents collés.
    /// `clean_output` les retire toutes : ce cas ne se produit plus sur une
    /// sortie nettoyée et ne subsiste que comme filet pour un contrôle mené
    /// directement sur du Markdown brut.
    Separator,
}

/// Ligne de séparation Markdown (« thematic break » CommonMark) : au moins
/// trois fois le même marqueur `-`, `*` ou `_`, seuls sur la ligne, espaces
/// autorisés entre eux — `---`, `***`, `___`, `----`, `- - -`.
fn is_separator(line: &str) -> bool {
    let line = line.trim();
    let marker = match line.chars().next() {
        Some(c @ ('-' | '*' | '_')) => c,
        _ => return false,
    };
    let mut markers = 0usize;
    for c in line.chars() {
        if c == marker {
            markers += 1;
        } else if !c.is_whitespace() {
            return false;
        }
    }
    markers >= 3
}

/// Retire les marqueurs d'emphase encadrants (`**`, `__`, `*`, `_`), au besoin
/// plusieurs fois (`***Résumé***`), et les espaces autour.
fn strip_emphasis(text: &str) -> &str {
    let mut text = text.trim();
    loop {
        let inner = ["**", "__", "*", "_"].into_iter().find_map(|marker| {
            let inner = text.strip_prefix(marker)?.strip_suffix(marker)?.trim();
            (!inner.is_empty()).then_some(inner)
        });
        match inner {
            Some(inner) => text = inner,
            None => return text,
        }
    }
}

/// Minuscule sans accent ; `None` pour une marque combinante, si bien qu'un
/// « é » composé (`e` + U+0301) et un « é » précomposé donnent la même clé.
fn fold_char(c: char) -> Option<char> {
    if ('\u{0300}'..='\u{036f}').contains(&c) {
        return None;
    }
    let c = c.to_lowercase().next().unwrap_or(c);
    Some(match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        'ÿ' => 'y',
        other => other,
    })
}

/// Clé de comparaison d'un titre de section : minuscules, accents retirés,
/// `&` lu « et », toute autre ponctuation (trait d'union compris) ramenée à un
/// espace, espaces repliés. « Points-clés », « POINTS CLEFS » et
/// « Points clés » donnent la même clé.
fn section_key(text: &str) -> String {
    let mut key = String::new();
    for c in text.chars() {
        match fold_char(c) {
            Some(c) if c.is_alphanumeric() => key.push(c),
            Some('&') => key.push_str(" et "),
            Some(_) => key.push(' '),
            None => {}
        }
    }
    key.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Forme canonique de la section nommée par `text`, ou `None` si ce n'est pas
/// une des trois sections du gabarit. Une numérotation en tête (« 1. Résumé »)
/// est ignorée.
fn canonical_section(text: &str) -> Option<&'static str> {
    let key = section_key(text);
    let key = key
        .split_once(' ')
        .filter(|(head, _)| head.chars().all(|c| c.is_ascii_digit()))
        .map_or(key.as_str(), |(_, rest)| rest);
    match key {
        "resume" => Some(SECTION_SUMMARY),
        "points cles" | "points cle" | "points clefs" | "points clef" => Some(SECTION_KEY_POINTS),
        "decisions et actions"
        | "decisions et action"
        | "decision et action"
        | "decisions actions"
        | "decisions" => Some(SECTION_DECISIONS),
        _ => None,
    }
}

/// Réécrit une ligne de titre ATX : emphase, dièses de fermeture et deux-points
/// finaux retirés, sections du gabarit ramenées à leur forme canonique exacte.
/// `None` si la ligne n'est pas un titre (le `#` doit être suivi d'un espace,
/// comme en CommonMark : `#mot-clé` reste du texte).
fn normalize_heading(line: &str) -> Option<String> {
    // L'emphase peut envelopper la ligne entière : `**## Résumé**`.
    let line = strip_emphasis(line.trim());
    let level = line.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut text = rest.trim();
    loop {
        let before = text.len();
        text = strip_emphasis(text.trim_end_matches('#'));
        text = text.trim_end_matches(|c: char| c == ':' || c == '\u{ff1a}' || c.is_whitespace());
        if text.len() == before {
            break;
        }
    }
    if text.is_empty() {
        return Some("#".repeat(level));
    }
    if level == 2 {
        if let Some(canonical) = canonical_section(text) {
            return Some(canonical.to_string());
        }
    }
    Some(format!("{} {text}", "#".repeat(level)))
}

/// Passe ligne à ligne : titres normalisés, lignes de séparation retirées où
/// qu'elles soient, espaces de fin et lignes vides en trop repliés.
fn normalize_lines(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if is_separator(line) {
            continue;
        }
        let line = normalize_heading(line).unwrap_or_else(|| line.trim_end().to_string());
        if line.is_empty() {
            // Une seule ligne vide de séparation, jamais en tête.
            if lines.last().is_none_or(|previous| previous.is_empty()) {
                continue;
            }
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Contrôle du gabarit (§6) : exactement une ligne `# `, `## Résumé`,
/// `## Points clés` ; `## Décisions et actions` facultatif ; toute autre ligne
/// `## ` et toute ligne de séparation sont refusées. Les sous-titres `### `
/// à l'intérieur d'une section restent acceptés. À appeler sur une sortie
/// passée par `clean_output` : la mise en forme y est déjà normalisée, ce qui
/// reste refusé ici relève du fond.
pub fn check_template(markdown: &str) -> Result<(), TemplateIssue> {
    let mut titles = 0usize;
    let mut has_summary = false;
    let mut has_key_points = false;
    for line in markdown.lines() {
        let line = line.trim();
        if is_separator(line) {
            return Err(TemplateIssue::Separator);
        }
        if let Some(rest) = line.strip_prefix("## ") {
            let heading = format!("## {}", rest.trim());
            if heading == SECTION_SUMMARY {
                has_summary = true;
            } else if heading == SECTION_KEY_POINTS {
                has_key_points = true;
            } else if heading != SECTION_DECISIONS {
                return Err(TemplateIssue::UnexpectedSection(heading));
            }
        } else if line
            .strip_prefix("# ")
            .is_some_and(|title| !title.trim().is_empty())
        {
            titles += 1;
        }
    }
    match titles {
        0 => return Err(TemplateIssue::MissingTitle),
        1 => {}
        several => return Err(TemplateIssue::MultipleTitles(several)),
    }
    if !has_summary {
        return Err(TemplateIssue::MissingSummary);
    }
    if !has_key_points {
        return Err(TemplateIssue::MissingKeyPoints);
    }
    Ok(())
}

/// Nettoyage et normalisation de la sortie brute du modèle : bloc de réflexion
/// `<think>…</think>` éventuel, clôture Markdown (accents graves), titres
/// ramenés à leur forme canonique, lignes de séparation retirées, espaces.
/// Idempotent : nettoyer une sortie déjà nettoyée ne change rien.
pub fn clean_output(raw: &str) -> String {
    let mut text = raw.to_string();
    while let Some(start) = text.find("<think>") {
        match text[start..].find("</think>") {
            Some(end) => text.replace_range(start..start + end + "</think>".len(), ""),
            None => text.truncate(start),
        }
    }
    let mut text = text.trim().to_string();
    if text.starts_with("```") {
        if let Some(newline) = text.find('\n') {
            text = text[newline + 1..].to_string();
        } else {
            text.clear();
        }
    }
    text = normalize_lines(&text);
    // Clôture finale, éventuellement démasquée par le retrait d'un séparateur.
    while let Some(stripped) = text.trim_end().strip_suffix("```") {
        text = normalize_lines(stripped);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "# Réunion budget\n\n## Résumé\nPhrase une. Phrase deux. Phrase trois.\n\n## Points clés\n- Budget : 30 000 €\n\n## Décisions et actions\n- Marie envoie le devis lundi\n";

    #[test]
    fn prompts_carry_the_spec_wording_and_the_text() {
        let direct = build_direct_messages("Bonjour.");
        assert_eq!(direct.system, SYSTEM_PROMPT);
        assert!(direct.user.starts_with(TEMPLATE_INSTRUCTIONS));
        assert!(direct
            .user
            .starts_with("Rédige un seul compte-rendu global en Markdown"));
        assert!(direct.user.ends_with("Transcription : Bonjour."));

        let notes = build_notes_messages("texte", 2, 3);
        assert_eq!(notes.system, SYSTEM_PROMPT);
        assert!(notes
            .user
            .starts_with("Voici la partie 2 sur 3 de la transcription d'un enregistrement."));
        assert!(notes.user.contains("sans résumer à outrance"));
        assert!(notes.user.ends_with("Transcription : texte"));

        let final_ = build_final_messages(&["- a".into(), "- b".into()]);
        assert_eq!(final_.system, SYSTEM_PROMPT);
        assert!(final_
            .user
            .starts_with("Voici les notes prises sur les 2 parties d'un enregistrement."));
        assert!(final_.user.contains("rédige un seul compte-rendu global"));
        assert!(final_.user.contains("Partie 1 :\n- a\n\nPartie 2 :\n- b"));
    }

    #[test]
    fn prompts_forbid_several_reports_separators_and_fences() {
        for constraint in [
            "un seul compte-rendu global",
            "250 à 450 mots",
            "une seule ligne `# `",
            "ne reproduis pas les parties une par une",
            "`---`",
            "bloc de code",
            "n'écris ni la section ni une mention",
        ] {
            assert!(
                TEMPLATE_INSTRUCTIONS.contains(constraint),
                "TEMPLATE_INSTRUCTIONS sans « {constraint} »"
            );
        }
        // Le gabarit décrit par les prompts est celui que `check_template` accepte.
        for section in [SECTION_SUMMARY, SECTION_KEY_POINTS, SECTION_DECISIONS] {
            assert!(TEMPLATE_INSTRUCTIONS.contains(section));
            assert!(STRICT_REMINDER.contains(section));
        }
        assert!(STRICT_REMINDER.contains("une seule ligne `# `"));
        assert!(STRICT_REMINDER.contains("`---`"));
        assert!(CONCISE_REMINDER.contains("coupée car trop longue"));
        assert!(CONCISE_REMINDER.contains("250 à 450 mots"));

        let notes = build_notes_messages("texte", 1, 4);
        assert!(notes.user.contains("25 puces maximum, une ligne par puce"));
        let final_ = build_final_messages(&["- a".into(), "- b".into(), "- c".into()]);
        assert!(final_
            .user
            .contains("un seul compte-rendu global qui synthétise l'ensemble des parties"));
    }

    /// Les écarts constatés en conditions réelles (titres en gras, deux-points,
    /// séparateurs) sont nommés dans les deux prompts qui décrivent le gabarit.
    #[test]
    fn prompts_spell_out_the_exact_section_headings() {
        for constraint in [
            "Les titres de section s'écrivent exactement",
            "sans gras",
            "sans deux-points",
            "sans ligne de séparation",
        ] {
            assert!(
                TEMPLATE_INSTRUCTIONS.contains(constraint),
                "TEMPLATE_INSTRUCTIONS sans « {constraint} »"
            );
            assert!(
                STRICT_REMINDER.contains(constraint),
                "STRICT_REMINDER sans « {constraint} »"
            );
        }
    }

    #[test]
    fn template_check_accepts_the_expected_structure() {
        assert_eq!(check_template(GOOD), Ok(()));
        let without_decisions = "# Titre\n## Résumé\nx\n## Points clés\n- y\n";
        assert_eq!(check_template(without_decisions), Ok(()));
        let with_subheadings =
            "# Titre\n## Résumé\nx\n## Points clés\n### Budget\n- y\n### Délais\n- z\n";
        assert_eq!(check_template(with_subheadings), Ok(()));
    }

    #[test]
    fn template_check_rejects_missing_or_extra_sections() {
        assert_eq!(
            check_template("## Résumé\nx\n## Points clés\n- y"),
            Err(TemplateIssue::MissingTitle)
        );
        assert_eq!(
            check_template("# T\n## Points clés\n- y"),
            Err(TemplateIssue::MissingSummary)
        );
        assert_eq!(
            check_template("# T\n## Résumé\nx"),
            Err(TemplateIssue::MissingKeyPoints)
        );
        assert_eq!(
            check_template("# T\n## Résumé\nx\n## Points clés\n- y\n## Conclusion\nz"),
            Err(TemplateIssue::UnexpectedSection("## Conclusion".into()))
        );
    }

    #[test]
    fn template_check_rejects_several_documents_and_separators() {
        let two_documents =
            format!("{GOOD}\n---\n\n# Deuxième réunion\n\n## Résumé\nx\n\n## Points clés\n- y\n");
        assert_eq!(
            check_template(&two_documents),
            Err(TemplateIssue::Separator)
        );

        let two_titles = "# Partie 1\n## Résumé\nx\n## Points clés\n- y\n\n# Partie 2\n";
        assert_eq!(
            check_template(two_titles),
            Err(TemplateIssue::MultipleTitles(2))
        );

        for separator in ["---", "***", "___", "  ---  ", "----", "- - -"] {
            let markdown = format!("# T\n## Résumé\nx\n{separator}\n## Points clés\n- y\n");
            assert_eq!(check_template(&markdown), Err(TemplateIssue::Separator));
        }
    }

    /// Plusieurs comptes-rendus collés restent refusés une fois les
    /// séparateurs retirés : c'est le nombre de titres qui les trahit.
    #[test]
    fn cleaning_never_turns_two_documents_into_one() {
        let two_documents = format!(
            "{GOOD}\n---\n\n# **Deuxième réunion**\n\n## **Résumé**\nx\n\n## Points clés\n- y\n"
        );
        let cleaned = clean_output(&two_documents);
        assert!(!cleaned.contains("---"), "{cleaned}");
        assert_eq!(
            check_template(&cleaned),
            Err(TemplateIssue::MultipleTitles(2))
        );
    }

    #[test]
    fn output_cleaning_strips_fences_thoughts_and_trailing_separator() {
        let raw = "<think>je réfléchis</think>\n```markdown\n# Titre\n## Résumé\nx\n## Points clés\n- y\n```\n";
        let cleaned = clean_output(raw);
        assert!(cleaned.starts_with("# Titre"));
        assert!(cleaned.ends_with("- y"));
        assert_eq!(check_template(&cleaned), Ok(()));
        assert_eq!(clean_output("  # A  "), "# A");
        assert_eq!(clean_output("<think>sans fin"), "");
        assert_eq!(
            clean_output("# T\n## Résumé\nx\n## Points clés\n- y\n\n---\n"),
            "# T\n## Résumé\nx\n## Points clés\n- y"
        );
        assert_eq!(clean_output("# T\n- y\n```\n---\n"), "# T\n- y");
    }

    /// Sortie réelle de Ministral relevée à la tâche 13 : titres en gras,
    /// deux-points, lignes `---` entre les sections.
    #[test]
    fn output_cleaning_normalizes_the_real_model_output() {
        let raw = "# **Création d'une landing page interactive**\n\n## **Résumé**\nPhrase une. Phrase deux. Phrase trois.\n\n---\n\n## **Points clés**\n- Budget : 30 000 €\n\n---\n\n## **Décisions et actions** :\n- Marie envoie le devis lundi\n";
        let cleaned = clean_output(raw);
        assert_eq!(
            cleaned,
            "# Création d'une landing page interactive\n\n## Résumé\nPhrase une. Phrase deux. Phrase trois.\n\n## Points clés\n- Budget : 30 000 €\n\n## Décisions et actions\n- Marie envoie le devis lundi"
        );
        assert_eq!(check_template(&cleaned), Ok(()));
        assert_eq!(clean_output(&cleaned), cleaned, "nettoyage idempotent");
    }

    #[test]
    fn section_headings_are_rewritten_in_canonical_form() {
        for raw in [
            "## **Résumé**",
            "## __Résumé__",
            "## *Résumé*",
            "## Résumé :",
            "## **Résumé :**",
            "## RÉSUMÉ",
            "## Resume",
            // « é » composé : `e` + accent aigu combinant.
            "## Re\u{0301}sume\u{0301}",
            "##   résumé   ",
            "## Résumé ##",
            "**## Résumé**",
            "## 1. Résumé",
        ] {
            assert_eq!(clean_output(raw), SECTION_SUMMARY, "« {raw} »");
        }
        for raw in [
            "## **Points clés**",
            "## Points-clés :",
            "## points clefs",
            "## Points Clés",
            "## POINTS CLEFS :",
        ] {
            assert_eq!(clean_output(raw), SECTION_KEY_POINTS, "« {raw} »");
        }
        for raw in [
            "## **Décisions et actions**",
            "## Décisions & actions :",
            "## DÉCISIONS ET ACTIONS",
            "## Decisions et actions",
            "## Décisions",
        ] {
            assert_eq!(clean_output(raw), SECTION_DECISIONS, "« {raw} »");
        }
        // Un titre de niveau 1 garde son texte ; seule la mise en forme part.
        assert_eq!(clean_output("# **Réunion budget** :"), "# Réunion budget");
        // Une section inconnue est normalisée, pas acceptée.
        assert_eq!(clean_output("## **Conclusion** :"), "## Conclusion");
        assert_eq!(
            check_template("# T\n## Résumé\nx\n## Points clés\n- y\n## Conclusion\nz"),
            Err(TemplateIssue::UnexpectedSection("## Conclusion".into()))
        );
        // Ni un mot-dièse ni du gras en début de ligne ne deviennent un titre.
        assert_eq!(clean_output("#budget serré"), "#budget serré");
        assert_eq!(
            clean_output("**Points clés** du jour"),
            "**Points clés** du jour"
        );
    }

    #[test]
    fn separators_are_removed_wherever_they_are() {
        for separator in ["---", "***", "___", "----------", "- - -", "  ***  "] {
            let raw = format!(
                "# T\n\n{separator}\n\n## Résumé\nx\n\n{separator}\n\n## Points clés\n- y\n\n{separator}\n"
            );
            let cleaned = clean_output(&raw);
            assert_eq!(
                cleaned, "# T\n\n## Résumé\nx\n\n## Points clés\n- y",
                "« {separator} »"
            );
            assert_eq!(check_template(&cleaned), Ok(()));
        }
        // Ce qui ressemble à un séparateur sans en être un reste intact.
        for kept in ["- point", "--", "*Italique*", "a---b"] {
            assert!(!is_separator(kept), "« {kept} »");
        }
    }
}
