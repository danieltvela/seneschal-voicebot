use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;

use super::Tool;

#[derive(Clone, Debug, PartialEq)]
pub enum SubtaskStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Clone)]
pub struct Subtask {
    pub id: String,
    pub tool_name: String,
    pub status: SubtaskStatus,
    pub description: String,
    pub created_at: Instant,
    pub result: Option<String>,
}

struct SubtaskTrackerInner {
    tasks: HashMap<String, Subtask>,
}

/// Maximum number of subtasks to track. Older completed/failed tasks are evicted.
const MAX_SUBTASKS: usize = 50;

/// Tracks background tool executions so the LLM can query their status.
pub struct SubtaskTracker {
    inner: Mutex<SubtaskTrackerInner>,
}

impl Default for SubtaskTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SubtaskTracker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(SubtaskTrackerInner {
                tasks: HashMap::new(),
            }),
        }
    }

    pub fn add(&self, id: String, tool_name: String, description: String) {
        let mut inner = self.inner.lock().unwrap();
        // Evict oldest completed/failed tasks if we're over capacity
        if inner.tasks.len() >= MAX_SUBTASKS {
            let to_remove: Vec<String> = inner
                .tasks
                .iter()
                .filter(|(_, t)| t.status != SubtaskStatus::Running)
                .map(|(k, _)| k.clone())
                .collect();
            for key in to_remove {
                inner.tasks.remove(&key);
            }
        }
        inner.tasks.insert(
            id.clone(),
            Subtask {
                id,
                tool_name,
                status: SubtaskStatus::Running,
                description,
                created_at: Instant::now(),
                result: None,
            },
        );
    }

    pub fn complete(&self, id: &str, result: String) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(task) = inner.tasks.get_mut(id) {
            task.status = SubtaskStatus::Completed;
            task.result = Some(truncate(&result, 500));
        }
    }

    pub fn fail(&self, id: &str, error: String) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(task) = inner.tasks.get_mut(id) {
            task.status = SubtaskStatus::Failed;
            task.result = Some(format!("Error: {}", error));
        }
    }

    pub fn list(&self) -> Vec<Subtask> {
        let inner = self.inner.lock().unwrap();
        inner.tasks.values().cloned().collect()
    }

    pub fn active_count(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner
            .tasks
            .values()
            .filter(|t| t.status == SubtaskStatus::Running)
            .count()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Safe UTF-8 truncation: use get() to find char boundary
        let safe_prefix = s.get(..max).unwrap_or(s);
        format!("{} (...truncated, {} total chars)", safe_prefix, s.len())
    }
}

fn safe_preview(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        s.get(..max).unwrap_or(s)
    }
}

/// Tool that lets the LLM query the status of background tasks.
pub struct ListTasksTool {
    tracker: Arc<SubtaskTracker>,
}

impl ListTasksTool {
    pub fn new(tracker: Arc<SubtaskTracker>) -> Self {
        Self { tracker }
    }
}

#[async_trait]
impl Tool for ListTasksTool {
    fn name(&self) -> &str {
        "list_tasks"
    }

    fn description(&self) -> &str {
        "Lista las tareas en segundo plano que se están ejecutando o que han terminado recientemente. \
         Úsala cuando el usuario pregunte qué estás haciendo, el estado de una tarea, o si algo terminó."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn run(&self, _args: &str) -> String {
        let tasks = self.tracker.list();
        if tasks.is_empty() {
            return "No hay tareas activas o recientes.".to_string();
        }

        let mut output = String::new();
        for task in &tasks {
            let status_str = match task.status {
                SubtaskStatus::Running => "en curso",
                SubtaskStatus::Completed => "completada",
                SubtaskStatus::Failed => "fallida",
            };
            let elapsed = task.created_at.elapsed().as_secs();
            output.push_str(&format!(
                "- {} ({} - {}s)\n",
                task.tool_name, status_str, elapsed
            ));
            if let Some(ref result) = task.result {
                let preview = safe_preview(result, 200);
                output.push_str(&format!("  Resultado: {}\n", preview));
            }
        }
        output.trim_end().to_string()
    }
}
