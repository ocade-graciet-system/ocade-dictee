/// Concatène les textes de tronçons transcrits en un texte lisible.
pub fn assemble_document(chunks: &[String]) -> String {
    chunks
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::assemble_document;

    #[test]
    fn joins_chunks_with_single_space_and_trims() {
        let out = assemble_document(&["Bonjour. ".into(), " Ça va".into()]);
        assert_eq!(out, "Bonjour. Ça va");
    }

    #[test]
    fn empty_chunks_are_skipped() {
        let out = assemble_document(&["Un.".into(), "".into(), "Deux.".into()]);
        assert_eq!(out, "Un. Deux.");
    }
}
