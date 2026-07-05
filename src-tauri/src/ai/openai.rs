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
    is_local: bool,
}

impl OpenAiCompat {
    pub fn openai(cfg: &ProviderConfig) -> Self {
        OpenAiCompat {
            base: base_or(cfg, "https://api.openai.com/v1"),
            is_openrouter: false,
            is_local: false,
        }
    }
    pub fn openrouter(cfg: &ProviderConfig) -> Self {
        OpenAiCompat {
            base: base_or(cfg, "https://openrouter.ai/api/v1"),
            is_openrouter: true,
            is_local: false,
        }
    }
    pub fn local(cfg: &ProviderConfig) -> Self {
        // Ollama's OpenAI-compatible endpoint; LM Studio uses :1234/v1.
        OpenAiCompat {
            base: base_or(cfg, "http://localhost:11434/v1"),
            is_openrouter: false,
            is_local: true,
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

impl OpenAiCompat {
    /// Try Ollama's native `/api/chat`. Unlike the OpenAI-compat `/v1` shim, it
    /// honours `think` fully — with thinking off a reasoning model (qwen3, …)
    /// emits no reasoning at all, so responses are fast and never truncated by a
    /// reasoning blow-out. Returns `None` on any failure (e.g. the backend is LM
    /// Studio, which has no `/api/chat`) so the caller falls back to `/v1`.
    async fn ollama_chat(
        &self,
        client: &reqwest::Client,
        sys: &str,
        user: &str,
        model: &str,
        temperature: f32,
        max_tokens: u32,
        thinking: Option<bool>,
    ) -> Option<String> {
        let root = self.base.strip_suffix("/v1").unwrap_or(&self.base).trim_end_matches('/');
        let url = format!("{root}/api/chat");
        let mut body = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": sys },
                { "role": "user", "content": user },
            ],
            "stream": false,
            "options": { "temperature": temperature, "num_predict": max_tokens },
        });
        if let Some(think) = thinking {
            body["think"] = json!(think);
        }
        let resp = client.post(&url).json(&body).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let v: serde_json::Value = resp.json().await.ok()?;
        v["message"]["content"].as_str().map(str::to_string)
    }
}

#[async_trait]
impl TranslationProvider for OpenAiCompat {
    async fn translate_batch(
        &self,
        client: &reqwest::Client,
        key: Option<&str>,
        req: &BatchReq,
    ) -> Result<Vec<String>> {
        let (sys, mut user) = build_messages(req);
        // Reasoning local models (e.g. Ollama qwen3) keep "thinking" even with
        // thinking off over the OpenAI-compat endpoint — the reasoning is counted
        // against max_tokens and can consume the whole budget before the answer.
        // The `/no_think` soft switch curbs it on the /v1 fallback and is harmless
        // to non-reasoning models / LM Studio (just extra text).
        if self.is_local && req.thinking == Some(false) {
            user.push_str(" /no_think");
        }

        // Local: prefer Ollama's native /api/chat, where `think:false` truly
        // disables reasoning (fast, no wasted tokens). Falls through to /v1 when
        // it isn't Ollama (e.g. LM Studio) or the call fails.
        if self.is_local {
            if let Some(content) = self
                .ollama_chat(client, &sys, &user, &req.model, req.temperature, req.max_tokens, req.thinking)
                .await
            {
                return parse_batch_response(&content, req.items.len());
            }
        }

        let url = format!("{}/chat/completions", self.base);
        let mut body = json!({
            "model": req.model,
            "messages": [
                { "role": "system", "content": sys },
                { "role": "user", "content": user },
            ],
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
        });
        // `think` is an Ollama extension; only send it to Local so strict cloud
        // APIs (OpenAI) don't 400 on an unknown field.
        if self.is_local {
            if let Some(think) = req.thinking {
                body["think"] = json!(think);
            }
        }
        // OpenRouter: turn reasoning off when thinking is disabled — a real speed
        // win on hybrid models, and the default gpt-4o-mini ignores it. (Pure
        // reasoning models like deepseek-r1 reject this; reasoning is mandatory
        // there, so those keep reasoning regardless of the toggle.)
        if self.is_openrouter && req.thinking == Some(false) {
            body["reasoning"] = json!({ "enabled": false });
        }

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

    async fn complete(
        &self,
        client: &reqwest::Client,
        key: Option<&str>,
        system: &str,
        user: &str,
        model: &str,
        max_tokens: u32,
    ) -> Result<String> {
        // Mining doesn't need reasoning; ask reasoning models to skip it for
        // speed and so the token budget goes to the answer, not the thoughts.
        let user_no_think;
        let user = if self.is_local {
            user_no_think = format!("{user} /no_think");
            &user_no_think
        } else {
            user
        };

        // Local: prefer Ollama's native /api/chat (think:false truly disables
        // reasoning), falling through to /v1 for LM Studio or on failure.
        if self.is_local {
            if let Some(content) = self
                .ollama_chat(client, system, user, model, 0.2, max_tokens, Some(false))
                .await
            {
                return Ok(content);
            }
        }

        let url = format!("{}/chat/completions", self.base);
        let mut body = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "temperature": 0.2,
            "max_tokens": max_tokens,
        });
        if self.is_local {
            body["think"] = json!(false);
        }
        if self.is_openrouter {
            body["reasoning"] = json!({ "enabled": false });
        }

        with_retry(4, 800, || async {
            let mut rb = client.post(&url).json(&body);
            if let Some(k) = key {
                rb = rb.bearer_auth(k);
            }
            if self.is_openrouter {
                rb = rb
                    .header("HTTP-Referer", "https://github.com/rpgtl")
                    .header("X-Title", "RPGMaker Translator");
            }
            let resp = rb.send().await.map_err(|e| CallError::Retryable(e.into()))?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| CallError::Retryable(e.into()))?;
            if status.is_success() {
                let v: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| CallError::Fatal(e.into()))?;
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
        .await
    }
}
