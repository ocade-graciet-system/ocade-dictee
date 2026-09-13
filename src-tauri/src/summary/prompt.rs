//! Gabarit et prompts (spec §6), en constantes. Toute modification du gabarit
//! doit rester cohérente avec `check_template` : ce que les prompts demandent
//! est exactement ce que le contrôle accepte.

pub const SYSTEM_PROMPT: &str = "Tu es un assistant de rédaction professionnel. Tu écris uniquement en français, dans un style clair et neutre. Tu reformules et organises le contenu fourni sans rien inventer : aucune information absente de la transcription, aucun commentaire, aucune introduction ni conclusion hors du gabarit demandé.";

/// Structure imposée du compte-rendu, commune au prompt direct et au prompt
/// final. Les contraintes « un seul compte-rendu », « un seul titre », « pas de
/// séparateur » et « pas de bloc de code » viennent de l'essai comparatif : le
/// modèle rendait sinon plusieurs comptes-rendus, un par partie, collés par une
/// ligne de séparation.
pub const TEMPLATE_INSTRUCTIONS: &str = "Rédige un seul compte-rendu global en Markdown, de 250 à 450 mots, en respectant exactement cette structure : une seule ligne `# ` suivie d'un titre court ; `## Résumé` : 3 à 6 phrases ; `## Points clés` : liste à puces des informations importantes (faits, chiffres, noms, dates) ; `## Décisions et actions` : liste à puces « qui, quoi, quand » — s'il n'y a ni décision ni action, n'écris ni la section ni une mention de son absence. N'ajoute aucune autre section, ne reproduis pas les parties une par une, n'écris aucune ligne de séparation `---` et n'utilise aucun bloc de code (pas de ```).";

/// Rappel ajouté au prompt lors de l'unique relance après un gabarit incomplet.
pub const STRICT_REMINDER: &str = "RAPPEL STRICT : la réponse précédente ne respectait pas la structure demandée. Réponds uniquement avec le compte-rendu, exactement dans cette structure et sans aucune autre section ni commentaire : une seule ligne `# ` avec un titre court, puis `## Résumé`, puis `## Points clés`, puis (seulement s'il y a des décisions ou des actions) `## Décisions et actions`. N'écris aucune ligne de séparation `---` et n'utilise aucun bloc de code.";

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
    Separator,
}

/// Ligne de séparation Markdown laissée seule sur sa ligne.
fn is_separator(line: &str) -> bool {
    matches!(line.trim(), "---" | "***" | "___")
}

/// Contrôle du gabarit (§6) : exactement une ligne `# `, `## Résumé`,
/// `## Points clés` ; `## Décisions et actions` facultatif ; toute autre ligne
/// `## ` et toute ligne de séparation sont refusées. Les sous-titres `### `
/// à l'intérieur d'une section restent acceptés.
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

/// Retire un séparateur laissé seul en dernière ligne.
fn strip_trailing_separator(text: &str) -> &str {
    let text = text.trim();
    match text.rsplit_once('\n') {
        Some((head, last)) if is_separator(last) => head.trim_end(),
        Some(_) => text,
        None if is_separator(text) => "",
        None => text,
    }
}

/// Nettoyage de la sortie brute du modèle : bloc de réflexion `<think>…</think>`
/// éventuel, clôture Markdown (accents graves), séparateur final, espaces.
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
    // Clôture et séparateur finaux, dans n'importe quel ordre.
    loop {
        let before = text.len();
        if let Some(stripped) = text.trim_end().strip_suffix("```") {
            text = stripped.to_string();
        }
        text = strip_trailing_separator(&text).to_string();
        if text.len() == before {
            return text;
        }
    }
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

        for separator in ["---", "***", "___", "  ---  "] {
            let markdown = format!("# T\n## Résumé\nx\n{separator}\n## Points clés\n- y\n");
            assert_eq!(check_template(&markdown), Err(TemplateIssue::Separator));
        }
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
}
