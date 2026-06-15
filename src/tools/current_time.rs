use async_trait::async_trait;
use chrono::Local;

use super::Tool;

pub struct CurrentTimeTool;

/// Returns true when the user is explicitly asking for the current time, date,
/// day or hour. Used by the pipeline to force a `current_time` tool call so the
/// model never answers from stale context or hallucinates a date.
pub fn is_explicit_time_request(text: &str) -> bool {
    let lower = text.to_lowercase();
    let spanish = [
        "qué hora es",
        "que hora es",
        "dime la hora",
        "la hora actual",
        "hora actual",
        "qué día es",
        "que dia es",
        "qué día es hoy",
        "que dia es hoy",
        "día de hoy",
        "dia de hoy",
        "qué fecha es",
        "que fecha es",
        "qué fecha es hoy",
        "que fecha es hoy",
        "fecha actual",
        "fecha de hoy",
    ];
    let english = [
        "what time is it",
        "what's the time",
        "whats the time",
        "tell me the time",
        "current time",
        "what day is it",
        "what's today",
        "whats today",
        "today is what day",
        "today's date",
        "todays date",
        "what date is it",
        "current date",
        "current day",
    ];

    spanish.iter().any(|p| lower.contains(p)) || english.iter().any(|p| lower.contains(p))
}

#[async_trait]
impl Tool for CurrentTimeTool {
    fn name(&self) -> &str {
        "current_time"
    }

    fn description(&self) -> &str {
        "Returns the current local date and time. \
         MUST be called EVERY TIME the user explicitly asks for the current time, date, day or hour. \
         Do not answer from memory, cached context or general knowledge; always call this tool."
    }

    fn should_force_for(&self, query: &str) -> bool {
        is_explicit_time_request(query)
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn run(&self, _args: &str) -> String {
        Local::now().format("%H:%M:%S, %A %d %B %Y").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_current_time() {
        assert_eq!(CurrentTimeTool.name(), "current_time");
    }

    #[test]
    fn description_mentions_mandatory_calling() {
        let desc = CurrentTimeTool.description();
        assert!(desc.contains("current"));
        assert!(desc.contains("MUST"));
        assert!(desc.contains("EVERY TIME"));
    }

    #[tokio::test]
    async fn run_returns_non_empty_string() {
        let result = CurrentTimeTool.run("").await;
        assert!(!result.is_empty());
    }

    #[tokio::test]
    async fn run_output_matches_format() {
        // Expected: "HH:MM:SS, Weekday DD Month YYYY"
        // Example:  "14:05:32, Saturday 08 March 2025"
        let result = CurrentTimeTool.run("").await;
        let parts: Vec<&str> = result.splitn(2, ", ").collect();
        assert_eq!(
            parts.len(),
            2,
            "output must contain ', ' separator: {result:?}"
        );

        // Time part: HH:MM:SS
        let time = parts[0];
        let time_parts: Vec<&str> = time.split(':').collect();
        assert_eq!(time_parts.len(), 3, "time must be HH:MM:SS: {time:?}");
        let h: u32 = time_parts[0].parse().expect("hours must be numeric");
        let m: u32 = time_parts[1].parse().expect("minutes must be numeric");
        let s: u32 = time_parts[2].parse().expect("seconds must be numeric");
        assert!(h < 24, "hours out of range: {h}");
        assert!(m < 60, "minutes out of range: {m}");
        assert!(s < 60, "seconds out of range: {s}");

        // Date part: "Weekday DD Month YYYY"
        let date = parts[1];
        let date_parts: Vec<&str> = date.split_whitespace().collect();
        assert_eq!(date_parts.len(), 4, "date must have 4 parts: {date:?}");
        let day: u32 = date_parts[1].parse().expect("day must be numeric");
        let year: u32 = date_parts[3].parse().expect("year must be numeric");
        assert!((1..=31).contains(&day), "day out of range: {day}");
        assert!(year >= 2024, "year seems wrong: {year}");
    }

    #[tokio::test]
    async fn run_output_is_consistent_within_same_second() {
        let r1 = CurrentTimeTool.run("").await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let r2 = CurrentTimeTool.run("").await;
        assert_eq!(r1, r2);
    }

    #[test]
    fn detects_explicit_time_requests() {
        assert!(is_explicit_time_request("¿Qué hora es?"));
        assert!(is_explicit_time_request("que hora es"));
        assert!(is_explicit_time_request("Dime la hora actual"));
        assert!(is_explicit_time_request("¿Qué día es hoy?"));
        assert!(is_explicit_time_request("¿Qué fecha es?"));
        assert!(is_explicit_time_request("What time is it?"));
        assert!(is_explicit_time_request("What's the time?"));
        assert!(is_explicit_time_request("Tell me today's date"));
        assert!(is_explicit_time_request("Current day"));
    }

    #[test]
    fn ignores_non_time_requests() {
        assert!(!is_explicit_time_request("Hola, ¿cómo estás?"));
        assert!(!is_explicit_time_request("Cuéntame un chiste"));
        assert!(!is_explicit_time_request("What time does the movie start?"));
        assert!(!is_explicit_time_request("I had a great time yesterday"));
    }
}
