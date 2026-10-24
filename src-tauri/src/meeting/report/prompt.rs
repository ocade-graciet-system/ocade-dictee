//! Prompts du compte rendu de réunion, budget de contexte et contrôle de la
//! structure demandée par le skill. Les formulations viennent du banc d'essai
//! sur Ministral 3 3B (`docs/compte-rendu-reunion-faisabilite.md`) : charte
//! dans le message système, skill avant la transcription, date de
//! l'enregistrement partout (sans elle, le modèle invente l'année), rappel
//! après la transcription.
//!
//! Le nettoyage ne réutilise pas `summary::prompt::clean_output` : celui-ci
//! ramène les titres aux trois sections du résumé de l'onglet Fichier
//! (« ## Décisions » y devient « ## Décisions et actions »), alors qu'ici les
//! titres sont ceux du skill.

use chrono::{Datelike, NaiveDate};

use crate::summary::chunking::{estimate_tokens, MAX_CHUNK_TOKENS};
use crate::summary::prompt::{ChatMessages, SYSTEM_PROMPT};

/// Gabarit utilisé quand aucun skill n'a été ajouté : celui du banc d'essai,
/// structure suivie dans 16 comptes rendus sur 16. Les exemples sont à trous :
/// une phrase d'exemple réaliste était recopiée telle quelle par le modèle.
pub const DEFAULT_SKILL: &str = r#"# Compte rendu de réunion

La transcription vient d'une reconnaissance vocale : elle ne contient pas le nom des locuteurs, la ponctuation est approximative et elle contient des hésitations et des redites. Identifiez les participants à partir de ce qu'ils disent (présentations, prénoms cités, « moi je »).

## Structure obligatoire

Le compte rendu suit exactement cette structure, dans cet ordre, sans aucune autre section :

```
# Compte rendu – <objet de la réunion>

**Date :** <date de la réunion>
**Participants :** <Prénom Nom (fonction)>, <Prénom Nom (fonction)>, …

## Ordre du jour
1. <sujet>

## Synthèse
<4 à 6 phrases>

## Décisions
- <décision>

## Plan d'actions
| Qui | Quoi | Échéance |
|---|---|---|
| <Prénom Nom> | <action> | <échéance> |

## Points en suspens
- <point en suspens>

## Prochaine réunion
<date, heure et modalité>
```

## Consignes par section

**En-tête.** Une seule ligne de titre `# `. La ligne « Participants » ne cite que les personnes présentes. Une personne seulement citée (un prestataire, un responsable absent) n'est pas un participant.

**Ordre du jour.** Liste numérotée des sujets abordés, dans l'ordre de la réunion, cinq mots environ par sujet.

**Synthèse.** Quatre à six phrases : objet de la réunion, avancement, principaux arbitrages. Pas de liste.

**Décisions.** Une puce par décision réellement prise pendant la réunion. Une proposition non validée n'est pas une décision. S'il n'y en a aucune, écrivez « Aucune ».

**Plan d'actions.** Un tableau Markdown à exactement trois colonnes : `Qui`, `Quoi`, `Échéance`. Une ligne par action et une seule personne par ligne, celle qui s'est engagée à la faire. Sans échéance donnée, écrivez « À définir ». N'inventez jamais une échéance.

**Points en suspens.** Les sujets reportés ou non tranchés. S'il n'y en a aucun, écrivez « Aucun ».

**Prochaine réunion.** Date, heure et modalité. Si elle n'a pas été fixée, écrivez « Non fixée ».

## Règles générales

- Toutes les sections sont présentes, dans cet ordre, avec ces titres exacts, sans numéro, sans gras et sans deux-points.
- 600 mots au maximum.
- Pas de conclusion, pas de formule de politesse, pas de commentaire sur la transcription.
- Pas de ligne de séparation `---` et pas de bloc de code dans la réponse.
- Rien n'est inventé : chaque nom, date, montant et chiffre figure dans la transcription.
"#;

/// Tokens estimés (`estimate_tokens`) des consignes et rappels qui encadrent
/// le contenu, charte et skill non compris.
pub const ENVELOPE_TOKENS: usize = 600;
/// En dessous, la charte et le skill ne laissent plus assez de place à la
/// transcription ou aux notes : le compte rendu est refusé avant de lancer
/// le moteur (le serveur répondrait sinon par une erreur de contexte).
pub const MIN_CONTENT_TOKENS: usize = 2_500;

/// Skill prêt à l'emploi : instructions sans en-tête YAML, et structure
/// attendue lue dans son gabarit (bloc de code contenant les titres `## `).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub instructions: String,
    /// Titres `## …` exigés, dans l'ordre ; vide si le skill n'expose pas de
    /// gabarit (aucun contrôle de structure alors).
    pub sections: Vec<String>,
    /// Le gabarit commence par un titre `# `.
    pub has_title: bool,
}

/// Retire l'en-tête YAML (`---` … `---`) d'un skill.
pub fn strip_front_matter(text: &str) -> &str {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let after = &rest[end + 4..];
            return after.split_once('\n').map_or("", |(_, body)| body);
        }
    }
    text
}

/// Lit un skill. La structure exigée est celle du premier bloc de code qui
/// contient au moins deux titres `## ` : c'est la forme d'un gabarit. Les
/// titres `## ` du skill lui-même (« Consignes », « Règles ») ne sont pas des
/// sections du compte rendu et ne comptent pas.
pub fn parse_skill(text: &str) -> Skill {
    let instructions = strip_front_matter(text).trim().to_string();
    let mut sections = Vec::new();
    let mut has_title = false;
    let mut block: Option<Vec<&str>> = None;
    for line in instructions.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            match block.take() {
                None => block = Some(Vec::new()),
                Some(lines) => {
                    let headings: Vec<String> = lines
                        .iter()
                        .filter_map(|l| l.trim().strip_prefix("## "))
                        .map(|h| format!("## {}", h.trim()))
                        .collect();
                    if headings.len() >= 2 {
                        has_title = lines.iter().any(|l| l.trim().starts_with("# "));
                        sections = headings;
                        break;
                    }
                }
            }
        } else if let Some(lines) = block.as_mut() {
            lines.push(line);
        }
    }
    Skill {
        instructions,
        sections,
        has_title,
    }
}

const WEEKDAYS: [&str; 7] = [
    "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
];
const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];

/// « jeudi 22 octobre 2026 », « vendredi 1er octobre 2027 ».
pub fn french_date(date: NaiveDate) -> String {
    let day = match date.day() {
        1 => "1er".to_string(),
        day => day.to_string(),
    };
    format!(
        "{} {day} {} {}",
        WEEKDAYS[date.weekday().num_days_from_monday() as usize],
        MONTHS[date.month0() as usize],
        date.year()
    )
}

/// Ce qui encadre la rédaction : date de la réunion, charte, skill.
pub struct ReportContext {
    /// Date de l'enregistrement, en toutes lettres.
    pub date_label: String,
    pub charter: Option<String>,
    pub skill: Skill,
}

impl ReportContext {
    /// Tokens estimés laissés à la transcription (passe directe) ou aux notes
    /// (synthèse finale) une fois la charte, le skill et les consignes
    /// comptés ; `Err` avec ce qu'il manque quand il n'en reste pas assez.
    pub fn content_budget(&self) -> Result<usize, usize> {
        let assets = self.charter.as_deref().map_or(0, estimate_tokens)
            + estimate_tokens(&self.skill.instructions)
            + ENVELOPE_TOKENS;
        let budget = MAX_CHUNK_TOKENS.saturating_sub(assets);
        if budget < MIN_CONTENT_TOKENS {
            Err(MIN_CONTENT_TOKENS - budget)
        } else {
            Ok(budget)
        }
    }

    fn system(&self) -> String {
        match &self.charter {
            Some(charter) => format!(
                "{SYSTEM_PROMPT}\n\nCharte éditoriale de l'entreprise, à respecter strictement dans toute la rédaction (ton, vocabulaire, dates, heures, montants, noms des personnes, ponctuation). Sur la forme, la charte l'emporte sur tes habitudes ; elle ne t'autorise jamais à ajouter une information absente de la transcription.\n\n<charte>\n{}\n</charte>",
                charter.trim()
            ),
            None => SYSTEM_PROMPT.to_string(),
        }
    }

    fn date_line(&self) -> String {
        format!(
            "Date de l'enregistrement : {}. Les dates citées sans année se rapportent à cette année (ou à la suivante pour les mois qui suivent).",
            self.date_label
        )
    }

    fn skill_block(&self) -> String {
        format!(
            "Rédige un compte rendu de réunion en appliquant exactement les instructions du skill ci-dessous (structure, sections, consignes). Elles remplacent tout autre gabarit.\n\n<skill>\n{}\n</skill>",
            self.skill.instructions
        )
    }

    /// Rappel placé après la transcription ou les notes : sur un long prompt,
    /// le modèle perd de vue les consignes de tête.
    pub fn reminder(&self) -> String {
        let mut reminder = String::from(
            "Rappel : réponds uniquement avec le compte rendu, en français, en Markdown, dans la structure exacte décrite par le skill (mêmes sections, même ordre, mêmes titres), sans rien inventer, sans ligne de séparation et sans bloc de code.",
        );
        if !self.skill.sections.is_empty() {
            reminder.push_str(" Titres attendus : ");
            reminder.push_str(&self.skill.sections.join(", "));
            reminder.push('.');
        }
        if self.charter.is_some() {
            reminder.push_str(
                " Applique toutes les règles de la charte éditoriale (vocabulaire, dates, heures, montants, noms des personnes, phrases courtes).",
            );
        }
        reminder.push_str(&format!(" La réunion a eu lieu le {}.", self.date_label));
        reminder
    }

    /// Rappel strict de l'unique relance, quand la structure manque.
    pub fn strict_reminder(&self) -> String {
        let mut text = String::from(
            "RAPPEL STRICT : la réponse précédente ne respectait pas la structure demandée par le skill. Réponds uniquement avec le compte rendu, exactement dans la structure du skill",
        );
        if self.skill.has_title {
            text.push_str(" : une seule ligne `# `");
        }
        if !self.skill.sections.is_empty() {
            text.push_str(", puis les sections ");
            text.push_str(&self.skill.sections.join(", "));
            text.push_str(", toutes présentes, dans cet ordre, avec ces titres exacts, sans gras ni deux-points");
        }
        text.push_str(
            ". N'écris aucune ligne de séparation `---` et n'utilise aucun bloc de code.",
        );
        text
    }

    /// Compte rendu en une passe : la transcription tient dans le budget.
    pub fn direct_messages(&self, transcript: &str) -> ChatMessages {
        ChatMessages {
            system: self.system(),
            user: format!(
                "{}\n\n{}\n\nTranscription brute de la réunion :\n{}\n\n{}",
                self.date_line(),
                self.skill_block(),
                transcript.trim(),
                self.reminder()
            ),
        }
    }

    /// Notes d'une partie d'une longue réunion. Neutres : ni charte ni skill,
    /// qui ne servent qu'à la synthèse finale (sur des notes, le modèle
    /// applique mieux le lexique de la charte que sur la transcription brute).
    pub fn notes_messages(&self, part: &str, index: u32, total: u32) -> ChatMessages {
        ChatMessages {
            system: SYSTEM_PROMPT.to_string(),
            user: format!(
                "{} Voici la partie {index} sur {total} de la transcription brute d'une réunion. Rédige des notes détaillées en puces, dans l'ordre : faits, chiffres, noms, dates, décisions, et pour chaque action qui s'engage à la faire et pour quand. Ne rien inventer, ne pas résumer à outrance. 25 puces au maximum, une ligne par puce, sans titres ni lignes de séparation.\n\nTranscription :\n{}",
                self.date_line(),
                part.trim()
            ),
        }
    }

    /// Compte rendu final à partir des notes de toutes les parties.
    pub fn final_messages(&self, notes: &[String]) -> ChatMessages {
        let joined = notes
            .iter()
            .enumerate()
            .map(|(i, n)| format!("Partie {} :\n{}", i + 1, n.trim()))
            .collect::<Vec<_>>()
            .join("\n\n");
        ChatMessages {
            system: self.system(),
            user: format!(
                "{}\n\n{}\n\nVoici les notes prises sur les {} parties de la réunion. À partir de ces notes uniquement, rédige un seul compte rendu global qui synthétise l'ensemble, et non une section par partie.\n\nNotes :\n{joined}\n\n{}",
                self.date_line(),
                self.skill_block(),
                notes.len(),
                self.reminder()
            ),
        }
    }
}

/// Rappel de concision, quand la sortie a été coupée au plafond de tokens.
pub const CONCISE_REMINDER: &str = "La réponse précédente a été coupée car trop longue. Réponds avec la même structure, de façon plus concise, sans rien ajouter d'autre.";

/// Minuscules, accents retirés, ponctuation ramenée à des espaces : clé de
/// comparaison des titres (« Plan d’actions : » = « plan d'actions »).
fn heading_key(text: &str) -> String {
    let folded: String = text
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        })
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Retire les marqueurs d'emphase qui encadrent un texte (`**Titre**`).
fn strip_emphasis(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim();
        let inner = ["**", "__", "*", "_"].into_iter().find_map(|m| {
            let inner = trimmed.strip_prefix(m)?.strip_suffix(m)?.trim();
            (!inner.is_empty()).then_some(inner)
        });
        match inner {
            Some(inner) => text = inner,
            None => return trimmed,
        }
    }
}

fn is_separator(line: &str) -> bool {
    let line = line.trim();
    let Some(marker) = line.chars().next().filter(|c| matches!(c, '-' | '*' | '_')) else {
        return false;
    };
    line.chars().filter(|c| *c == marker).count() >= 3
        && line.chars().all(|c| c == marker || c.is_whitespace())
}

/// Titre normalisé (emphase, deux-points et dièses de fin retirés), ramené au
/// titre exact du skill quand il y correspond.
fn normalize_heading(line: &str, sections: &[String]) -> Option<String> {
    let line = strip_emphasis(line);
    let level = line.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line[level..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let text = strip_emphasis(rest.trim().trim_end_matches('#'))
        .trim_end_matches(|c: char| c == ':' || c.is_whitespace());
    let text = strip_emphasis(text);
    if level == 2 {
        let key = heading_key(text);
        if let Some(section) = sections.iter().find(|s| heading_key(&s[3..]) == key) {
            return Some(section.clone());
        }
    }
    Some(format!("{} {text}", "#".repeat(level)))
}

/// Nettoie la sortie du modèle : bloc de réflexion, lignes de bloc de code et
/// de séparation retirés, titres normalisés sur ceux du skill, préambule avant
/// le titre supprimé, lignes vides repliées. Idempotent.
pub fn clean_report(raw: &str, skill: &Skill) -> String {
    let mut text = raw.to_string();
    while let Some(start) = text.find("<think>") {
        match text[start..].find("</think>") {
            Some(end) => text.replace_range(start..start + end + "</think>".len(), ""),
            None => text.truncate(start),
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for line in text.trim().lines() {
        if is_separator(line) || line.trim().starts_with("```") {
            continue;
        }
        let line =
            normalize_heading(line, &skill.sections).unwrap_or_else(|| line.trim_end().to_string());
        if line.is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    // Préambule (« Voici le compte rendu : ») avant le titre.
    if skill.has_title {
        if let Some(index) = lines.iter().position(|l| l.starts_with("# ")) {
            lines.drain(..index);
        }
    }
    lines.join("\n")
}

/// Écart à la structure du skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureIssue {
    Empty,
    MissingTitle,
    MultipleTitles(usize),
    MissingSection(String),
    OutOfOrder(String),
    UnexpectedSection(String),
}

/// Contrôle sur une sortie nettoyée : un seul titre `# ` si le gabarit en a
/// un, toutes les sections du skill dans l'ordre, aucune autre section `## `.
/// Un skill sans gabarit n'impose rien d'autre qu'un texte non vide.
pub fn check_structure(markdown: &str, skill: &Skill) -> Result<(), StructureIssue> {
    if markdown.trim().is_empty() {
        return Err(StructureIssue::Empty);
    }
    if skill.sections.is_empty() {
        return Ok(());
    }
    let titles = markdown
        .lines()
        .filter(|l| l.strip_prefix("# ").is_some_and(|t| !t.trim().is_empty()))
        .count();
    if skill.has_title {
        match titles {
            0 => return Err(StructureIssue::MissingTitle),
            1 => {}
            several => return Err(StructureIssue::MultipleTitles(several)),
        }
    }
    let found: Vec<&str> = markdown
        .lines()
        .filter(|l| l.starts_with("## "))
        .map(str::trim_end)
        .collect();
    if let Some(unexpected) = found
        .iter()
        .find(|h| !skill.sections.iter().any(|s| s == *h))
    {
        return Err(StructureIssue::UnexpectedSection(unexpected.to_string()));
    }
    for section in &skill.sections {
        if !found.contains(&section.as_str()) {
            return Err(StructureIssue::MissingSection(section.clone()));
        }
    }
    let order: Vec<usize> = found
        .iter()
        .filter_map(|h| skill.sections.iter().position(|s| s == h))
        .collect();
    if let Some(pair) = order.windows(2).find(|w| w[0] >= w[1]) {
        return Err(StructureIssue::OutOfOrder(skill.sections[pair[1]].clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(charter: Option<&str>) -> ReportContext {
        ReportContext {
            date_label: french_date(NaiveDate::from_ymd_opt(2026, 10, 22).unwrap()),
            charter: charter.map(str::to_string),
            skill: parse_skill(DEFAULT_SKILL),
        }
    }

    #[test]
    fn the_default_skill_exposes_its_template() {
        let skill = parse_skill(DEFAULT_SKILL);
        assert!(skill.has_title);
        assert_eq!(
            skill.sections,
            vec![
                "## Ordre du jour",
                "## Synthèse",
                "## Décisions",
                "## Plan d'actions",
                "## Points en suspens",
                "## Prochaine réunion",
            ]
        );
    }

    #[test]
    fn skill_headings_outside_a_template_are_not_sections() {
        let skill = parse_skill(
            "---\nname: cr\ndescription: x\n---\n# Skill\n\n## Consignes\nSoyez bref.\n\n## Règles\n- Rien.",
        );
        assert!(skill.sections.is_empty());
        assert!(skill.instructions.starts_with("# Skill"));
        assert!(!skill.instructions.contains("name: cr"));
    }

    #[test]
    fn dates_are_written_in_french_with_the_first_of_the_month() {
        assert_eq!(
            french_date(NaiveDate::from_ymd_opt(2026, 10, 22).unwrap()),
            "jeudi 22 octobre 2026"
        );
        assert_eq!(
            french_date(NaiveDate::from_ymd_opt(2027, 10, 1).unwrap()),
            "vendredi 1er octobre 2027"
        );
    }

    #[test]
    fn the_charter_goes_to_the_system_message_and_the_date_everywhere() {
        let ctx = context(Some("Vouvoyez."));
        let direct = ctx.direct_messages("Bonjour à tous.");
        assert!(direct.system.starts_with(SYSTEM_PROMPT));
        assert!(direct.system.contains("<charte>\nVouvoyez.\n</charte>"));
        assert!(direct
            .user
            .starts_with("Date de l'enregistrement : jeudi 22 octobre 2026."));
        assert!(direct.user.contains("<skill>"));
        assert!(direct
            .user
            .contains("Transcription brute de la réunion :\nBonjour à tous."));
        assert!(direct
            .user
            .ends_with("La réunion a eu lieu le jeudi 22 octobre 2026."));
        assert!(direct.user.contains("## Plan d'actions"));

        // Les notes restent neutres : ni charte, ni skill, mais la date.
        let notes = ctx.notes_messages("texte", 1, 3);
        assert_eq!(notes.system, SYSTEM_PROMPT);
        assert!(!notes.user.contains("<skill>"));
        assert!(notes.user.contains("jeudi 22 octobre 2026"));
        assert!(notes.user.contains("partie 1 sur 3"));

        let final_messages = ctx.final_messages(&["- a".into(), "- b".into()]);
        assert!(final_messages.system.contains("<charte>"));
        assert!(final_messages.user.contains("Partie 2 :\n- b"));
    }

    #[test]
    fn a_long_charter_shrinks_the_budget_and_an_oversized_one_is_refused() {
        let without = context(None).content_budget().unwrap();
        let with = context(Some(&"Règle de style. ".repeat(300)))
            .content_budget()
            .unwrap();
        assert!(with < without);
        assert!(context(Some(&"Règle de style. ".repeat(2_000)))
            .content_budget()
            .is_err());
    }

    #[test]
    fn cleaning_maps_headings_onto_the_skill_and_drops_the_preamble() {
        let skill = parse_skill(DEFAULT_SKILL);
        let raw = "Voici le compte rendu :\n\n```markdown\n# Compte rendu – Site\n\n**Date :** jeudi\n\n## **Ordre du jour :**\n1. Site\n---\n## SYNTHESE\nTexte.\n## Décisions\n- Aucune\n## Plan d’actions\n| Qui | Quoi | Échéance |\n|---|---|---|\n| A | B | C |\n## Points en suspens\n- Aucun\n## Prochaine réunion :\nNon fixée\n```";
        let clean = clean_report(raw, &skill);
        assert!(clean.starts_with("# Compte rendu – Site"));
        assert!(clean.contains("\n## Ordre du jour\n"));
        assert!(clean.contains("\n## Synthèse\n"));
        assert!(clean.contains("\n## Plan d'actions\n| Qui | Quoi | Échéance |\n|---|---|---|"));
        assert!(!clean.contains("```") && !clean.contains("\n---\n"));
        assert_eq!(check_structure(&clean, &skill), Ok(()));
        assert_eq!(clean_report(&clean, &skill), clean, "idempotent");
    }

    #[test]
    fn structure_problems_are_reported() {
        let skill = parse_skill(DEFAULT_SKILL);
        assert_eq!(check_structure("  ", &skill), Err(StructureIssue::Empty));
        assert_eq!(
            check_structure("## Ordre du jour\nx", &skill),
            Err(StructureIssue::MissingTitle)
        );
        let missing = "# T\n## Ordre du jour\n## Synthèse\n## Décisions\n## Plan d'actions\n## Points en suspens";
        assert_eq!(
            check_structure(missing, &skill),
            Err(StructureIssue::MissingSection(
                "## Prochaine réunion".into()
            ))
        );
        let swapped = "# T\n## Synthèse\n## Ordre du jour\n## Décisions\n## Plan d'actions\n## Points en suspens\n## Prochaine réunion";
        assert!(matches!(
            check_structure(swapped, &skill),
            Err(StructureIssue::OutOfOrder(_))
        ));
        let extra = "# T\n## Ordre du jour\n## Synthèse\n## Conclusion\n";
        assert_eq!(
            check_structure(extra, &skill),
            Err(StructureIssue::UnexpectedSection("## Conclusion".into()))
        );
    }

    #[test]
    fn reminders_name_the_expected_headings() {
        let ctx = context(Some("x"));
        assert!(ctx.reminder().contains("## Ordre du jour, ## Synthèse"));
        assert!(ctx.reminder().contains("charte éditoriale"));
        assert!(ctx.strict_reminder().contains("une seule ligne `# `"));
        assert!(!context(None).reminder().contains("charte"));
    }
}
