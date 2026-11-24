//! Thin REST client for the daemon Settings API (issue #33).
//!
//! The shell keeps wire models generic (`serde_json::Value`) because the
//! Control API is the boundary; the webview owns the typed Settings form.

use serde_json::Value;
use std::time::Duration;

const SETTINGS_TIMEOUT: Duration = Duration::from_secs(10);

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(SETTINGS_TIMEOUT)
        .build()
        .expect("failed to build settings HTTP client")
}

pub async fn get_config(base_url: &str) -> anyhow::Result<Value> {
    let response = client()
        .get(format!("{base_url}/control/config"))
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

pub async fn validate_config(base_url: &str, patch: &Value) -> anyhow::Result<Value> {
    let response = client()
        .post(format!("{base_url}/control/config/validate"))
        .json(patch)
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

pub async fn apply_config(base_url: &str, patch: &Value) -> anyhow::Result<Value> {
    let response = client()
        .post(format!("{base_url}/control/config/apply"))
        .json(patch)
        .send()
        .await?;

    let status = response.status();
    let body: Value = response.json().await?;
    if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
        return Err(anyhow::anyhow!(settings_error_message(&body)));
    }
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "Settings apply failed (HTTP {}): {}",
            status.as_u16(),
            settings_error_message(&body)
        ));
    }
    Ok(body)
}

pub async fn get_onboarding_defaults(base_url: &str) -> anyhow::Result<Value> {
    let response = client()
        .get(format!("{base_url}/control/onboarding/defaults"))
        .send()
        .await?
        .error_for_status()?;
    Ok(response.json().await?)
}

fn settings_error_message(body: &Value) -> String {
    let errors = body
        .get("errors")
        .and_then(|errors| errors.as_array())
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| {
                    let field = error.get("field")?.as_str()?;
                    let message = error.get("message")?.as_str()?;
                    Some(format!("{field}: {message}"))
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    if errors.is_empty() {
        body.to_string()
    } else {
        errors
    }
}
