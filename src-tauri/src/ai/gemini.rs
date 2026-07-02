//! Gemini provider — Google Generative Language `generateContent` API.

use super::prompt::{build_messages, parse_batch_response};
use super::retry::{status_is_retryable, with_retry, CallError};
use super::{BatchReq, ProviderConfig, TranslationProvider};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;

pub struct Gemini {
    base: String,
}

impl Gemini {
    pub fn new(cfg: &ProviderConfig) -> Self {
        let base = cfg
            .base_url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string())
            .trim_end_matches('/')
            .to_string();
        Gemini { base }
    }
}

#[async_trait]
impl TranslationProvider for Gemini {
    async fn translate_batch(
        &self,
        client: &reqwest::Client,
        key: Option<&str>,
        req: &BatchReq,
    ) -> Result<Vec<String>> {
        let (sys, user) = build_messages(req);
        let key = key.ok_or_else(|| anyhow!("Gemini requires an API key"))?;
        // Key is passed as a query param; model is part of the path.
        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base, req.model, key
        );
        let body = json!({
            "systemInstruction": { "parts": [ { "text": sys } ] },
            "contents": [ { "role": "user", "parts": [ { "text": user } ] } ],
            "generationConfig": {
                "temperature": req.temperature,
                "maxOutputTokens": req.max_tokens,
            },
        });

        let content = with_retry(4, 800, || async {
            let resp = client
                .post(&url)
                .json(&body)
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
                let joined: String = v["candidates"][0]["content"]["parts"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default();
                if joined.is_empty() {
                    Err(CallError::Fatal(anyhow!("empty response: {text}")))
                } else {
                    Ok(joined)
                }
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
