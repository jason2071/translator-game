//! QA additions: AI resilience layer — batch/split fallback and retry/backoff.
//! Uses an in-process MockProvider (no network).

use anyhow::{anyhow, Result};
use app_lib::ai::retry::{with_retry, CallError};
use app_lib::ai::{translate_batch_or_split, BatchItem, BatchReq, TranslationProvider};
use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};

/// A provider that can be told to fail whole batches (len > 1) and/or fail a
/// specific item, so we can exercise the split-and-retry fallback.
struct MockProvider {
    fail_multi: bool,
    bad: &'static str,
}

#[async_trait]
impl TranslationProvider for MockProvider {
    async fn translate_batch(
        &self,
        _client: &reqwest::Client,
        _key: Option<&str>,
        req: &BatchReq,
    ) -> Result<Vec<String>> {
        if self.fail_multi && req.items.len() > 1 {
            return Err(anyhow!("batch failed"));
        }
        let mut out = Vec::new();
        for it in &req.items {
            if it.text == self.bad {
                return Err(anyhow!("bad item"));
            }
            out.push(format!("T[{}]", it.text));
        }
        Ok(out)
    }

    async fn complete(
        &self,
        _client: &reqwest::Client,
        _key: Option<&str>,
        _system: &str,
        user: &str,
        _model: &str,
        _max_tokens: u32,
    ) -> Result<String> {
        Ok(format!("C[{user}]"))
    }
}

fn req(texts: &[&str]) -> BatchReq {
    BatchReq {
        items: texts
            .iter()
            .enumerate()
            .map(|(i, t)| BatchItem {
                id: i as i64,
                text: (*t).to_string(),
                context: None,
                neighbors: None,
            })
            .collect(),
        glossary: vec![],
        source_lang: "English".into(),
        target_lang: "Thai".into(),
        tone: "casual".into(),
        extra_system: None,
        model: "mock".into(),
        temperature: 0.0,
        max_tokens: 100,
        thinking: None,
    }
}

#[tokio::test]
async fn split_fallback_recovers_every_item() {
    let p = MockProvider {
        fail_multi: true,
        bad: "\u{0}",
    };
    let client = reqwest::Client::new();
    let out = translate_batch_or_split(&p, &client, None, &req(&["a", "b", "c"])).await;
    assert_eq!(
        out,
        vec![
            Some("T[a]".into()),
            Some("T[b]".into()),
            Some("T[c]".into())
        ]
    );
}

#[tokio::test]
async fn split_isolates_the_bad_item() {
    // Whole batch fails, and on the per-item retry only "b" fails.
    let p = MockProvider {
        fail_multi: true,
        bad: "b",
    };
    let client = reqwest::Client::new();
    let out = translate_batch_or_split(&p, &client, None, &req(&["a", "b", "c"])).await;
    assert_eq!(out, vec![Some("T[a]".into()), None, Some("T[c]".into())]);
}

#[tokio::test]
async fn single_item_failure_yields_none() {
    let p = MockProvider {
        fail_multi: false,
        bad: "x",
    };
    let client = reqwest::Client::new();
    let out = translate_batch_or_split(&p, &client, None, &req(&["x"])).await;
    assert_eq!(out, vec![None]);
}

#[tokio::test]
async fn happy_multi_passes_through() {
    let p = MockProvider {
        fail_multi: false,
        bad: "\u{0}",
    };
    let client = reqwest::Client::new();
    let out = translate_batch_or_split(&p, &client, None, &req(&["a", "b"])).await;
    assert_eq!(out, vec![Some("T[a]".into()), Some("T[b]".into())]);
}

struct WrongLanguageProvider {
    split: bool,
    calls: AtomicU32,
}

#[async_trait]
impl TranslationProvider for WrongLanguageProvider {
    async fn translate_batch(
        &self,
        _: &reqwest::Client,
        _: Option<&str>,
        req: &BatchReq,
    ) -> Result<Vec<String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.split && req.items.len() > 1 {
            return Err(anyhow!("force single-item fallback"));
        }
        Ok(req
            .items
            .iter()
            .map(|item| match item.text.as_str() {
                "Strongest equipment" => "最强装备".into(),
                "Mixed" => "คำแปล蜕ปน".into(),
                _ => "พลังชีวิต".into(),
            })
            .collect())
    }

    async fn complete(
        &self,
        _: &reqwest::Client,
        _: Option<&str>,
        _: &str,
        _: &str,
        _: &str,
        _: u32,
    ) -> Result<String> {
        unreachable!("batch test")
    }
}

#[tokio::test]
async fn rejects_wrong_language_per_item_in_batch_and_split_without_rebilling_good_items() {
    for split in [false, true] {
        let provider = WrongLanguageProvider {
            split,
            calls: AtomicU32::new(0),
        };
        let output = translate_batch_or_split(
            &provider,
            &reqwest::Client::new(),
            None,
            &req(&["HP", "Strongest equipment", "Mixed"]),
        )
        .await;
        assert_eq!(output, vec![Some("พลังชีวิต".into()), None, None]);
        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            if split { 4 } else { 1 }
        );
    }
}

#[tokio::test]
async fn cjk_target_output_is_not_rejected_by_the_thai_guard() {
    let provider = WrongLanguageProvider {
        split: false,
        calls: AtomicU32::new(0),
    };
    let mut request = req(&["Strongest equipment"]);
    request.target_lang = "Chinese".into();
    assert_eq!(
        translate_batch_or_split(&provider, &reqwest::Client::new(), None, &request).await,
        vec![Some("最强装备".into())]
    );
}

// ---- retry/backoff ----

#[tokio::test]
async fn retry_recovers_after_transient_failures() {
    let n = AtomicU32::new(0);
    let r: Result<u32> = with_retry(5, 0, || async {
        let attempt = n.fetch_add(1, Ordering::SeqCst);
        if attempt < 2 {
            Err(CallError::Retryable(anyhow!("429")))
        } else {
            Ok(attempt)
        }
    })
    .await;
    assert_eq!(r.unwrap(), 2);
    assert_eq!(n.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retry_gives_up_immediately_on_fatal() {
    let n = AtomicU32::new(0);
    let r: Result<u32> = with_retry(5, 0, || async {
        n.fetch_add(1, Ordering::SeqCst);
        Err(CallError::Fatal(anyhow!("401 unauthorized")))
    })
    .await;
    assert!(r.is_err());
    assert_eq!(n.load(Ordering::SeqCst), 1, "fatal must not retry");
}

#[tokio::test]
async fn retry_exhausts_max_tries() {
    let n = AtomicU32::new(0);
    let r: Result<u32> = with_retry(3, 0, || async {
        n.fetch_add(1, Ordering::SeqCst);
        Err(CallError::Retryable(anyhow!("500")))
    })
    .await;
    assert!(r.is_err());
    assert_eq!(n.load(Ordering::SeqCst), 3);
}
