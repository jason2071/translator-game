//! OpenAI-compatible chat provider — serves OpenAI, OpenRouter, Local
//! (Ollama / LM Studio), and Ollama Cloud backends.

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
    is_ollama_cloud: bool,
}

impl OpenAiCompat {
    pub fn openai(cfg: &ProviderConfig) -> Self {
        OpenAiCompat {
            base: base_or(cfg, "https://api.openai.com/v1"),
            is_openrouter: false,
            is_local: false,
            is_ollama_cloud: false,
        }
    }
    pub fn openrouter(cfg: &ProviderConfig) -> Self {
        OpenAiCompat {
            base: base_or(cfg, "https://openrouter.ai/api/v1"),
            is_openrouter: true,
            is_local: false,
            is_ollama_cloud: false,
        }
    }
    pub fn local(cfg: &ProviderConfig) -> Self {
        // Ollama's OpenAI-compatible endpoint; LM Studio uses :1234/v1.
        OpenAiCompat {
            base: base_or(cfg, "http://localhost:11434/v1"),
            is_openrouter: false,
            is_local: true,
            is_ollama_cloud: false,
        }
    }
    pub fn ollama_cloud(cfg: &ProviderConfig) -> Self {
        OpenAiCompat {
            base: base_or(cfg, "https://ollama.com"),
            is_openrouter: false,
            is_local: false,
            is_ollama_cloud: true,
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
        key: Option<&str>,
        sys: &str,
        user: &str,
        model: &str,
        temperature: f32,
        max_tokens: u32,
        thinking: Option<bool>,
    ) -> Result<Option<String>> {
        let root = self
            .base
            .strip_suffix("/v1")
            .unwrap_or(&self.base)
            .trim_end_matches('/');
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
        let call = || async {
            let mut request = client.post(&url).json(&body);
            if let Some(key) = key {
                request = request.bearer_auth(key);
            }
            let response = request
                .send()
                .await
                .map_err(|e| CallError::Retryable(e.into()))?;
            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|e| CallError::Retryable(e.into()))?;
            if status.is_success() {
                let value: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| CallError::Fatal(e.into()))?;
                value["message"]["content"]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| CallError::Fatal(anyhow!("unexpected response: {text}")))
            } else if status_is_retryable(status.as_u16()) {
                Err(CallError::Retryable(anyhow!("{status}: {text}")))
            } else {
                Err(CallError::Fatal(anyhow!("{status}: {text}")))
            }
        };

        if self.is_ollama_cloud {
            return with_retry(4, 800, call).await.map(Some);
        }
        Ok(call().await.ok())
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
        if self.is_local || self.is_ollama_cloud {
            if let Some(content) = self
                .ollama_chat(
                    client,
                    key,
                    &sys,
                    &user,
                    &req.model,
                    req.temperature,
                    req.max_tokens,
                    req.thinking,
                )
                .await?
            {
                return parse_batch_response(&content, req.items.len());
            }
        }

        if self.is_ollama_cloud {
            return Err(anyhow!("Ollama Cloud returned no chat content"));
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
        if self.is_local || self.is_ollama_cloud {
            if let Some(content) = self
                .ollama_chat(
                    client,
                    key,
                    system,
                    user,
                    model,
                    0.2,
                    max_tokens,
                    Some(false),
                )
                .await?
            {
                return Ok(content);
            }
        }

        if self.is_ollama_cloud {
            return Err(anyhow!("Ollama Cloud returned no chat content"));
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

#[cfg(test)]
mod tests {
    use super::OpenAiCompat;
    use crate::ai::ProviderConfig;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 1024];
        let (header_end, content_len) = loop {
            let count = stream.read(&mut chunk).expect("read request");
            assert!(count > 0, "client closed before sending a complete request");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = end + 4;
                let headers = std::str::from_utf8(&bytes[..header_end]).expect("UTF-8 headers");
                let content_len = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .expect("content length")
                    .parse::<usize>()
                    .expect("numeric content length");
                break (header_end, content_len);
            }
        };
        while bytes.len() < header_end + content_len {
            let count = stream.read(&mut chunk).expect("read request body");
            assert!(count > 0, "client closed before sending the complete body");
            bytes.extend_from_slice(&chunk[..count]);
        }
        String::from_utf8(bytes).expect("UTF-8 request")
    }

    #[tokio::test]
    async fn ollama_cloud_uses_bearer_auth_and_native_chat_api() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept client");
            let request = read_request(&mut stream);
            let body = r#"{"message":{"content":"translated"}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write response");
            sender.send(request).expect("send captured request");
        });

        let cfg = ProviderConfig {
            kind: "ollama".into(),
            base_url: Some(format!("http://{address}")),
            model: "gpt-oss:120b".into(),
            temperature: None,
            max_tokens: None,
            batch_size: None,
            rpm: None,
            concurrency: None,
            tone: None,
            system_prompt: None,
            thinking: None,
        };
        let provider = OpenAiCompat::ollama_cloud(&cfg);
        let content = provider
            .ollama_chat(
                &reqwest::Client::new(),
                Some("ollama-test-key"),
                "system prompt",
                "user prompt",
                "gpt-oss:120b",
                0.25,
                512,
                Some(false),
            )
            .await
            .expect("successful cloud response");
        assert_eq!(content.as_deref(), Some("translated"));

        let request = receiver.recv().expect("captured request");
        assert!(request.starts_with("POST /api/chat HTTP/1.1\r\n"));
        assert!(request.contains("authorization: Bearer ollama-test-key\r\n"));
        let body = request.split_once("\r\n\r\n").expect("request body").1;
        let body: serde_json::Value = serde_json::from_str(body).expect("JSON body");
        assert_eq!(body["model"], "gpt-oss:120b");
        assert_eq!(body["stream"], false);
        assert_eq!(body["think"], false);
        assert_eq!(body["options"]["temperature"], 0.25);
        assert_eq!(body["options"]["num_predict"], 512);
    }
}
