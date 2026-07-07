use super::*;
use crate::config::Config;
use crate::search::{exa, searxng, tavily};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn format_results_empty_input_returns_empty_string() {
    let s = format_results(&[], 5);
    assert!(s.is_empty());
}

#[test]
fn format_results_numbers_and_includes_title_url_content() {
    let r = vec![
        SearchResult {
            title: "T1".into(),
            url: "https://a".into(),
            content: "c1".into(),
        },
        SearchResult {
            title: "T2".into(),
            url: "https://b".into(),
            content: "c2".into(),
        },
    ];
    let s = format_results(&r, 10);
    assert!(s.contains("1. T1"));
    assert!(s.contains("https://a"));
    assert!(s.contains("2. T2"));
    assert!(s.contains("https://b"));
}

#[test]
fn format_results_respects_max() {
    let r = vec![
        SearchResult {
            title: "A".into(),
            url: "u".into(),
            content: "c".into(),
        },
        SearchResult {
            title: "B".into(),
            url: "u".into(),
            content: "c".into(),
        },
        SearchResult {
            title: "C".into(),
            url: "u".into(),
            content: "c".into(),
        },
    ];
    let s = format_results(&r, 2);
    assert!(s.contains("1. A"));
    assert!(s.contains("2. B"));
    assert!(!s.contains("3. C"));
}

#[test]
fn format_results_truncates_at_max_output_bytes() {
    let big = "x".repeat(1000);
    let r = (0..20)
        .map(|i| SearchResult {
            title: format!("Title {i}"),
            url: format!("https://example.com/{i}"),
            content: big.clone(),
        })
        .collect::<Vec<_>>();
    let s = format_results(&r, 20);
    assert!(
        s.len() <= MAX_OUTPUT_BYTES,
        "Output must respect MAX_OUTPUT_BYTES cap"
    );
}

/// Returns a config with Brave disabled so fallback providers are exercised.
fn cfg_no_brave_with(tavily: Option<&str>, exa: Option<&str>, searxng: Option<&str>) -> Config {
    let mut c = Config::from_env().expect("Config::from_env failed");
    c.brave_public_search_enabled = false;
    c.tavily_api_key = tavily.map(String::from);
    c.exa_api_key = exa.map(String::from);
    c.searxng_url = searxng.map(String::from);
    c
}

#[test]
fn from_config_prefers_brave_by_default() {
    let mut cfg = Config::from_env().expect("Config::from_env failed");
    cfg.brave_public_search_enabled = true;
    cfg.tavily_api_key = None;
    cfg.exa_api_key = None;
    cfg.searxng_url = None;
    let p = from_config(&cfg).unwrap();
    assert_eq!(p.name(), "brave");
}

#[test]
fn from_config_prefers_brave_even_when_others_set() {
    let mut cfg = Config::from_env().expect("Config::from_env failed");
    cfg.brave_public_search_enabled = true;
    cfg.tavily_api_key = Some("tv".into());
    cfg.exa_api_key = Some("ex".into());
    cfg.searxng_url = Some("http://sx".into());
    let p = from_config(&cfg).unwrap();
    assert_eq!(p.name(), "brave");
}

#[test]
fn from_config_prefers_tavily_when_brave_disabled() {
    let cfg = cfg_no_brave_with(Some("tv"), Some("ex"), Some("http://sx"));
    let p = from_config(&cfg).unwrap();
    assert_eq!(p.name(), "tavily");
}

#[test]
fn from_config_falls_back_to_exa_when_no_tavily() {
    let cfg = cfg_no_brave_with(None, Some("ex"), Some("http://sx"));
    let p = from_config(&cfg).unwrap();
    assert_eq!(p.name(), "exa");
}

#[test]
fn from_config_falls_back_to_searxng_when_no_native_key() {
    let cfg = cfg_no_brave_with(None, None, Some("http://sx"));
    let p = from_config(&cfg).unwrap();
    assert_eq!(p.name(), "searxng");
}

#[test]
fn from_config_returns_none_when_nothing_configured() {
    let cfg = cfg_no_brave_with(None, None, None);
    assert!(from_config(&cfg).is_none());
}

#[test]
fn from_config_ignores_empty_tavily_key() {
    let cfg = cfg_no_brave_with(Some(""), Some("ex"), None);
    let p = from_config(&cfg).unwrap();
    assert_eq!(p.name(), "exa");
}

#[tokio::test]
async fn tavily_empty_query_short_circuits_with_error() {
    let p = tavily::TavilyProvider::new("key", 0);
    assert_eq!(p.search("", 5).await, "Error: no search query provided.");
    assert_eq!(p.name(), "tavily");
}

#[tokio::test]
async fn exa_empty_query_short_circuits() {
    let p = exa::ExaProvider::new("k");
    assert_eq!(p.search("", 3).await, "Error: no search query provided.");
    assert_eq!(p.name(), "exa");
}

#[tokio::test]
async fn searxng_empty_query_short_circuits() {
    let p = searxng::SearXngProvider::new("http://example.com".into(), "".into());
    assert_eq!(p.search("", 3).await, "Error: no search query provided.");
    assert_eq!(p.name(), "searxng");
}

#[tokio::test]
async fn searxng_sends_authorization_header_when_secret_set() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("format", "json"))
        .and(header("Authorization", "Bearer mysecret"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"results":[]}"#))
        .expect(1)
        .mount(&server)
        .await;
    let p = searxng::SearXngProvider::new(server.uri(), "mysecret".into());
    assert_eq!(p.search("rust", 3).await, "No results found.");
}

#[tokio::test]
async fn searxng_parses_results_into_format() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"results":[
                {"title":"S1","url":"https://s1","content":"sc1"},
                {"title":"S2","url":"https://s2","content":"sc2"}
            ]}"#,
        ))
        .mount(&server)
        .await;
    let p = searxng::SearXngProvider::new(server.uri(), "".into());
    let out = p.search("rust async", 5).await;
    assert!(out.contains("1. S1"));
    assert!(out.contains("https://s1"));
    assert!(out.contains("2. S2"));
}

#[tokio::test]
async fn searxng_connection_failure_surfaces_as_error() {
    let p = searxng::SearXngProvider::new("http://127.0.0.1:1".into(), "".into());
    let out: String = p.search("rust", 3).await;
    assert!(out.starts_with("Error:"), "got: {out}");
}
