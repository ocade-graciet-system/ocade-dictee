use crate::llm_client::send_chat_completion;
use crate::settings::get_settings;
use tauri::AppHandle;

/// Préfixe du prompt de mise en forme envoyé au LLM configuré pour le
/// post-traitement. Contrairement à `post_process_transcription` dans
/// `actions.rs`, cette action est déclenchée explicitement par un bouton dans
/// l'onglet Fichier : elle réutilise le même fournisseur/modèle/clé API que le
/// post-traitement de dictée, mais n'est PAS gardée par
/// `settings.post_process_enabled` ni par le prompt sélectionné pour la
/// dictée.
const FORMAT_PROMPT_PREFIX: &str = "Tu mets en forme des transcriptions en français. Reformate le texte suivant en un document clair et bien structuré : paragraphes logiques, titres/sections si pertinent ; corrige l'orthographe, la grammaire, les accents et la ponctuation ; NE CHANGE NI LE SENS NI LES PROPOS. Réponds UNIQUEMENT par le document mis en forme, sans commentaire.\n\nTexte à mettre en forme :\n";

/// Envoie le document transcrit au fournisseur de post-traitement actif
/// (local Ollama ou cloud, selon la config utilisateur) pour le reformater.
/// Calqué sur `post_process_transcription` (actions.rs) pour la récupération
/// provider/modèle/clé API, mais sans la garde `post_process_enabled` : c'est
/// une action explicite via bouton, indépendante du post-traitement de
/// dictée.
#[tauri::command]
#[specta::specta]
pub async fn format_document(app: AppHandle, text: String) -> Result<String, String> {
    let settings = get_settings(&app);

    let provider = settings
        .active_post_process_provider()
        .cloned()
        .ok_or_else(|| "Aucun fournisseur de mise en forme configuré".to_string())?;

    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    if model.trim().is_empty() {
        return Err("Aucun fournisseur de mise en forme configuré".to_string());
    }

    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    let prompt = format!("{FORMAT_PROMPT_PREFIX}{text}");

    match send_chat_completion(&provider, api_key, &model, prompt, None, None).await {
        Ok(Some(formatted)) => Ok(formatted),
        Ok(None) => Err("Réponse vide du modèle".to_string()),
        Err(e) => Err(e),
    }
}

/// Indique si un fournisseur de post-traitement actif ET un modèle non vide
/// sont configurés, pour piloter l'affichage du bouton « Mettre en forme »
/// côté UI (bouton si `true`, encart de configuration sinon).
#[tauri::command]
#[specta::specta]
pub fn format_ready(app: AppHandle) -> bool {
    let settings = get_settings(&app);

    match settings.active_post_process_provider() {
        Some(provider) => settings
            .post_process_models
            .get(&provider.id)
            .map(|model| !model.trim().is_empty())
            .unwrap_or(false),
        None => false,
    }
}
