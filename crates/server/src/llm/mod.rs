//! LLM chat + embeddings client.
//!
//! Chat/vision completions and embeddings are served by a configurable,
//! OpenAI-compatible provider — the LiteLLM proxy by default, or OpenRouter
//! when `FORAGEBUDDY_LLM_PROVIDER=openrouter` (the revert switch; note
//! OpenRouter has no native embeddings — it proxies to OpenAI via a BYOK
//! key).
//!
//! Ported from `ai_buddy`'s `llm` module, **minus every cost/billing/wallet
//! concept** (this app has no payments, so there is no `cost_cents`, no
//! `model_rate`, no USD→EUR conversion) **and minus streaming** (there is no
//! live chat UI here, so no `chat_stream`/SSE parsing). **Plus vision**:
//! [`LlmClient::chat_json_vision`] attaches one or more images to the user
//! turn using the standard OpenAI vision content-array format, used by the
//! triage pass to send photos straight to the model.
//!
//! Exposes:
//!   - JSON-mode structured completion ([`LlmClient::chat_json`]),
//!   - the same, with images attached ([`LlmClient::chat_json_vision`]),
//!   - raw tool-calling turns ([`LlmClient::chat_tools`]), used by the
//!     deep-dive Wikipedia-agentic-query loop,
//!   - single-text embeddings ([`LlmClient::embed`]).
//!
//! The chat methods take an explicit `model` so callers can honour a
//! per-request model choice. Use [`LlmClient::resolve_model`] to validate a
//! requested id against the allowed list before calling.

use std::time::Duration;

use anyhow::anyhow;
use serde::de::DeserializeOwned;

use crate::config::Config;
use crate::error::{AppError, AppResult};

/// OpenRouter chat completions endpoint. Retained so switching back to the
/// OpenRouter chat path (`FORAGEBUDDY_LLM_PROVIDER=openrouter`) is trivial.
const OPENROUTER_CHAT_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const OPENROUTER_EMBED_URL: &str = "https://openrouter.ai/api/v1/embeddings";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Embeddings are called in a loop (once per Wikipedia chunk during
/// deep-dive's grounding step) and are supposed to be small/fast — unlike
/// chat/vision/tool calls, a single stalled embed call must not be allowed
/// to eat the shared 60s client timeout, since several of those in a row
/// would blow through the server's own 120s per-request budget
/// (routes.rs's TimeoutLayer) before deep-dive's own warn-and-skip
/// resilience (see deepdive/mod.rs) ever gets a chance to move on.
const EMBED_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A single tool/function call requested by the model in a [`ToolTurn`].
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// Provider-assigned id, echoed back in the matching `role:"tool"` message.
    pub id: String,
    /// The function name (e.g. `search_wikipedia`).
    pub name: String,
    /// The raw JSON argument string exactly as the model produced it.
    pub arguments: String,
}

/// The result of one non-streaming tool-calling turn. Either `content` carries
/// the assistant's prose reply, or `tool_calls` carries one or more requested
/// function invocations (a turn can, in principle, carry both).
#[derive(Debug, Clone)]
pub struct ToolTurn {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone)]
pub struct LlmClient {
    inner: std::sync::Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    /// Full chat completions URL (provider-dependent).
    chat_url: String,
    /// API key for the chat provider (LiteLLM or OpenRouter).
    chat_api_key: Option<String>,
    /// Default chat model when the caller passes an unknown/empty model.
    default_chat_model: String,
    /// Model for the identification pipeline's facts/risks gatherers — see
    /// `Config::identification_chat_model`'s doc comment. Falls back to
    /// `default_chat_model` when `FORAGEBUDDY_IDENTIFICATION_MODEL` is unset.
    identification_chat_model: String,
    /// Chat model ids a caller may select.
    allowed_chat_models: Vec<String>,
    /// Full embeddings URL (provider-dependent).
    embed_url: String,
    /// API key for the embeddings provider.
    embed_api_key: Option<String>,
    embedding_model: String,
    #[allow(dead_code)]
    embedding_dim: usize,
}

impl LlmClient {
    pub fn new(config: &Config) -> Self {
        // Harden the outbound client like the OIDC one: no redirects (SSRF
        // defence) and a bounded timeout so a slow provider can't pin a
        // request open. Fall back to a default client if the builder fails
        // (panic-safe).
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        // Pick the chat endpoint + key by provider (embeddings mirror it below).
        let (chat_url, chat_api_key) = if config.llm_provider == "openrouter" {
            (
                OPENROUTER_CHAT_URL.to_string(),
                config.openrouter_api_key.clone(),
            )
        } else {
            // Default / "litellm": OpenAI-compatible base + /chat/completions.
            (
                format!(
                    "{}/chat/completions",
                    config.litellm_base_url.trim_end_matches('/')
                ),
                config.litellm_api_key.clone(),
            )
        };

        // Embeddings follow the same provider. LiteLLM serves `bge-m3` etc.;
        // OpenRouter's /embeddings proxies to OpenAI (BYOK) and is only used
        // when llm_provider=openrouter. The OpenRouter URLs/keys stay wired
        // for revert.
        let (embed_url, embed_api_key) = if config.llm_provider == "openrouter" {
            (
                OPENROUTER_EMBED_URL.to_string(),
                config.openrouter_api_key.clone(),
            )
        } else {
            (
                format!(
                    "{}/embeddings",
                    config.litellm_base_url.trim_end_matches('/')
                ),
                config.litellm_api_key.clone(),
            )
        };

        Self {
            inner: std::sync::Arc::new(Inner {
                http,
                chat_url,
                chat_api_key,
                default_chat_model: config.chat_model.clone(),
                identification_chat_model: config.identification_chat_model.clone(),
                allowed_chat_models: config.allowed_chat_models.clone(),
                embed_url,
                embed_api_key,
                embedding_model: config.embedding_model.clone(),
                embedding_dim: config.embedding_dim,
            }),
        }
    }

    /// The configured default chat model.
    pub fn default_chat_model(&self) -> &str {
        &self.inner.default_chat_model
    }

    /// The configured identification-pipeline chat model (falls back to the
    /// default chat model when `FORAGEBUDDY_IDENTIFICATION_MODEL` is unset).
    pub fn identification_chat_model(&self) -> &str {
        &self.inner.identification_chat_model
    }

    /// Resolve a requested model to a usable one: return `requested` when it
    /// is in the allowed list, otherwise fall back to the default.
    pub fn resolve_model(&self, requested: Option<&str>) -> String {
        match requested {
            Some(m) if self.inner.allowed_chat_models.iter().any(|a| a == m) => m.to_string(),
            _ => self.inner.default_chat_model.clone(),
        }
    }

    /// The chat provider's API key, or a `BadRequest` when unconfigured.
    fn require_chat_key(&self) -> AppResult<&str> {
        self.inner
            .chat_api_key
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("LLM API key not configured".to_string()))
    }

    /// The embeddings provider's API key, or a `BadRequest` when unset.
    fn require_embed_key(&self) -> AppResult<&str> {
        self.inner
            .embed_api_key
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("embeddings API key not configured".to_string()))
    }

    /// JSON-mode structured completion, no images. Sends
    /// `response_format:json_object` and `temperature:0`, then extracts the
    /// first `{ … }` object from the model's output (tolerating prose / code
    /// fences around it) and deserializes it into `T`.
    pub async fn chat_json<T: DeserializeOwned>(
        &self,
        model: &str,
        system: &str,
        user: &str,
    ) -> AppResult<T> {
        let messages = serde_json::json!([
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ]);
        let content = self.complete_json_mode(model, messages).await?;
        decode_json_object(&content)
    }

    /// Same as [`Self::chat_json`], but the user turn also carries `images`
    /// (each a full data URL like `data:image/jpeg;base64,....`). When
    /// `images` is empty this behaves exactly like [`Self::chat_json`].
    pub async fn chat_json_vision<T: DeserializeOwned>(
        &self,
        model: &str,
        system: &str,
        user_text: &str,
        images: &[String],
    ) -> AppResult<T> {
        let messages = serde_json::json!([
            { "role": "system", "content": system },
            { "role": "user", "content": build_vision_content(user_text, images) },
        ]);
        let content = self.complete_json_mode(model, messages).await?;
        decode_json_object(&content)
    }

    /// Shared JSON-mode request path for [`Self::chat_json`] and
    /// [`Self::chat_json_vision`]: posts `messages` with `response_format:
    /// json_object` + `temperature:0` and returns the assistant's raw text.
    async fn complete_json_mode(
        &self,
        model: &str,
        messages: serde_json::Value,
    ) -> AppResult<String> {
        let key = self.require_chat_key()?;
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "response_format": { "type": "json_object" },
            "temperature": 0,
        });

        let resp = self
            .inner
            .http
            .post(&self.inner.chat_url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Internal(anyhow!("LLM request failed: {e}")))?;

        let parsed: ChatResponse = read_json(resp).await?;
        parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| AppError::Internal(anyhow!("LLM returned no content")))
    }

    /// Non-streaming, tool-calling chat completion. `messages` is the raw
    /// OpenAI-style messages array (each an object with `role`/`content`, and
    /// possibly an assistant `tool_calls` array or a `role:"tool"` result),
    /// and `tools` is the OpenAI `tools` array. Sends `tool_choice:"auto"` and
    /// parses the first choice into a [`ToolTurn`] (assistant prose and/or
    /// requested function calls). Missing fields degrade to empty.
    pub async fn chat_tools(
        &self,
        model: &str,
        messages: Vec<serde_json::Value>,
        tools: &serde_json::Value,
    ) -> AppResult<ToolTurn> {
        let key = self.require_chat_key()?;
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
        });

        let resp = self
            .inner
            .http
            .post(&self.inner.chat_url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Internal(anyhow!("LLM request failed: {e}")))?;

        let parsed: ToolChatResponse = read_json(resp).await?;
        let (content, tool_calls) = match parsed.choices.into_iter().next() {
            Some(choice) => {
                let calls = choice
                    .message
                    .tool_calls
                    .into_iter()
                    .map(|t| ToolCall {
                        id: t.id,
                        name: t.function.name,
                        arguments: t.function.arguments,
                    })
                    .collect();
                (choice.message.content, calls)
            }
            None => (None, Vec::new()),
        };

        Ok(ToolTurn {
            content,
            tool_calls,
        })
    }

    /// Embed a single text into a `Vec<f32>` (length `embedding_dim`). On any
    /// endpoint error the [`AppError`] is returned so callers can treat RAG as
    /// best-effort.
    pub async fn embed(&self, text: &str) -> AppResult<Vec<f32>> {
        let key = self.require_embed_key()?;
        let body = serde_json::json!({
            "model": self.inner.embedding_model,
            "input": text,
        });

        let resp = self
            .inner
            .http
            .post(&self.inner.embed_url)
            .bearer_auth(key)
            .timeout(EMBED_REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Internal(anyhow!("embeddings request failed: {e}")))?;

        let parsed: EmbedResponse = read_json(resp).await?;
        parsed
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| AppError::Internal(anyhow!("embeddings provider returned no embedding")))
    }
}

/// Decode the first `{ … }` JSON object found in `content` into `T`.
fn decode_json_object<T: DeserializeOwned>(content: &str) -> AppResult<T> {
    let json = extract_json(content)
        .ok_or_else(|| AppError::Internal(anyhow!("no JSON object in model output")))?;
    serde_json::from_str(json)
        .map_err(|e| AppError::Internal(anyhow!("could not parse model JSON: {e}")))
}

/// Build the OpenAI vision content value for a user turn: plain text when
/// `images` is empty (so JSON-mode providers that don't like content arrays
/// for text-only turns still get a plain string), otherwise a content array
/// of one `{"type":"text",...}` part followed by one `{"type":"image_url",
/// "image_url":{"url": ...}}` part per image.
fn build_vision_content(text: &str, images: &[String]) -> serde_json::Value {
    if images.is_empty() {
        return serde_json::Value::String(text.to_string());
    }

    let mut parts = Vec::with_capacity(images.len() + 1);
    parts.push(serde_json::json!({ "type": "text", "text": text }));
    for image in images {
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": image },
        }));
    }
    serde_json::Value::Array(parts)
}

/// Read a JSON response, mapping non-2xx statuses to `AppError` (401 → a
/// `BadRequest` naming the key, other statuses → `Internal` with the body).
async fn read_json<T: DeserializeOwned>(resp: reqwest::Response) -> AppResult<T> {
    let status = resp.status();
    let raw = resp
        .text()
        .await
        .map_err(|e| AppError::Internal(anyhow!("reading LLM response failed: {e}")))?;

    if !status.is_success() {
        if status.as_u16() == 401 {
            return Err(AppError::BadRequest(
                "LLM provider rejected the API key".to_string(),
            ));
        }
        return Err(AppError::Internal(anyhow!("LLM returned {status}: {raw}")));
    }

    serde_json::from_str(&raw)
        .map_err(|e| AppError::Internal(anyhow!("unexpected LLM response: {e}")))
}

// --- OpenAI-compatible response shapes ---

#[derive(Debug, serde::Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, serde::Deserialize)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

// Tool-calling response shapes (parsed by `chat_tools`). Kept separate from
// `ChatResponse` so the plain JSON-mode path is unaffected.

#[derive(Debug, serde::Deserialize)]
struct ToolChatResponse {
    #[serde(default)]
    choices: Vec<ToolChatChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct ToolChatChoice {
    message: ToolChatMessage,
}

#[derive(Debug, serde::Deserialize)]
struct ToolChatMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<RespToolCall>,
}

#[derive(Debug, serde::Deserialize)]
struct RespToolCall {
    #[serde(default)]
    id: String,
    function: RespFunction,
}

#[derive(Debug, serde::Deserialize)]
struct RespFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Debug, serde::Deserialize)]
struct EmbedResponse {
    #[serde(default)]
    data: Vec<EmbedData>,
}

#[derive(Debug, serde::Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

/// Pull the first `{ … }` JSON object out of `content`, tolerating a model
/// that wrapped it in prose or ```json fences.
fn extract_json(content: &str) -> Option<&str> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end < start {
        return None;
    }
    content.get(start..=end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_honours_allowed_list() {
        let client = LlmClient::new(&Config::from_env());
        // The config default is always allowed.
        assert_eq!(
            client.default_chat_model(),
            "openrouter/~anthropic/claude-haiku-latest"
        );
        // An allowed model is returned as-is.
        assert_eq!(client.resolve_model(Some("gemma4-26b")), "gemma4-26b");
        // A disallowed / absent model falls back to the default.
        assert_eq!(
            client.resolve_model(Some("not-allowed")),
            client.default_chat_model()
        );
        assert_eq!(client.resolve_model(None), client.default_chat_model());
    }

    #[test]
    fn extract_json_handles_fenced_output() {
        let fenced = "```json\n{\"a\":1}\n```";
        assert_eq!(extract_json(fenced), Some("{\"a\":1}"));
        assert_eq!(
            extract_json("prose {\"x\":true} more"),
            Some("{\"x\":true}")
        );
        assert_eq!(extract_json("no json here"), None);
        assert_eq!(extract_json("} {"), None);
    }

    #[test]
    fn decode_json_object_parses_wrapped_output() {
        #[derive(Debug, serde::Deserialize, PartialEq)]
        struct Out {
            status: String,
        }
        let out: Out = decode_json_object("here you go: {\"status\":\"ok\"} thanks").unwrap();
        assert_eq!(
            out,
            Out {
                status: "ok".to_string()
            }
        );
    }

    #[test]
    fn decode_json_object_rejects_missing_json() {
        let err = decode_json_object::<serde_json::Value>("no braces here");
        assert!(err.is_err());
    }

    #[test]
    fn build_vision_content_plain_text_when_no_images() {
        let value = build_vision_content("hello", &[]);
        assert_eq!(value, serde_json::Value::String("hello".to_string()));
    }

    #[test]
    fn build_vision_content_builds_parts_array() {
        let images = vec![
            "data:image/jpeg;base64,AAAA".to_string(),
            "data:image/jpeg;base64,BBBB".to_string(),
        ];
        let value = build_vision_content("what is this?", &images);
        let arr = value.as_array().expect("expected content array");
        assert_eq!(arr.len(), 3);

        let part0 = arr.first().expect("part 0");
        assert_eq!(part0.get("type").and_then(|v| v.as_str()), Some("text"));
        assert_eq!(
            part0.get("text").and_then(|v| v.as_str()),
            Some("what is this?")
        );

        let part1 = arr.get(1).expect("part 1");
        assert_eq!(
            part1.get("type").and_then(|v| v.as_str()),
            Some("image_url")
        );
        assert_eq!(
            part1
                .get("image_url")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str()),
            Some("data:image/jpeg;base64,AAAA")
        );

        let part2 = arr.get(2).expect("part 2");
        assert_eq!(
            part2
                .get("image_url")
                .and_then(|v| v.get("url"))
                .and_then(|v| v.as_str()),
            Some("data:image/jpeg;base64,BBBB")
        );
    }
}
