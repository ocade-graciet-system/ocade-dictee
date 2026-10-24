//! Éléments du compte rendu de réunion : charte éditoriale et skill (textes
//! importés d'un fichier ou collés), charte graphique (couleurs, police, pied
//! de page, logo). Ils sont seulement conservés ici ; le compte rendu les
//! appliquera.

use std::io::Read;
use std::path::Path;

use super::store::MeetingStore;
use super::{GraphicCharter, MeetingAssets, MeetingError, TextAsset, TextAssetKind};

/// Fichier importé : 20 Mo au plus (un Word illustré dépasse vite quelques Mo).
pub const MAX_IMPORT_BYTES: u64 = 20 * 1024 * 1024;
/// Texte conservé : 300 000 caractères au plus (~50 000 mots).
pub const MAX_TEXT_CHARS: usize = 300_000;
/// Logo : 5 Mo au plus.
pub const MAX_LOGO_BYTES: u64 = 5 * 1024 * 1024;

pub const TEXT_EXTENSIONS: &[&str] = &["md", "markdown", "txt", "docx"];
pub const SKILL_EXTENSIONS: &[&str] = &["md", "markdown", "txt", "skill", "zip"];
pub const LOGO_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "svg", "gif"];

const GRAPHIC_KIND: &str = "graphic";

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn check_size(path: &Path, max_bytes: u64) -> Result<(), MeetingError> {
    let len = std::fs::metadata(path)
        .map_err(|e| MeetingError::InvalidFile {
            detail: e.to_string(),
        })?
        .len();
    if len > max_bytes {
        return Err(MeetingError::FileTooLarge { max_bytes });
    }
    Ok(())
}

/// Octets → texte : UTF-8 (BOM retiré), sinon Windows-1252 approché par
/// Latin-1, l'encodage des vieux `.txt` de Windows.
fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

fn invalid(detail: impl std::fmt::Display) -> MeetingError {
    MeetingError::InvalidFile {
        detail: detail.to_string(),
    }
}

/// Texte d'un document Word : paragraphes de `word/document.xml`, tabulations
/// et retours à la ligne conservés, mise en forme ignorée.
pub fn docx_to_text(bytes: &[u8]) -> Result<String, MeetingError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(invalid)?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(invalid)?
        .read_to_string(&mut xml)
        .map_err(invalid)?;
    Ok(word_xml_to_text(&xml))
}

fn word_xml_to_text(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let tag = &rest[start + 1..start + end];
        let after = &rest[start + end + 1..];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default();
        match name {
            "w:t" if !tag.starts_with('/') && !tag.ends_with('/') => {
                let close = after.find("</w:t>").unwrap_or(after.len());
                out.push_str(&unescape_xml(&after[..close]));
                rest = &after[close..];
                continue;
            }
            "w:tab" => out.push('\t'),
            "w:br" | "w:cr" => out.push('\n'),
            // Fin de paragraphe, ou paragraphe vide `<w:p/>`.
            "w:p" if tag.starts_with('/') || tag.ends_with('/') => out.push('\n'),
            _ => {}
        }
        rest = after;
    }
    // Lignes vides répétées ramenées à une seule.
    let mut text = String::new();
    let mut blank = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text.trim().to_string()
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// `SKILL.md` d'une archive de skill (`.skill` ou `.zip`) : celui le moins
/// profond, à défaut le premier `.md`.
fn skill_from_archive(bytes: &[u8]) -> Result<String, MeetingError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(invalid)?;
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    let depth = |name: &str| name.matches('/').count();
    let pick = names
        .iter()
        .filter(|n| {
            n.rsplit('/')
                .next()
                .is_some_and(|f| f.eq_ignore_ascii_case("SKILL.md"))
        })
        .min_by_key(|n| depth(n))
        .or_else(|| {
            names
                .iter()
                .filter(|n| n.to_ascii_lowercase().ends_with(".md"))
                .min_by_key(|n| depth(n))
        })
        .cloned()
        .ok_or_else(|| invalid("aucun fichier SKILL.md dans l'archive"))?;
    let mut content = Vec::new();
    archive
        .by_name(&pick)
        .map_err(invalid)?
        .read_to_end(&mut content)
        .map_err(invalid)?;
    Ok(decode_text(&content))
}

/// Lit le texte d'un fichier importé (charte éditoriale ou skill).
pub fn read_text_file(path: &Path, kind: TextAssetKind) -> Result<String, MeetingError> {
    let ext = extension(path);
    let allowed = match kind {
        TextAssetKind::Editorial => TEXT_EXTENSIONS,
        TextAssetKind::Skill => SKILL_EXTENSIONS,
    };
    if !allowed.contains(&ext.as_str()) {
        return Err(MeetingError::UnsupportedFile);
    }
    check_size(path, MAX_IMPORT_BYTES)?;
    let bytes = std::fs::read(path).map_err(invalid)?;
    match ext.as_str() {
        "docx" => docx_to_text(&bytes),
        "skill" | "zip" => skill_from_archive(&bytes),
        _ => Ok(decode_text(&bytes)),
    }
}

/// Texte nettoyé et borné, ou erreur s'il est vide ou trop long.
pub fn validate_text(text: &str) -> Result<String, MeetingError> {
    let text = text.replace("\r\n", "\n").trim().to_string();
    if text.is_empty() {
        return Err(invalid("le texte est vide"));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(MeetingError::FileTooLarge {
            max_bytes: MAX_TEXT_CHARS as u64,
        });
    }
    Ok(text)
}

/// `name` et `description` de l'en-tête YAML d'un skill (`---` … `---`).
pub fn skill_front_matter(content: &str) -> (Option<String>, Option<String>) {
    let Some(rest) = content.trim_start().strip_prefix("---") else {
        return (None, None);
    };
    let Some(end) = rest.find("\n---") else {
        return (None, None);
    };
    let mut name = None;
    let mut description = None;
    for line in rest[..end].lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            _ => {}
        }
    }
    (name, description)
}

fn to_text_asset(kind: TextAssetKind, stored: super::store::StoredAsset) -> TextAsset {
    let (title, description) = match kind {
        TextAssetKind::Skill => skill_front_matter(&stored.content),
        TextAssetKind::Editorial => (None, None),
    };
    TextAsset {
        words: stored.content.split_whitespace().count() as i64,
        file_name: stored.file_name,
        content: stored.content,
        updated_at: stored.updated_at,
        title,
        description,
    }
}

/// Charte graphique validée : couleurs `#RRGGBB`, police et pied de page bornés.
pub fn validate_graphic(charter: &GraphicCharter) -> Result<GraphicCharter, MeetingError> {
    let color = |value: &str| -> Result<String, MeetingError> {
        let value = value.trim();
        let ok = value.len() == 7
            && value.starts_with('#')
            && value[1..].chars().all(|c| c.is_ascii_hexdigit());
        if ok {
            Ok(value.to_ascii_uppercase())
        } else {
            Err(MeetingError::InvalidValue)
        }
    };
    let font = charter.font.trim();
    let footer = charter.footer.trim();
    if font.is_empty() || font.chars().count() > 80 || footer.chars().count() > 300 {
        return Err(MeetingError::InvalidValue);
    }
    Ok(GraphicCharter {
        primary_color: color(&charter.primary_color)?,
        secondary_color: color(&charter.secondary_color)?,
        font: font.to_string(),
        footer: footer.to_string(),
    })
}

/// Logo actuel (un seul fichier `logo-*.ext` dans le dossier de la charte).
fn current_logo(store: &MeetingStore) -> Option<std::path::PathBuf> {
    std::fs::read_dir(store.charter_dir())
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .find(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("logo-"))
        })
}

fn remove_logos(store: &MeetingStore) -> Result<(), MeetingError> {
    while let Some(path) = current_logo(store) {
        std::fs::remove_file(&path).map_err(MeetingError::storage)?;
    }
    Ok(())
}

/// Copie le logo dans les données de l'application. Le nom porte l'heure
/// d'import : l'aperçu ne ressert jamais l'ancienne image en cache.
pub fn import_logo(store: &MeetingStore, source: &Path) -> Result<(), MeetingError> {
    let ext = extension(source);
    if !LOGO_EXTENSIONS.contains(&ext.as_str()) {
        return Err(MeetingError::UnsupportedFile);
    }
    check_size(source, MAX_LOGO_BYTES)?;
    let target = store
        .charter_dir()
        .join(format!("logo-{}.{ext}", super::now_unix()));
    let bytes = std::fs::read(source).map_err(invalid)?;
    remove_logos(store)?;
    std::fs::write(&target, bytes).map_err(MeetingError::storage)?;
    Ok(())
}

pub fn clear_logo(store: &MeetingStore) -> Result<(), MeetingError> {
    remove_logos(store)
}

pub fn save_graphic(store: &MeetingStore, charter: &GraphicCharter) -> Result<(), MeetingError> {
    let charter = validate_graphic(charter)?;
    let json = serde_json::to_string(&charter).map_err(MeetingError::storage)?;
    store.set_asset(GRAPHIC_KIND, None, &json)
}

pub fn clear_graphic(store: &MeetingStore) -> Result<(), MeetingError> {
    store.clear_asset(GRAPHIC_KIND)?;
    remove_logos(store)
}

pub fn load(store: &MeetingStore) -> Result<MeetingAssets, MeetingError> {
    let text = |kind: TextAssetKind| -> Result<Option<TextAsset>, MeetingError> {
        Ok(store
            .get_asset(kind.as_db())?
            .map(|stored| to_text_asset(kind, stored)))
    };
    let graphic_row = store.get_asset(GRAPHIC_KIND)?;
    let graphic_updated_at = graphic_row.as_ref().map(|row| row.updated_at);
    let graphic = graphic_row.and_then(|row| serde_json::from_str(&row.content).ok());
    Ok(MeetingAssets {
        editorial: text(TextAssetKind::Editorial)?,
        skill: text(TextAssetKind::Skill)?,
        graphic,
        logo_path: current_logo(store).map(|p| p.to_string_lossy().into_owned()),
        graphic_updated_at,
    })
}

pub fn import_text(
    store: &MeetingStore,
    kind: TextAssetKind,
    path: &Path,
) -> Result<(), MeetingError> {
    let text = validate_text(&read_text_file(path, kind)?)?;
    store.set_asset(kind.as_db(), Some(&file_name(path)), &text)
}

pub fn set_text(store: &MeetingStore, kind: TextAssetKind, text: &str) -> Result<(), MeetingError> {
    let text = validate_text(text)?;
    store.set_asset(kind.as_db(), None, &text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            for (name, content) in entries {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        buffer.into_inner()
    }

    #[test]
    fn word_documents_keep_paragraphs_tabs_and_entities() {
        let xml = r#"<w:document><w:body>
            <w:p><w:r><w:t>Ton :</w:t></w:r><w:r><w:tab/><w:t xml:space="preserve">vouvoiement &amp; clarté</w:t></w:r></w:p>
            <w:p/><w:p/><w:p/>
            <w:p><w:r><w:t>Mots &lt;interdits&gt;</w:t><w:br/><w:t>« deadline »</w:t></w:r></w:p>
        </w:body></w:document>"#;
        let docx = zip_with(&[("word/document.xml", xml)]);
        assert_eq!(
            docx_to_text(&docx).unwrap(),
            "Ton :\tvouvoiement & clarté\n\nMots <interdits>\n« deadline »"
        );
    }

    #[test]
    fn skill_archives_use_the_shallowest_skill_md() {
        let archive = zip_with(&[
            ("cr/references/SKILL.md", "profond"),
            ("cr/SKILL.md", "---\nname: compte-rendu\n---\nRacine"),
            ("cr/README.md", "lisez-moi"),
        ]);
        assert!(skill_from_archive(&archive).unwrap().ends_with("Racine"));
        let without = zip_with(&[("notes.txt", "x")]);
        assert!(skill_from_archive(&without).is_err());
    }

    #[test]
    fn skill_front_matter_is_read_when_present() {
        let content =
            "---\nname: compte-rendu-reunion\ndescription: \"Structure du CR\"\n---\n# Corps";
        assert_eq!(
            skill_front_matter(content),
            (
                Some("compte-rendu-reunion".into()),
                Some("Structure du CR".into())
            )
        );
        assert_eq!(skill_front_matter("# Sans en-tête"), (None, None));
    }

    #[test]
    fn text_is_decoded_from_utf8_with_bom_or_latin1() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFcharte"), "charte");
        assert_eq!(decode_text(b"r\xE9union"), "réunion");
    }

    #[test]
    fn graphic_charter_is_validated() {
        let charter = GraphicCharter {
            primary_color: " #fe414c ".into(),
            secondary_color: "#385623".into(),
            font: " Poppins ".into(),
            footer: "".into(),
        };
        let valid = validate_graphic(&charter).unwrap();
        assert_eq!(valid.primary_color, "#FE414C");
        assert_eq!(valid.font, "Poppins");
        for bad in ["red", "#12345", "#GGGGGG"] {
            let mut wrong = charter.clone();
            wrong.secondary_color = bad.into();
            assert_eq!(validate_graphic(&wrong), Err(MeetingError::InvalidValue));
        }
    }

    #[test]
    fn imports_reject_unknown_extensions_and_empty_text() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("charte.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        assert_eq!(
            read_text_file(&pdf, TextAssetKind::Editorial),
            Err(MeetingError::UnsupportedFile)
        );
        assert!(validate_text("  \n ").is_err());
    }

    #[test]
    fn assets_round_trip_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = MeetingStore::new_at(dir.path()).unwrap();
        let md = dir.path().join("SKILL.md");
        std::fs::write(&md, "---\nname: cr\n---\nSections : Décisions.").unwrap();
        import_text(&store, TextAssetKind::Skill, &md).unwrap();
        set_text(&store, TextAssetKind::Editorial, "Phrases courtes.").unwrap();
        let logo = dir.path().join("logo.png");
        std::fs::write(&logo, b"png").unwrap();
        import_logo(&store, &logo).unwrap();
        import_logo(&store, &logo).unwrap();
        save_graphic(
            &store,
            &GraphicCharter {
                primary_color: "#112233".into(),
                secondary_color: "#445566".into(),
                font: "Arial".into(),
                footer: "Confidentiel".into(),
            },
        )
        .unwrap();

        let assets = load(&store).unwrap();
        let skill = assets.skill.unwrap();
        assert_eq!(skill.file_name.as_deref(), Some("SKILL.md"));
        assert_eq!(skill.title.as_deref(), Some("cr"));
        assert_eq!(assets.editorial.unwrap().words, 2);
        assert_eq!(assets.graphic.unwrap().footer, "Confidentiel");
        assert!(assets.logo_path.is_some());
        // Un seul logo conservé, même après deux imports.
        assert_eq!(std::fs::read_dir(store.charter_dir()).unwrap().count(), 1);

        clear_graphic(&store).unwrap();
        let assets = load(&store).unwrap();
        assert!(assets.graphic.is_none() && assets.logo_path.is_none());
    }
}
