use crate::settings::PostProcessProvider;
use log::debug;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, REFERER, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::IpAddr;
use std::time::Duration;

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct JsonSchema {
    name: String,
    strict: bool,
    schema: Value,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
    json_schema: JsonSchema,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<bool>,
}

/// Paramètres d'échantillonnage optionnels (plan 09 : température basse et
/// plafond de tokens pour le compte-rendu local). Absents du JSON quand `None`,
/// pour que les fournisseurs distants gardent exactement le corps d'aujourd'hui.
// `dead_code` : consommé par l'orchestration du résumé (tâche 12 du plan 09) ;
// l'attribut tombe avec son premier appelant.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatSamplingOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
}

/// Résultat d'une complétion : le contenu du premier choix et l'indicateur de
/// troncature. `truncated` vaut `true` quand le serveur a rendu
/// `finish_reason: "length"`, c'est-à-dire que la sortie a été coupée par
/// `max_tokens` — l'appelant (résumé par tranches) doit le savoir plutôt que de
/// prendre un texte amputé pour une réponse complète.
#[derive(Debug, Clone, Default)]
pub struct ChatCompletionOutcome {
    pub content: Option<String>,
    // `dead_code` : lu par la tâche 12 du plan 09, pas encore ici.
    #[allow(dead_code)]
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
    /// `"stop"`, `"length"`, … ; absent ou `null` chez certains serveurs.
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: Option<String>,
}

/// Délai global d'une requête de complétion. Le serveur llama.cpp local
/// pré-traite un long prompt à quelques centaines de tokens/s sur CPU : une
/// tranche de 9 000 tokens puis 900 tokens de sortie peut prendre plusieurs
/// minutes sur un PC portable ; 15 minutes laissent de la marge sans jamais
/// bloquer indéfiniment sur un serveur muet.
const CHAT_COMPLETION_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Délai d'établissement de la connexion : court, car un hôte injoignable se
/// voit tout de suite. Seule la génération a besoin des 15 minutes ci-dessus.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Vrai quand l'URL désigne la boucle locale (`127.0.0.0/8`, `::1`,
/// `localhost`). `reqwest` active `auto_sys_proxy` par défaut et n'exempte pas
/// la boucle locale : sans `no_proxy`, un proxy système (fréquent en
/// entreprise) capterait les appels au `llama-server` local et les ferait tous
/// échouer.
fn is_loopback_base_url(base_url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(base_url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    // `host_str` garde les crochets d'une adresse IPv6 (`[::1]`).
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Build headers for API requests based on provider type
fn build_headers(provider: &PostProcessProvider, api_key: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();

    // Common headers
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        REFERER,
        HeaderValue::from_static("https://github.com/cjpais/Handy"),
    );
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("Handy/1.0 (+https://github.com/cjpais/Handy)"),
    );
    headers.insert("X-Title", HeaderValue::from_static("Handy"));

    // Provider-specific auth headers
    if !api_key.is_empty() {
        if provider.id == "anthropic" {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(api_key)
                    .map_err(|e| format!("Invalid API key header value: {}", e))?,
            );
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        } else {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", api_key))
                    .map_err(|e| format!("Invalid authorization header value: {}", e))?,
            );
        }
    }

    Ok(headers)
}

/// Create an HTTP client with provider-specific headers
fn create_client(provider: &PostProcessProvider, api_key: &str) -> Result<reqwest::Client, String> {
    let headers = build_headers(provider, api_key)?;
    let mut builder = reqwest::Client::builder()
        .default_headers(headers)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(CHAT_COMPLETION_TIMEOUT);
    if is_loopback_base_url(&provider.base_url) {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

/// Message système optionnel puis message utilisateur, dans cet ordre.
fn build_messages(user_content: String, system_prompt: Option<String>) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    if let Some(system) = system_prompt {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system,
        });
    }
    messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_content,
    });
    messages
}

/// Contenu du premier choix et troncature déduite de son `finish_reason`.
fn outcome_from_response(completion: ChatCompletionResponse) -> ChatCompletionOutcome {
    let choice = completion.choices.first();
    ChatCompletionOutcome {
        content: choice.and_then(|choice| choice.message.content.clone()),
        truncated: choice
            .and_then(|choice| choice.finish_reason.as_deref())
            .is_some_and(|reason| reason == "length"),
    }
}

/// Envoi effectif d'une requête `chat/completions` et lecture du premier choix.
/// Partagé par les points d'entrée publics.
async fn post_chat_completion(
    provider: &PostProcessProvider,
    api_key: &str,
    request_body: &ChatCompletionRequest,
) -> Result<ChatCompletionOutcome, String> {
    let base_url = provider.base_url.trim_end_matches('/');
    let url = format!("{}/chat/completions", base_url);

    debug!("Sending chat completion request to: {}", url);

    let client = create_client(provider, api_key)?;

    let response = client
        .post(&url)
        .json(request_body)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Failed to read error response".to_string());
        return Err(format!(
            "API request failed with status {}: {}",
            status, error_text
        ));
    }

    let completion: ChatCompletionResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse API response: {}", e))?;

    Ok(outcome_from_response(completion))
}

/// Send a chat completion request to an OpenAI-compatible API
/// Returns Ok(Some(content)) on success, Ok(None) if response has no content,
/// or Err on actual errors (HTTP, parsing, etc.)
pub async fn send_chat_completion(
    provider: &PostProcessProvider,
    api_key: String,
    model: &str,
    prompt: String,
    reasoning_effort: Option<String>,
    reasoning: Option<ReasoningConfig>,
) -> Result<Option<String>, String> {
    send_chat_completion_with_schema(
        provider,
        api_key,
        model,
        prompt,
        None,
        None,
        reasoning_effort,
        reasoning,
    )
    .await
}

/// Send a chat completion request with structured output support
/// When json_schema is provided, uses structured outputs mode
/// system_prompt is used as the system message when provided
/// reasoning_effort sets the OpenAI-style top-level field (e.g., "none", "low", "medium", "high")
/// reasoning sets the OpenRouter-style nested object (effort + exclude)
#[allow(clippy::too_many_arguments)]
pub async fn send_chat_completion_with_schema(
    provider: &PostProcessProvider,
    api_key: String,
    model: &str,
    user_content: String,
    system_prompt: Option<String>,
    json_schema: Option<Value>,
    reasoning_effort: Option<String>,
    reasoning: Option<ReasoningConfig>,
) -> Result<Option<String>, String> {
    // Build response_format if schema is provided
    let response_format = json_schema.map(|schema| ResponseFormat {
        format_type: "json_schema".to_string(),
        json_schema: JsonSchema {
            name: "transcription_output".to_string(),
            strict: true,
            schema,
        },
    });

    let request_body = ChatCompletionRequest {
        model: model.to_string(),
        messages: build_messages(user_content, system_prompt),
        response_format,
        reasoning_effort,
        reasoning,
        temperature: None,
        max_tokens: None,
    };

    Ok(post_chat_completion(provider, &api_key, &request_body)
        .await?
        .content)
}

/// Variante sans schéma avec paramètres d'échantillonnage explicites (plan 09).
/// Rend aussi la troncature, que le résumé local utilise pour signaler une
/// tranche coupée par `max_tokens`.
#[allow(dead_code)]
pub async fn send_chat_completion_with_options(
    provider: &PostProcessProvider,
    api_key: String,
    model: &str,
    user_content: String,
    system_prompt: Option<String>,
    options: ChatSamplingOptions,
) -> Result<ChatCompletionOutcome, String> {
    let request_body = ChatCompletionRequest {
        model: model.to_string(),
        messages: build_messages(user_content, system_prompt),
        response_format: None,
        reasoning_effort: None,
        reasoning: None,
        temperature: options.temperature,
        max_tokens: options.max_tokens,
    };

    post_chat_completion(provider, &api_key, &request_body).await
}

/// Fetch available models from an OpenAI-compatible API
/// Returns a list of model IDs
pub async fn fetch_models(
    provider: &PostProcessProvider,
    api_key: String,
) -> Result<Vec<String>, String> {
    let base_url = provider.base_url.trim_end_matches('/');
    let url = format!("{}/models", base_url);

    debug!("Fetching models from: {}", url);

    let client = create_client(provider, &api_key)?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch models: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!(
            "Model list request failed ({}): {}",
            status, error_text
        ));
    }

    let parsed: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    let mut models = Vec::new();

    // Handle OpenAI format: { data: [ { id: "..." }, ... ] }
    if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
        for entry in data {
            if let Some(id) = entry.get("id").and_then(|i| i.as_str()) {
                models.push(id.to_string());
            } else if let Some(name) = entry.get("name").and_then(|n| n.as_str()) {
                models.push(name.to_string());
            }
        }
    }
    // Handle array format: [ "model1", "model2", ... ]
    else if let Some(array) = parsed.as_array() {
        for entry in array {
            if let Some(model) = entry.as_str() {
                models.push(model.to_string());
            }
        }
    }

    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_options_are_serialized_only_when_set() {
        let body = ChatCompletionRequest {
            model: "m".into(),
            messages: build_messages("u".into(), Some("s".into())),
            response_format: None,
            reasoning_effort: None,
            reasoning: None,
            temperature: Some(0.2),
            max_tokens: Some(1200),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["temperature"], 0.2);
        assert_eq!(json["max_tokens"], 1200);
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][1]["role"], "user");
        assert!(json.get("response_format").is_none());
        let without = ChatCompletionRequest {
            model: "m".into(),
            messages: build_messages("u".into(), None),
            response_format: None,
            reasoning_effort: None,
            reasoning: None,
            temperature: None,
            max_tokens: None,
        };
        let json = serde_json::to_value(&without).unwrap();
        assert!(json.get("temperature").is_none());
        assert!(json.get("max_tokens").is_none());
    }

    #[test]
    fn local_server_gets_bearer_header() {
        let provider = PostProcessProvider {
            id: "llama-local".into(),
            label: "llama.cpp local".into(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            allow_base_url_edit: false,
            models_endpoint: None,
            supports_structured_output: false,
        };
        let headers = build_headers(&provider, "secret").unwrap();
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer secret");
    }

    #[test]
    fn finish_reason_length_marks_the_outcome_as_truncated() {
        let cut: ChatCompletionResponse = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"coupé"},"finish_reason":"length"}]}"#,
        )
        .unwrap();
        let outcome = outcome_from_response(cut);
        assert_eq!(outcome.content.as_deref(), Some("coupé"));
        assert!(outcome.truncated);

        let stopped: ChatCompletionResponse = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"entier"},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        assert!(!outcome_from_response(stopped).truncated);

        let missing: ChatCompletionResponse =
            serde_json::from_str(r#"{"choices":[{"message":{"content":"entier"}}]}"#).unwrap();
        let outcome = outcome_from_response(missing);
        assert_eq!(outcome.content.as_deref(), Some("entier"));
        assert!(!outcome.truncated);

        let empty: ChatCompletionResponse = serde_json::from_str(r#"{"choices":[]}"#).unwrap();
        let outcome = outcome_from_response(empty);
        assert!(outcome.content.is_none());
        assert!(!outcome.truncated);
    }

    #[test]
    fn loopback_base_urls_are_detected() {
        assert!(is_loopback_base_url("http://127.0.0.1:1234/v1"));
        assert!(is_loopback_base_url("http://localhost:11434/v1"));
        assert!(is_loopback_base_url("http://LocalHost:11434/v1"));
        assert!(is_loopback_base_url("http://[::1]:1234/v1"));
        assert!(!is_loopback_base_url("https://api.openai.com/v1"));
        assert!(!is_loopback_base_url("https://127.0.0.1.example.com/v1"));
        assert!(!is_loopback_base_url("https://openrouter.ai/api/v1"));
        assert!(!is_loopback_base_url("pas une url"));
    }
}
