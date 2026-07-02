//! AI translation layer.
//!
//! One [`TranslationProvider`] trait, five concrete providers behind it:
//! OpenAI / OpenRouter / Local share the OpenAI-compatible chat API
//! ([`openai`]); Claude ([`anthropic`]) and Gemini ([`gemini`]) have their own
//! wire formats. All share the numbered-JSON batching in [`prompt`] so a batch
//! response can be re-aligned to its inputs, and shared retry/backoff in
//! [`retry`].

pub mod anthropic;
pub mod gemini;
pub mod openai;
pub mod prompt;
pub mod retry;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;

/// A single string to translate, already control-code-masked by the caller.
#[derive(Debug, Clone)]
pub struct BatchItem {
    pub id: i64,
    /// Masked source text (control codes replaced by ⟦n⟧ sentinels).
    pub text: String,
    pub context: Option<String>,
}

/// Glossary pair passed to the model for consistency.
#[derive(Debug, Clone)]
pub struct GlossPair {
    pub term: String,
    pub translation: String,
}

/// Everything a provider needs to translate one batch.
#[derive(Debug, Clone)]
pub struct BatchReq {
    pub items: Vec<BatchItem>,
    pub glossary: Vec<GlossPair>,
    pub source_lang: String,
    pub target_lang: String,
    pub tone: String,
    /// Extra user instructions appended to the system prompt.
    pub extra_system: Option<String>,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

/// Provider configuration sent from the frontend. The API key is *not* here —
/// it is loaded from the OS keychain by [`crate::keys`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// "openai" | "openrouter" | "local" | "anthropic" | "gemini"
    pub kind: String,
    pub base_url: Option<String>,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub batch_size: Option<usize>,
    /// Requests per minute throttle (0/None = unlimited).
    pub rpm: Option<u32>,
    pub tone: Option<String>,
    pub system_prompt: Option<String>,
}

impl ProviderConfig {
    pub fn temperature(&self) -> f32 {
        self.temperature.unwrap_or(0.3)
    }
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens.unwrap_or(4096)
    }
    pub fn batch_size(&self) -> usize {
        self.batch_size.unwrap_or(40).clamp(1, 200)
    }
    /// Minimum milliseconds between requests derived from the RPM throttle.
    pub fn min_interval_ms(&self) -> u64 {
        match self.rpm {
            Some(r) if r > 0 => (60_000 / r as u64).max(1),
            _ => 0,
        }
    }
    /// True if this provider needs an API key.
    pub fn needs_key(&self) -> bool {
        !matches!(self.kind.as_str(), "local")
    }
}

/// A translation backend.
#[async_trait]
pub trait TranslationProvider: Send + Sync {
    /// Translate one batch; returns translations aligned 1:1 with `req.items`.
    async fn translate_batch(
        &self,
        client: &reqwest::Client,
        key: Option<&str>,
        req: &BatchReq,
    ) -> Result<Vec<String>>;
}

/// Translate a batch; on any batch-level failure, fall back to translating each
/// item on its own so one bad item can't sink its whole batch. Returns results
/// aligned 1:1 with `req.items` (`None` = that item failed).
pub async fn translate_batch_or_split(
    provider: &dyn TranslationProvider,
    client: &reqwest::Client,
    key: Option<&str>,
    req: &BatchReq,
) -> Vec<Option<String>> {
    match provider.translate_batch(client, key, req).await {
        Ok(v) => v.into_iter().map(Some).collect(),
        Err(_) if req.items.len() <= 1 => vec![None; req.items.len()],
        Err(_) => {
            let mut out = Vec::with_capacity(req.items.len());
            for it in &req.items {
                let single = BatchReq {
                    items: vec![it.clone()],
                    ..req.clone()
                };
                match provider.translate_batch(client, key, &single).await {
                    Ok(mut v) => out.push(v.pop()),
                    Err(_) => out.push(None),
                }
            }
            out
        }
    }
}

/// Resolve the API base URL for a provider (config override or default).
fn resolve_base(cfg: &ProviderConfig) -> String {
    let default = match cfg.kind.as_str() {
        "openai" => "https://api.openai.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "local" => "http://localhost:11434/v1",
        "anthropic" => "https://api.anthropic.com",
        "gemini" => "https://generativelanguage.googleapis.com",
        _ => "",
    };
    cfg.base_url
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// List the models a provider currently offers. Used to populate the model
/// picker (notably Ollama's installed models via its OpenAI-compatible API).
pub async fn list_models(
    client: &reqwest::Client,
    key: Option<&str>,
    cfg: &ProviderConfig,
) -> Result<Vec<String>> {
    let base = resolve_base(cfg);
    let (url, mut req) = match cfg.kind.as_str() {
        "openai" | "openrouter" | "local" => {
            let url = format!("{base}/models");
            let mut rb = client.get(&url);
            if let Some(k) = key {
                rb = rb.bearer_auth(k);
            }
            (url, rb)
        }
        "anthropic" => {
            let url = format!("{base}/v1/models");
            let rb = client
                .get(&url)
                .header("x-api-key", key.unwrap_or(""))
                .header("anthropic-version", "2023-06-01");
            (url, rb)
        }
        "gemini" => {
            let url = format!("{base}/v1beta/models?key={}", key.unwrap_or(""));
            (url.clone(), client.get(&url))
        }
        other => return Err(anyhow!("unknown provider kind: {other}")),
    };
    req = req.header("accept", "application/json");

    let resp = req.send().await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("{status} from {url}: {text}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text)?;

    let mut models: Vec<String> = match cfg.kind.as_str() {
        "gemini" => v["models"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["name"].as_str())
                    .map(|n| n.strip_prefix("models/").unwrap_or(n).to_string())
                    .collect()
            })
            .unwrap_or_default(),
        // OpenAI-compatible + Anthropic both return { "data": [ { "id": ... } ] }.
        _ => v["data"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["id"].as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    };
    models.sort();
    models.dedup();
    Ok(models)
}

/// Build the concrete provider for a config.
pub fn make_provider(cfg: &ProviderConfig) -> Result<Box<dyn TranslationProvider>> {
    match cfg.kind.as_str() {
        "openai" => Ok(Box::new(openai::OpenAiCompat::openai(cfg))),
        "openrouter" => Ok(Box::new(openai::OpenAiCompat::openrouter(cfg))),
        "local" => Ok(Box::new(openai::OpenAiCompat::local(cfg))),
        "anthropic" => Ok(Box::new(anthropic::Anthropic::new(cfg))),
        "gemini" => Ok(Box::new(gemini::Gemini::new(cfg))),
        other => Err(anyhow!("unknown provider kind: {other}")),
    }
}
