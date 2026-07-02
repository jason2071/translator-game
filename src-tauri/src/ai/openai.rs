//! OpenAI-compatible chat provider — serves OpenAI, OpenRouter, and Local
//! (Ollama / LM Studio) backends, which differ only by base URL and auth.

use super::prompt::{build_messages, parse_batch_response};
use super::retry::{status_is_retryable, with_retry, CallError};
use super::{BatchReq, ProviderConfig, TranslationProvider};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;

pub struct OpenAiCompat {
    base: String,
    is_openrouter: bool,
}

impl OpenAiCompat {
    pub fn openai(cfg: &ProviderConfig) -> Self {
        OpenAiCompat {
            base: base_or(cfg, "https://api.openai.com/v1"),
            is_openrouter: false,
        }
    }
    pub fn openrouter(cfg: &ProviderConfig) -> Self {
        OpenAiCompat {
            base: base_or(cfg, "https://openrouter.ai/api/v1"),
            is_openrouter: true,
        }
    }
    pub fn local(cfg: &ProviderConfig) -> Self {
        // Ollama's OpenAI-compatible endpoint; LM Studio uses :1234/v1.
        OpenAiCompat {
            base: base_or(cfg, "http://localhost:11434/v1"),
            is_openrouter: false,
        }
    }
}

fn base_or(cfg: &ProviderConfig, default: &str) -> String {
    cfg.base_url
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
        .trim_end_matches('/')
        .to_string()
}

#[async_trait]
impl TranslationProvider for OpenAiCompat {
    async fn translate_batch(
        &self,
        client: &reqwest::Client,
        key: Option<&str>,
        req: &BatchReq,
    ) -> Result<Vec<String>> {
        let (sys, user) = build_messages(req);
        let url = format!("{}/chat/completions", self.base);
        let body = json!({
            "model": req.model,
            "messages": [
                { "role": "system", "content": sys },
                { "role": "user", "content": user },
            ],
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
        });

        let content = with_retry(4, 800, || async {
            let mut rb = client.post(&url).json(&body);
            if let Some(k) = key {
                rb = rb.bearer_auth(k);
            }
            if self.is_openrouter {
                rb = rb
                    .header("HTTP-Referer", "https://github.com/rpgtl")
                    .header("X-Title", "RPGMaker Translator");
            }
            let resp = rb
                .send()
                .await
                .map_err(|e| CallError::Retryable(e.into()))?;
            let status = resp.status();
            let text = resp
                .text()
                .await
                .map_err(|e| CallError::Retryable(e.into()))?;
            if status.is_success() {
                let v: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|e| CallError::Fatal(e.into()))?;
                v["choices"][0]["message"]["content"]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| CallError::Fatal(anyhow!("unexpected response: {text}")))
            } else if status_is_retryable(status.as_u16()) {
                Err(CallError::Retryable(anyhow!("{status}: {text}")))
            } else {
                Err(CallError::Fatal(anyhow!("{status}: {text}")))
            }
        })
        .await?;

        parse_batch_response(&content, req.items.len())
    }
}
