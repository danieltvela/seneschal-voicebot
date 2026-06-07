use super::*;
use std::time::Duration;

fn entry(key: &'static str, value: &str, ttl_ms: u64) -> ContextEntry {
    ContextEntry {
        key,
        value: value.to_string(),
        confidence: 1.0,
        valid_until: Instant::now() + Duration::from_millis(ttl_ms),
        source: "test",
    }
}

#[test]
fn fresh_lens_returns_none_for_missing_key() {
    let lens = ContextLens::new();
    assert!(lens.get("anything").is_none());
    assert!(lens.format_for_llm().is_none());
}

#[test]
fn upsert_then_get_returns_value() {
    let mut lens = ContextLens::new();
    lens.upsert(entry("k", "v", 60_000));
    assert_eq!(lens.get("k").unwrap().value, "v");
}

#[test]
fn get_returns_none_after_ttl_expires() {
    let mut lens = ContextLens::new();
    lens.upsert(entry("k", "v", 5));
    std::thread::sleep(Duration::from_millis(20));
    assert!(
        lens.get("k").is_none(),
        "Expired entries must not be returned"
    );
}

#[test]
fn upsert_replaces_existing_entry() {
    let mut lens = ContextLens::new();
    lens.upsert(entry("k", "first", 60_000));
    lens.upsert(entry("k", "second", 60_000));
    assert_eq!(lens.get("k").unwrap().value, "second");
    assert_eq!(lens.format_for_llm().unwrap().matches("second").count(), 1);
}

#[test]
fn purge_expired_drops_only_expired_entries() {
    let mut lens = ContextLens::new();
    lens.upsert(entry("alive", "yes", 60_000));
    lens.upsert(entry("dead", "no", 5));
    std::thread::sleep(Duration::from_millis(20));
    lens.purge_expired();
    assert!(lens.get("alive").is_some());
    assert!(lens.get("dead").is_none());
}

#[test]
fn format_for_llm_includes_only_fresh_entries() {
    let mut lens = ContextLens::new();
    lens.upsert(entry("a", "alpha", 60_000));
    lens.upsert(entry("b", "beta", 5));
    std::thread::sleep(Duration::from_millis(20));
    let out = lens.format_for_llm().unwrap();
    assert!(out.contains("alpha"));
    assert!(!out.contains("beta"));
    assert!(out.starts_with("\n[Analysis Context]\n"));
}

#[test]
fn default_impl_matches_new() {
    let a = ContextLens::default();
    assert!(a.get("anything").is_none());
}
