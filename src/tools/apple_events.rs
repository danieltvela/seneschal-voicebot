use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use super::Tool;

/// Maximum reminders to enumerate via AppleScript before truncating.
/// Reminders supports fast batch property access via range (`reminders 1 thru N`).
const MAX_REMINDERS: i32 = 50;
/// Maximum events to enumerate. Calendar AppleScript is extremely slow for batch
/// property access on large calendars (5000+ events), so we use a conservative
/// limit that works reliably across all calendar sizes.
const MAX_EVENTS: i32 = 5;
/// Timeout for each osascript invocation.
const OSA_TIMEOUT: Duration = Duration::from_secs(15);

const USAGE: &str = "Valid operations:\n\
  Calendar:\n\
    list_calendars                         — list all calendar names\n\
    list_events    calendar, from, to      — list events in date range\n\
    create_event   calendar, title, start, end [, location, notes]\n\
    delete_event   calendar, title         — delete event by title\n\
  Reminders:\n\
    list_reminder_lists                    — list all reminder list names\n\
    list_reminders  [, list]               — list reminders (optionally by list)\n\
    create_reminder title [, list, due_date, notes]\n\
    complete_reminder title [, list]       — mark a reminder as completed\n\
    delete_reminder title [, list]\n\
\n\
Date format: ISO 8601 — '2024-01-01T10:00:00'";

pub struct AppleEventsTool;

fn parse_iso_parts(iso: &str) -> Option<(i32, i32, i32, i32, i32)> {
    let s = iso.trim();
    let parts: Vec<&str> = s.splitn(2, 'T').collect();
    if parts.len() != 2 {
        return None;
    }
    let date_components: Vec<&str> = parts[0].splitn(3, '-').collect();
    if date_components.len() != 3 {
        return None;
    }
    let time_components: Vec<&str> = parts[1].splitn(2, ':').collect();
    if time_components.len() < 2 {
        return None;
    }
    Some((
        date_components[0].parse().ok()?,
        date_components[1].parse().ok()?,
        date_components[2].parse().ok()?,
        time_components[0].parse().ok()?,
        time_components[1].parse().ok()?,
    ))
}

fn as_date_script(var: &str, iso: &str) -> String {
    let (y, m, d, h, mi) = parse_iso_parts(iso).unwrap_or((2024, 1, 1, 0, 0));
    format!(
        "set {var} to current date\n\
         set year of {var} to {y}\n\
         set month of {var} to {m}\n\
         set day of {var} to {d}\n\
         set hours of {var} to {h}\n\
         set minutes of {var} to {mi}\n\
         set seconds of {var} to 0\n"
    )
}

async fn osascript(script: &str) -> String {
    let output = tokio::time::timeout(
        OSA_TIMEOUT,
        Command::new("osascript").arg("-e").arg(script).output(),
    )
    .await;
    match output {
        Ok(Ok(out)) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if text.is_empty() {
                "OK".to_string()
            } else {
                text
            }
        }
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if stderr.contains("-1743")
                || stderr.contains("not allowed")
                || stderr.contains("denied")
            {
                "Access denied. Grant Calendar/Reminders access in System Settings → Privacy & Security.".to_string()
            } else if stderr.is_empty() {
                format!("Operation failed with exit code {:?}", out.status.code())
            } else {
                format!("Error: {stderr}")
            }
        }
        Ok(Err(e)) => {
            format!("Failed to run osascript: {e}. This tool requires macOS.")
        }
        Err(_) => {
            "Operation timed out after 15 seconds. The operation may be too slow (e.g. too many reminders to enumerate). Try narrowing the request to a specific list or date range.".to_string()
        }
    }
}

impl AppleEventsTool {
    async fn list_calendars(&self) -> String {
        osascript(
            "tell application \"Calendar\"\n\
             set names to name of every calendar\n\
             set AppleScript's text item delimiters to \"\n\"\n\
             return names as string\n\
             end tell",
        )
        .await
    }

    async fn list_events(&self, params: &serde_json::Value) -> String {
        let calendar = params["calendar"]
            .as_str()
            .filter(|c| !c.is_empty())
            .map(|c| format!("first calendar whose name is \"{c}\""))
            .unwrap_or_else(|| "first calendar".to_string());

        // Calendar AppleScript is fundamentally slow for batch property access on
        // large calendars (5000+ events). Each `property of (events 1 thru N of cal)`
        // call resolves N Apple Event references, and the cost scales with both N
        // and the total calendar size. Testing showed:
        //   - 5104-event calendar: 5 events × 2 properties = OK, 5 × 3 = timeout
        //   - 857-event calendar:  10 events × 3 properties = OK
        //
        // We use a conservative limit (5 events, 2 properties: summary + start date)
        // that works reliably across all calendar sizes. End dates are omitted
        // because they push large calendars over the timeout.
        let script = format!(
            "tell application \"Calendar\"\n\
             set cal to {calendar}\n\
             set totalCount to count of events of cal\n\
             set maxCount to {MAX_EVENTS}\n\
             if totalCount < maxCount then set maxCount to totalCount\n\
             set AppleScript's text item delimiters to linefeed\n\
             set allSummaries to summary of (events 1 thru maxCount of cal)\n\
             set allStarts to start date of (events 1 thru maxCount of cal)\n\
             set output to \"\"\n\
             repeat with i from 1 to maxCount\n\
             set output to output & (item i of allSummaries) & \" | \" & \
             ((item i of allStarts) as string) & \"\n\"\n\
             end repeat\n\
             if output is \"\" then\n\
             set output to \"No events found.\"\n\
             else if totalCount > maxCount then\n\
             set output to output & \"(Showing first \" & maxCount & \" of \" & totalCount & \" events)\"\n\
             end if\n\
             return output\n\
             end tell"
        );
        osascript(&script).await
    }

    async fn create_event(&self, params: &serde_json::Value) -> String {
        let calendar = params["calendar"]
            .as_str()
            .filter(|c| !c.is_empty())
            .map(|c| format!("first calendar whose name is \"{c}\""))
            .unwrap_or_else(|| "first calendar".to_string());
        let title = params["title"].as_str().unwrap_or("Untitled Event");
        let start = params["start"].as_str().unwrap_or("2024-01-01T09:00:00");
        let end = params["end"].as_str().unwrap_or("2024-01-01T10:00:00");
        let location = params["location"].as_str().unwrap_or("");
        let notes = params["notes"].as_str().unwrap_or("");

        let start_script = as_date_script("startDate", start);
        let end_script = as_date_script("endDate", end);

        let props = if !location.is_empty() && !notes.is_empty() {
            format!(
                "summary:\"{title}\", start date:startDate, end date:endDate, \
                 location:\"{location}\", description:\"{notes}\""
            )
        } else if !location.is_empty() {
            format!(
                "summary:\"{title}\", start date:startDate, end date:endDate, \
                 location:\"{location}\""
            )
        } else if !notes.is_empty() {
            format!(
                "summary:\"{title}\", start date:startDate, end date:endDate, \
                 description:\"{notes}\""
            )
        } else {
            format!("summary:\"{title}\", start date:startDate, end date:endDate")
        };

        let script = format!(
            "tell application \"Calendar\"\n\
             set cal to {calendar}\n\
             {start_script}\
             {end_script}\
             make new event at end of events of cal with properties {{{props}}}\n\
             return \"Event created: {title}\"\n\
             end tell"
        );
        osascript(&script).await
    }

    async fn delete_event(&self, params: &serde_json::Value) -> String {
        let calendar = params["calendar"]
            .as_str()
            .filter(|c| !c.is_empty())
            .map(|c| format!("first calendar whose name is \"{c}\""))
            .unwrap_or_else(|| "first calendar".to_string());
        let title = params["title"].as_str().unwrap_or("");
        if title.is_empty() {
            return "Missing 'title' for delete_event.".to_string();
        }
        let script = format!(
            "tell application \"Calendar\"\n\
             set cal to {calendar}\n\
             set targetEvent to first event of cal whose summary is \"{title}\"\n\
             delete targetEvent\n\
             return \"Event deleted: {title}\"\n\
             end tell"
        );
        osascript(&script).await
    }

    async fn list_reminder_lists(&self) -> String {
        osascript(
            "tell application \"Reminders\"\n\
             set names to name of every list\n\
             set AppleScript's text item delimiters to \"\n\"\n\
             return names as string\n\
             end tell",
        )
        .await
    }

    async fn list_reminders(&self, params: &serde_json::Value) -> String {
        let list = params["list"]
            .as_str()
            .filter(|l| !l.is_empty())
            .map(|l| format!("whose name is \"{l}\""))
            .unwrap_or_default();

        let show_completed = params["show_completed"].as_bool().unwrap_or(false);

        // Use range access (`reminders 1 thru N`) + batch property fetch.
        // Iterating with `repeat with r in every reminder` or `item i of matchingReminders`
        // is O(n²) due to per-item Apple Event IPC, which hangs on lists with thousands
        // of reminders. Range access + batch `name of (reminders 1 thru N)` is O(n).
        // The `whose completed is false` filter is also O(n) but with a large constant
        // factor that hangs on 3000+ reminder lists, so we filter in the loop instead.
        let script = format!(
            "tell application \"Reminders\"\n\
             set targetList to first list {list}\n\
             set totalCount to count of reminders of targetList\n\
             set maxCount to {MAX_REMINDERS}\n\
             if totalCount < maxCount then set maxCount to totalCount\n\
             set AppleScript's text item delimiters to linefeed\n\
             set allNames to name of (reminders 1 thru maxCount of targetList)\n\
             set allDue to due date of (reminders 1 thru maxCount of targetList)\n\
             set allCompleted to completed of (reminders 1 thru maxCount of targetList)\n\
             set output to \"\"\n\
             set shown to 0\n\
             repeat with i from 1 to maxCount\n\
             if {show_completed} or (item i of allCompleted) is false then\n\
             set rName to item i of allNames\n\
             set dueStr to \"\"\n\
             set d to item i of allDue\n\
             if d is not missing value then\n\
             set dueStr to \" [due: \" & (d as string) & \"]\"\n\
             end if\n\
             set completedStr to \"\"\n\
             if item i of allCompleted then set completedStr to \" ✓\"\n\
             set output to output & rName & dueStr & completedStr & \"\n\"\n\
             set shown to shown + 1\n\
             end if\n\
             end repeat\n\
             if output is \"\" then\n\
             set output to \"No reminders found.\"\n\
             else if totalCount > maxCount then\n\
             set output to output & \"(Showing \" & shown & \" of \" & totalCount & \" reminders)\"\n\
             end if\n\
             return output\n\
             end tell"
        );
        osascript(&script).await
    }

    async fn create_reminder(&self, params: &serde_json::Value) -> String {
        let title = params["title"].as_str().unwrap_or("Untitled Reminder");
        let list = params["list"].as_str().unwrap_or("");
        let due = params["due_date"].as_str();
        let notes = params["notes"].as_str().unwrap_or("");

        let list_ref = if list.is_empty() {
            "first list".to_string()
        } else {
            format!("first list whose name is \"{list}\"")
        };

        let mut props = format!("name:\"{title}\"");
        if !notes.is_empty() {
            props.push_str(&format!(", body:\"{notes}\""));
        }

        let mut script = format!(
            "tell application \"Reminders\"\n\
             set targetList to {list_ref}\n"
        );

        if let Some(due_iso) = due {
            let due_script = as_date_script("dueDate", due_iso);
            script.push_str(&format!(
                "{due_script}\
                 make new reminder at end of reminders of targetList \
                 with properties {{{}, due date:dueDate}}\n",
                props
            ));
        } else {
            script.push_str(&format!(
                "make new reminder at end of reminders of targetList \
                 with properties {{{}}}\n",
                props
            ));
        }

        script.push_str(&format!(
            "return \"Reminder created: {title}\"\n\
             end tell"
        ));
        osascript(&script).await
    }

    async fn complete_reminder(&self, params: &serde_json::Value) -> String {
        let title = params["title"].as_str().unwrap_or("");
        if title.is_empty() {
            return "Missing 'title' for complete_reminder.".to_string();
        }
        let list = params["list"]
            .as_str()
            .filter(|l| !l.is_empty())
            .map(|l| format!("first list whose name is \"{l}\""))
            .unwrap_or_else(|| "first list".to_string());
        let script = format!(
            "tell application \"Reminders\"\n\
             set targetList to {list}\n\
             set targetReminder to first reminder of targetList \
             whose name is \"{title}\"\n\
             set completed of targetReminder to true\n\
             return \"Reminder completed: {title}\"\n\
             end tell"
        );
        osascript(&script).await
    }

    async fn delete_reminder(&self, params: &serde_json::Value) -> String {
        let title = params["title"].as_str().unwrap_or("");
        if title.is_empty() {
            return "Missing 'title' for delete_reminder.".to_string();
        }
        let list = params["list"]
            .as_str()
            .filter(|l| !l.is_empty())
            .map(|l| format!("first list whose name is \"{l}\""))
            .unwrap_or_else(|| "first list".to_string());
        let script = format!(
            "tell application \"Reminders\"\n\
             set targetList to {list}\n\
             set targetReminder to first reminder of targetList \
             whose name is \"{title}\"\n\
             delete targetReminder\n\
             return \"Reminder deleted: {title}\"\n\
             end tell"
        );
        osascript(&script).await
    }
}

#[async_trait]
impl Tool for AppleEventsTool {
    fn name(&self) -> &str {
        "apple_events"
    }

    fn description(&self) -> &str {
        "Accesses Apple Calendar and Reminders on macOS via AppleScript. \
         Use for scheduling, events, appointments, reminders, and tasks."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "list_calendars",
                        "list_events",
                        "create_event",
                        "delete_event",
                        "list_reminder_lists",
                        "list_reminders",
                        "create_reminder",
                        "complete_reminder",
                        "delete_reminder"
                    ],
                    "description": "The operation to perform"
                },
                "calendar": {
                    "type": "string",
                    "description": "Calendar name (e.g. 'Work', 'Home', 'Calendar')"
                },
                "title": {
                    "type": "string",
                    "description": "Event or reminder title"
                },
                "start": {
                    "type": "string",
                    "description": "Start date/time in ISO 8601, e.g. '2024-01-01T10:00:00'"
                },
                "end": {
                    "type": "string",
                    "description": "End date/time in ISO 8601"
                },
                "location": {
                    "type": "string",
                    "description": "Event location (optional)"
                },
                "notes": {
                    "type": "string",
                    "description": "Notes or description (optional)"
                },
                "list": {
                    "type": "string",
                    "description": "Reminders list name (optional)"
                },
                "from": {
                    "type": "string",
                    "description": "Start of date range for listing events (ISO 8601)"
                },
                "to": {
                    "type": "string",
                    "description": "End of date range for listing events (ISO 8601)"
                },
                "due_date": {
                    "type": "string",
                    "description": "Due date in ISO 8601 (for reminders)"
                },
                "show_completed": {
                    "type": "boolean",
                    "description": "If true, list_reminders includes completed reminders (default: false, only incomplete)"
                }
            },
            "required": ["operation"]
        })
    }

    async fn run(&self, args: &str) -> String {
        let params: serde_json::Value = match serde_json::from_str(args) {
            Ok(v) => v,
            Err(_) => {
                return format!("Invalid JSON arguments. Required: `operation` field.\n\n{USAGE}");
            }
        };

        let operation = match params["operation"].as_str() {
            Some(op) => op,
            None => {
                return format!("Missing 'operation' field.\n\n{USAGE}");
            }
        };

        match operation {
            "list_calendars" => self.list_calendars().await,
            "list_events" => self.list_events(&params).await,
            "create_event" => self.create_event(&params).await,
            "delete_event" => self.delete_event(&params).await,
            "list_reminder_lists" => self.list_reminder_lists().await,
            "list_reminders" => self.list_reminders(&params).await,
            "create_reminder" => self.create_reminder(&params).await,
            "complete_reminder" => self.complete_reminder(&params).await,
            "delete_reminder" => self.delete_reminder(&params).await,
            _ => format!("Unknown operation: {operation}.\n\n{USAGE}"),
        }
    }
}
