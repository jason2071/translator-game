//! Claude provider — Anthropic `/v1/messages` API.

use super::prompt::{build_messages, parse_batch_response};
use super::retry::{status_is_retryable, with_retry, CallError};
use super::{BatchReq, ProviderConfig, TranslationProvider};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;

pub struct Anthropic {
    base: String,
}

impl Anthropic {
    pub fn new(cfg: &ProviderConfig) -> Self {
        let base = cfg
            .base_url
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string())
            .trim_end_matches('/')
            .to_string();
        Anthropic { base }
    }
}

#[async_trait]
impl TranslationProvider for Anthropic {
    async fn translate_batch(
        &self,
        client: &reqwest::Client,
        key: Option<&str>,
        req: &BatchReq,
    ) -> Result<Vec<String>> {
        let (sys, user) = build_messages(req);
        let url = format!("{}/v1/messages", self.base);
        let key = key.ok_or_else(|| anyhow!("Anthropic requires an API key"))?;
        let body = json!({
            "model": req.model,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "system": sys,
            "messages": [ { "role": "user", "content": user } ],
        });

        let content = with_retry(4, 800, || async {
            let resp = client
                .post(&url)
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
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
                // content is an array of blocks; concatenate text blocks.
                let joined: String = v["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b["text"].as_str())
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

    async fn complete(
        &self,
        client: &reqwest::Client,
        key: Option<&str>,
        system: &str,
        user: &str,
        model: &str,
        max_tokens: u32,
    ) -> Result<String> {
        let url = format!("{}/v1/messages", self.base);
        let key = key.ok_or_else(|| anyhow!("Anthropic requires an API key"))?;
        let body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "temperature": 0.2,
            "system": system,
            "messages": [ { "role": "user", "content": user } ],
        });

        with_retry(4, 800, || async {
            let resp = client
                .post(&url)
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
                .map_err(|e| CallError::Retryable(e.into()))?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| CallError::Retryable(e.into()))?;
            if status.is_success() {
                let v: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| CallError::Fatal(e.into()))?;
                let joined: String = v["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b["text"].as_str())
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
        .await
    }
}
