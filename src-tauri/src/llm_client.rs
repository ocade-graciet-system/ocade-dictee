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
    pub truncated: bool,
}

/// Échec d'une complétion. Le dépassement de contexte est mis à part : le
/// serveur tournait et a lu la requête, mais le prompt et la sortie demandée
/// ne tiennent pas dans sa fenêtre. L'appelant (résumé local) doit pouvoir le
/// dire tel quel plutôt que de le confondre avec une panne du moteur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatCompletionError {
    /// `llama-server` répond HTTP 400 avec `"type": "exceed_context_size_error"`
    /// (et `n_prompt_tokens`, `n_ctx`) ; `detail` reprend le statut et le corps.
    ContextExceeded {
        detail: String,
    },
    Other(String),
}

impl std::fmt::Display for ChatCompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatCompletionError::ContextExceeded { detail }
            | ChatCompletionError::Other(detail) => f.write_str(detail),
        }
    }
}

impl From<String> for ChatCompletionError {
    fn from(detail: String) -> Self {
        ChatCompletionError::Other(detail)
    }
}

/// Type d'erreur que `llama-server` donne à une requête trop longue pour son
/// contexte.
const EXCEED_CONTEXT_ERROR_TYPE: &str = "exceed_context_size_error";

/// Classe une réponse d'erreur HTTP : dépassement de contexte si le corps JSON
/// porte `error.type == "exceed_context_size_error"`, autre échec sinon.
fn classify_error_response(status: reqwest::StatusCode, body: &str) -> ChatCompletionError {
    let detail = format!("API request failed with status {}: {}", status, body);
    let error_type = serde_json::from_str::<Value>(body).ok().and_then(|json| {
        json.get("error")
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    if error_type.as_deref() == Some(EXCEED_CONTEXT_ERROR_TYPE) {
        ChatCompletionError::ContextExceeded { detail }
    } else {
        ChatCompletionError::Other(detail)
    }
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
) -> Result<ChatCompletionOutcome, ChatCompletionError> {
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
        return Err(classify_error_response(status, &error_text));
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

    post_chat_completion(provider, &api_key, &request_body)
        .await
        .map(|outcome| outcome.content)
        .map_err(|e| e.to_string())
}

/// Variante sans schéma avec paramètres d'échantillonnage explicites (plan 09).
/// Rend aussi la troncature, que le résumé local utilise pour signaler une
/// tranche coupée par `max_tokens`, et distingue le dépassement de contexte.
pub async fn send_chat_completion_with_options(
    provider: &PostProcessProvider,
    api_key: String,
    model: &str,
    user_content: String,
    system_prompt: Option<String>,
    options: ChatSamplingOptions,
) -> Result<ChatCompletionOutcome, ChatCompletionError> {
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

    /// Corps rendu par `llama-server` (contexte de 12 288 tokens) au banc
    /// d'essai du 24/10/2026, pour un prompt trop long.
    const EXCEED_CONTEXT_BODY: &str = r#"{"error":{"code":400,"message":"the request exceeds the available context size, try increasing it","type":"exceed_context_size_error","n_prompt_tokens":13104,"n_ctx":12288}}"#;

    #[test]
    fn context_size_errors_are_told_apart() {
        let exceeded =
            classify_error_response(reqwest::StatusCode::BAD_REQUEST, EXCEED_CONTEXT_BODY);
        match &exceeded {
            ChatCompletionError::ContextExceeded { detail } => {
                assert!(detail.contains("400"), "{detail}");
                assert!(detail.contains("n_prompt_tokens"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
        assert!(exceeded.to_string().contains("exceed_context_size_error"));

        // Une autre erreur 400, un corps non JSON ou un 500 restent « autres ».
        for (status, body) in [
            (
                reqwest::StatusCode::BAD_REQUEST,
                r#"{"error":{"code":400,"message":"bad","type":"invalid_request_error"}}"#,
            ),
            (
                reqwest::StatusCode::BAD_REQUEST,
                "exceed_context_size_error",
            ),
            (reqwest::StatusCode::INTERNAL_SERVER_ERROR, "boom"),
        ] {
            assert!(
                matches!(
                    classify_error_response(status, body),
                    ChatCompletionError::Other(_)
                ),
                "{status} {body}"
            );
        }
    }

    /// Serveur HTTP factice : lit une requête (en-têtes et corps, pour ne pas
    /// couper la connexion sous le client), répond `status` et `body`.
    fn fake_chat_server(status: &'static str, body: &'static str) -> u16 {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0usize;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                line.clear();
            }
            let mut request_body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut request_body);
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        port
    }

    fn local_provider(port: u16) -> PostProcessProvider {
        PostProcessProvider {
            id: "llama-local".into(),
            label: "llama.cpp local".into(),
            base_url: format!("http://127.0.0.1:{port}/v1"),
            allow_base_url_edit: false,
            models_endpoint: None,
            supports_structured_output: false,
        }
    }

    #[tokio::test]
    async fn a_too_long_request_comes_back_as_context_exceeded() {
        let port = fake_chat_server("400 Bad Request", EXCEED_CONTEXT_BODY);
        let err = send_chat_completion_with_options(
            &local_provider(port),
            "k".into(),
            "m",
            "texte".into(),
            Some("système".into()),
            ChatSamplingOptions::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, ChatCompletionError::ContextExceeded { ref detail } if detail.contains("n_ctx")),
            "{err:?}"
        );

        // Les points d'entrée à erreur texte gardent le message d'avant.
        let port = fake_chat_server("400 Bad Request", EXCEED_CONTEXT_BODY);
        let err = send_chat_completion(
            &local_provider(port),
            "k".into(),
            "m",
            "t".into(),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(
            err.starts_with("API request failed with status 400 Bad Request:"),
            "{err}"
        );
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
