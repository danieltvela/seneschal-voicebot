# Seneschal — MCP as Universal Integration Layer

**Estado:** Borrador para revisión  
**Author:** Análisis arquitectural  
**Fecha:** Julio 2026  
**Ref:** `seneschal-architecture.md` (rediseño FSM anterior), `doc/ARCHITECTURE.md`

---

## 1. Resumen ejecutivo

Seneschal evoluciona de **voicebot con TUI embebido** a **orquestador de voz puro** que delega
toda UI compleja a aplicaciones externas vía **MCP (Model Context Protocol)**. El núcleo de
Seneschal (pipeline STT→LLM→TTS, FSM, barge-in, memoria, agentes) se mantiene sin cambios
estructurales. Las nuevas funcionalidades que motivan este rediseño — editor de prompts
visible/editable, visualización de ACP sessions, terminal embebido, file picker, BrowserOS
por voz, edición colaborativa de documentos — se resuelven integrando apps externas con
servers MCP en vez de construir un UI monolítico.

**Resultado:**
- Seneschal core: ~3-4 semanas de trabajo quirúrgico (3 gaps en MCP client + 1 nuevo tool +
  1 refactor + endpoints extra en Control API).
- Apps externas con MCP server: work paralelo no acoplado a Seneschal.
- Multi-plataforma (macOS+Linux) sin esfuerzo extra — cada app maneja su propia GUI.
- Reutiliza ecosistema MCP creciente (VS Code MCP, Playwright MCP, chrome-devtools MCP, etc.).

**No se construye un UI nuevo dentro de Seneschal.** El TUI actual queda como modo
"status-only" opcional (sin paneles).

---

## 2. Visión arquitectónica

```
┌─────────────────────────────────────────────────────────────┐
│                  SENESCHAL (orquestador de voz)              │
│                                                              │
│  Pipeline STT → LLM → TTS (sin cambios)                     │
│  FSM, barge-in, SQLite memory (sin cambios)                 │
│  Audio I/O local (server-side, confirmed)                   │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐ │
│  │  MCP CLIENT (extendido)                                │ │
│  │   - Stdio transport (actual)                            │ │
│  │   + HTTP/SSE transport (nuevo, Gap 2)                  │ │
│  │   + Server→Client notifications (nuevo, Gap 1)          │ │
│  │   + resources/subscribe (nuevo, Gap 3)                  │ │
│  │   + per-server notification handler (nuevo)             │ │
│  └────────────────┬────────────────┬───────────────────────┘ │
│                   │                │                         │
│  ┌────────────────▼──────┐  ┌──────▼────────────┐           │
│  │ RequestPathTool (Gap 4)│  │ Control API (Gap 6)│           │
│  │  - Llama MCP picker   │  │  + /control/agents │           │
│  │  - Fallback nativo    │  │  + /control/acp    │           │
│  │  (osascript/zenity)   │  │  + /proactive_events│          │
│  └────────────────────────┘  └────────────────────┘           │
└────────────────┬────────────────┬─────────────────┬───────────┘
                 │                │                 │
        MCP stdio          MCP HTTP/SSE       Control HTTP/SSE
        (subprocess)       (local or remote)   (any monitoring tool)
                 │                │                 │
   ┌─────────────▼──────┐ ┌───────▼────────┐  ┌─────▼────────────┐
   │ Editor MCP          │ │ Browser MCP    │  │ Dashboard app    │
   │  (eg VS Code ext)   │ │  (Playwright/  │  │  (any stack,     │
   │   - prompts: open/  │ │   chrome-dev-  │  │  subscribes SSE) │
   │     update/close    │ │   tools MCP)   │  │                  │
   │   - resources: doc  │ │   - tools: nav │  │  Visualizes:      │
   │     state           │ │     click, ... │  │   - Pipeline state│
   │   - notifications:  │ │                │  │   - ACP sessions  │
   │     document_changed│ │                │  │   - ProactiveEvents│
   └─────────────────────┘ └────────────────┘  └──────────────────┘
```

---

## 3. Gap analysis (estado actual vs necesario)

Revisé `src/mcp/mod.rs` línea a línea. Estado actual del MCP client:

| ID | Gap | Estado actual | Referencia código |
|----|-----|---------------|-------------------|
| 1 | Server→client notifications routing | `// Notifications (initialized, etc.) are silently ignored.` | `src/mcp/mod.rs:268` |
| 2 | HTTP/SSE transport | Solo subprocess stdio | `McpClient::spawn_and_init` (no `new_http`) |
| 3 | `resources/subscribe` | No implementado | Solo `tools/list`, `tools/call` |
| 4 | `RequestPathTool` con fallback nativo | No existe | `src/tools/` (no `request_path.rs`) |
| 5 | Refactor `set_prompt_build` a MCP editor backend | Estado local `Arc<Mutex<PromptBuildState>>` sin ruta MCP | `src/tools/prompt_build.rs`, `src/pipeline/llm_task.rs:460` |
| 6 | Endpoints introspectivos en Control API | Solo `/control/sessions`, `/control/state`, `/control/history` | `src/control/api.rs:27-35` — faltan ACP sessions y proactive events stream |
| 7 (opcional) | Seneschal como MCP server | No existe | Futuro: exponer estado interno a dashboards MCP-aware |

---

## 4. Contratos MCP propuestos

Los contratos son **agnósticos al editor/browser**: Seneschal habla con cualquier MCP server
que cumpla el método/params esperado. Esto permite múltiples implementaciones (VS Code
extension, custom Tauri app, web app, etc.).

### 4.1 Editor MCP Server (Feature A1 + D)

**Herramientas expuestas por el editor (`tools/list`):**

```jsonc
// editor.open_document
{
  "name": "open_document",
  "description": "Open a new or existing document visible to the user.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "content": {"type": "string", "description": "Initial content. Can be empty for new doc."},
      "path": {"type": "string", "description": "File path if opening existing; omit for new."},
      "language": {"type": "string", "description": "Language hint for syntax highlighting (md, txt, toml...)"}
    }
  }
}
// Response: { "doc_id": "string", "content": "string" }

// editor.update_content
{
  "name": "update_content",
  "description": "Replace the full content of an open document.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "doc_id": {"type": "string"},
      "content": {"type": "string"}
    },
    "required": ["doc_id", "content"]
  }
}
// Response: { "applied": true, "content": "string" }

// editor.insert_text (granular para collab editing)
{
  "name": "insert_text",
  "inputSchema": {
    "type": "object",
    "properties": {
      "doc_id": {"type": "string"},
      "location": {"type": "object", "properties": {"line": {"type": "integer"}, "column": {"type": "integer"}}, "description": "Insertion point. If omitted, append to end."},
      "text": {"type": "string"}
    },
    "required": ["doc_id", "text"]
  }
}

// editor.delete_range (granular)
{ "name": "delete_range", ... }

// editor.get_content
{
  "name": "get_content",
  "inputSchema": {"type": "object", "properties": {"doc_id": {"type": "string"}}, "required": ["doc_id"]}
}
// Response: { "content": "string", "version": 42 }

// editor.close_document
{
  "name": "close_document",
  "inputSchema": {"type": "object", "properties": {"doc_id": {"type": "string"}}, "required": ["doc_id"]}
}
// Response: { "closed": true }
```

**Recursos (`resources/list`):**

```jsonc
{ "uri": "editor://docs/{doc_id}", "name": "Document {doc_id}", "mimeType": "text/plain" }
```

**Notificaciones server→client (Gap 1):**

```jsonc
// notifications/document_changed — fires when user edits in the editor
{
  "jsonrpc": "2.0",
  "method": "notifications/document_changed",
  "params": { "doc_id": "...", "content": "...", "version": 43, "source": "user" }
}

// notifications/document_saved
{
  "method": "notifications/document_saved",
  "params": { "doc_id": "...", "path": "/path/to/file.md" }
}

// notifications/document_closed
{
  "method": "notifications/document_closed",
  "params": { "doc_id": "..." }
}
```

Seneschal routea estas notifications a `ProactiveEvent::McpNotification { server, method, params }`
que el LLM usa en su siguiente turno.

### 4.2 File Picker MCP Server (Feature B)

**Herramientas:**

```jsonc
// picker.pick_file
{
  "name": "pick_file",
  "description": "Open native file picker dialog, return selected path.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "starting_dir": {"type": "string", "description": "Optional initial directory"},
      "filter": {"type": "string", "description": "Optional glob filter, eg '*.md'"}
    }
  }
}
// Response: { "path": "/users/daniel/file.md" } or { "cancelled": true }

// picker.pick_directory
{
  "name": "pick_directory",
  "inputSchema": { "type": "object", "properties": {} }
}
// Response: { "path": "/users/daniel/projects" }
```

**Implementation reference:** small server stdio o HTTP. macOS: `osascript` o `NSOpenPanel`
via Swift helper. Linux: `zenity --file-selection --directory`. Multi-plataforma.

**Fallback sin MCP picker configurado**: Seneschal lanza directamente el picker nativo desde
`RequestPathTool` sin pasar por MCP.

### 4.3 Browser MCP Server (Feature C)

No hay contrato nuevo. Reutiliza **Playwright MCP** o **chrome-devtools MCP** existentes:
- `browser.navigate({ url })`
- `browser.click({ selector })`
- `browser.get_screenshot()`
- ...

Seneschal no cambia. **Para visualizar el estado del browser en un dashboard externo**, dicho
dashboard puede subscribirse a `/control/events` y ver cuando Seneschal llama `browser_mcp__*`.

### 4.4 Terminal MCP Server (Feature A3, futuro)

```jsonc
// terminal.start_session
{ "name": "start_session", "inputSchema": {"type": "object", "properties": {"cwd": {"type": "string"}}}}
// Response: { "session_id": "..." }

// terminal.run
{ "name": "run", "inputSchema": {"type": "object", "properties": {"session_id": {"type":"string"}, "command": {"type": "string"}}, "required": ["command"]}}
// Response: { "exit_code": 0, "stdout": "...", "stderr": "..." }

// terminal.send_input (interactive shells)
{ "name": "send_input", "inputSchema": {"type":"object","properties":{"session_id":{"type":"string"},"text":{"type":"string"}}}}

// Notifications:
//   "notifications/terminal_output" { session_id, chunk }
//   "notifications/terminal_exited"  { session_id, exit_code }
```

Útil para que Seneschal gestione terminales para otros agentes externos sin embedir un
terminal dentro de su binary.

### 4.5 Seneschal como MCP Server (Gap 7, opcional/futuro)

Exponer el estado interno de Seneschal a dashboards MCP-aware vía resources:

```jsonc
{ "uri": "seneschal://pipeline_state", "name": "Pipeline state" }
{ "uri": "seneschal://acp_sessions",   "name": "ACP sessions" }
{ "uri": "seneschal://proactive_events_queue", ... }
```

Y notifications:

```jsonc
// "notifications/pipeline_state_changed" { new_state, utterance_id }
// "notifications/acp_message" { task_id, agent_name, message }
```

Esto da a un dashboard MCP-native acceso real-time sin pasar por HTTP/SSE. **Posponer hasta
que exista un dashboard MCP-aware real.**

---

## 5. Cambios en Seneschal (detallados)

### Gap 1 — Server→Client notification routing

**Archivos afectados:**
- `src/mcp/mod.rs` — extender reader task
- `src/agents/mod.rs` — nuevo `ProactiveEvent::McpNotification`
- `src/main.rs` — wire notification handler cuando se spawn McpClient

**Diseño:**

```rust
// src/agents/mod.rs
pub enum ProactiveEvent {
    // ... existentes ...
    McpNotification {
        server_name: String,
        method: String,
        params: serde_json::Value,
    },
}

// src/mcp/mod.rs — McpClient ahora acepta callback
pub type NotificationHandler =
    Arc<dyn Fn(&str, serde_json::Value) -> Option<ProactiveEvent> + Send + Sync>;

pub struct McpClient {
    writer: Mutex<McpWriter>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<RpcResponse>>>>,
    tool_timeout_secs: u64,
    notification_handler: Option<NotificationHandler>,
    proactive_tx: Option<mpsc::Sender<ProactiveEvent>>,
}

// Reader task:
match &parsed {
    Value::Object(obj) if obj.contains_key("method") && !obj.contains_key("id") => {
        let method = obj["method"].as_str().unwrap_or_default();
        let params = obj.get("params").cloned().unwrap_or_default();
        if let (Some(handler), Some(tx)) = (&self.notification_handler, &self.proactive_tx) {
            if let Some(event) = handler(method, params) {
                let _ = tx.try_send(event);
            }
        }
    }
    _ => { /* response parsing actual */ }
}
```

**Compatibilidad:** notification_handler `Option<>` — sin handler, behavior es el actual
(ignore). No breaking.

### Gap 2 — HTTP/SSE transport

**Archivos afectados:**
- `src/mcp/mod.rs` — extraction de transport
- `src/mcp/transport.rs` (nuevo)

**Diseño:**

```rust
// src/mcp/transport.rs
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn send(&self, json: serde_json::Value) -> Result<()>;
    /// Subscribe to incoming messages. Caller spawns a reader task.
    async fn subscribe(&self) -> Result<mpsc::Receiver<serde_json::Value>>;
    async fn close(&self) -> Result<()>;
}

pub struct StdioMcpTransport { stdin, child, stdout }
pub struct HttpMcpTransport { base_url, client: reqwest::Client, sse_rx: Arc<Mutex<Option<...>>> }
```

`HttpMcpTransport` usa POST para request y GET con `Accept: text/event-stream` para SSE de
responses y notifications (transporte MCP HTTP+SSE estándar).

`McpClient::new_stdio(command)` y `McpClient::new_http(url)` son factories que devuelven
`McpClient<T: McpTransport>`. Refactor no rompe callers existentes si exponemos `McpClient`
como trait object o struct genérico con default type parameter.

`McpConfig` gana nuevo campo `transport: enum { Stdio, Http(url) }`. Env override
`MCP_<NAME>_URL` >> HTTP mode.

### Gap 3 — `resources/subscribe`

**Archivos afectados:** `src/mcp/mod.rs`

```rust
impl McpClient {
    /// Subscribe to a resource. Returns a channel that receives updates.
    pub async fn subscribe_resource(&self, uri: &str) -> Result<mpsc::Receiver<ResourceUpdate>> {
        let (tx, rx) = mpsc::channel(16);
        let id = self.send_request("resources/subscribe", json!({ "uri": uri })).await?;
        // Reader task splits notifications by params.subscriptionId → channel
        self.subscription_handlers.lock().await.insert(uri.to_string(), tx);
        Ok(rx)
    }

    pub async fn list_resources(&self) -> Result<Vec<McpResource>> { ... }
    pub async fn read_resource(&self, uri: &str) -> Result<Value> { ... }
}

// Notifications "notifications/resources/updated" route to subscription_handlers map keyed by uri.
```

Es opcional inicialmente. El editor MCP puede empezar con polling (`editor.get_content`
cada cierto tiempo) y migrar a subscriptions cuando sea necesario.

### Gap 4 — RequestPathTool

**Archivos afectados:**
- `src/tools/request_path.rs` (nuevo)
- `src/main.rs` — registrar tool
- `src/agents/mod.rs` — nuevo `ProactiveEvent::RequestUserInput`

**Diseño:**

```rust
// src/agents/mod.rs
pub enum UserInputKind {
    FilePicker { filter: Option<String>, starting_dir: Option<String> },
    DirectoryPicker { starting_dir: Option<String> },
    TextInput { placeholder: Option<String>, multiline: bool },
    Confirm { question: String, options: Vec<String> },
}

pub struct UserInputRequest {
    pub request_id: u64,
    pub kind: UserInputKind,
    pub prompt: String,
    pub response_tx: oneshot::Sender<String>,
}

pub enum ProactiveEvent {
    // ...
    RequestUserInput(UserInputRequest),
}

// src/tools/request_path.rs
pub struct RequestPathTool {
    proactive_tx: mpsc::Sender<ProactiveEvent>,
}

#[async_trait]
impl Tool for RequestPathTool {
    fn name(&self) -> &str { "request_path" }
    fn description(&self) -> &str {
        "Ask the user to select a file or directory via UI picker. Use when the LLM needs \
         a specific path that the user cannot dictate easily via voice. \
         Actions: 'file' or 'directory'."
    }
    fn should_force_for(&self, msg: &str) -> bool {
        msg.to_lowercase().contains("el fichero")
            || msg.contains("el archivo")
            || msg.contains("la carpeta")
        // heuristic
    }
    async fn run(&self, args: &str) -> String {
        let kind = parse_kind(args);
        let (tx, rx) = oneshot::channel();
        let _ = self.proactive_tx.send(ProactiveEvent::RequestUserInput {
            request_id: next_id(), kind, prompt: ..., response_tx: tx,
        }).await;
        match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(path)) => format!("User selected: {path}"),
            Ok(Err(_)) => "User cancelled selection".to_string(),
            Err(_) => "Selection timed out".to_string(),
        }
    }
}
```

**Routing del `ProactiveEvent::RequestUserInput`** en `main.rs`:
1. Si hay una UI MCP client suscrita (eg editor con `ui.request_user_input` tool), Seneschal
   la llama vía MCP. La app abre el picker nativo, user selecciona, response vuelve por MCP.
2. Si no: fallback local — spawn de proceso picker nativo
   (`osascript -e ...` / `zenity --file-selection`).

### Gap 5 — Refactor `set_prompt_build` → editor MCP backend

**Archivos afectados:**
- `src/config.rs` — nuevo campo `editor_mcp_server: Option<String>` (MCP server name)
- `src/pipeline/llm_task.rs:460` — cuando LLM llama `set_prompt_build(action="start")`, si
  `editor_mcp_server` está configurado, el handler traduce a:
  - LLM call: `editor_mcp__open_document(content="")` en lugar de actualizar `PromptBuildState`
  - Captura el `doc_id` retornado y lo guarda en
    `PromptBuildState::Active { prompt: None, doc_id: Some(id) }` (state renombrado de
    "texto inline" a "referencia a doc externo")
- LLM calls `set_prompt_build(action="update", prompt="...")` → handler traduce a
  `editor_mcp__update_content(doc_id, content=prompt)`
- LLM calls `set_prompt_build(action="cancel")` → handler llama
  `editor_mcp__close_document(doc_id)`
- TUI ya no necesita mostrar el prompt como inline text; sólo indicador de "editable en
  editor externo". Los TUI events `PromptBuildUpdate{...}` quedan como no-op en modo MCP.

**User edita en el editor externo, Seneschal recibe
`McpNotification { method: "notifications/document_changed", params: { doc_id, content, source: "user" } }`**.
El pipeline lo routea a `ProactiveEvent::McpNotification` (Gap 1). En la próxima llamada de
`set_prompt_build(action="update")`, el LLM verá el content actualizado en la respuesta de
`editor_mcp__get_content(doc_id)`.

Para Feature D (collab blog post): patrón idéntico, con
`editor.open_document(path="blog.md", language="md")` en lugar de pasar `content`. La
diferencia con Feature A1 (prompt) es sólo el filename y el context en que se invoca.

### Gap 6 — Endpoints introspectivos en Control API

**Archivos afectados:**
- `src/control/api.rs` — nuevos routes
- `src/control/state.rs` — acceso a `AcpSessionManager` y proactive_tx
- `src/agents/session_manager.rs` — método introspectivo `list_active_sessions()` para los
  endpoints

**Nuevos endpoints:**

```rust
.route("/control/agents",           get(get_agents))           // AgentRegistry config
.route("/control/agent_sessions",   get(get_acp_sessions))     // AcpSessionManager state
.route("/control/pending_interactions", get(get_pending_interactions)) // Queue de AgentQuestion + RequestUserInput
.route("/control/mcp_servers",      get(get_mcp_servers))       // McpRegistry + tool count per server
.route("/control/proactive_stream", get(sse_proactive_events)) // SSE stream de ProactiveEvent
```

Cualquier monitoring tool/dashboard externo (Future A2 — visualización ACP sessions) puede
subscribirse vía SSE y renderizar el estado. **No se construye dentro de Seneschal.**

### Gap 7 — Seneschal como MCP server (opcional, futuro)

Exponer state interno como recursos MCP para que dashboards MCP-native puedan subscribirse
sin HTTP. **Posponer** hasta que exista demanda concreta. Esta tarea requiere extraer un
`trait McpServerHandler` y servir JSON-RPC 2.0 encima de stdio o HTTP.

---

## 6. Plan de migración por fases

### Fase 0 — Spike de validación MCP externo (~1 semana, paralela, no bloqueante)

Antes de tocar código Rust:
- Verificar madurez del soporte MCP en editors existentes (VS Code extensions marketplace,
  Continue.dev, Cursor editor, Orbit, etc.) — ¿alguno expone `resources/subscribe` y
  notificaciones custom?
- Probar BrowserOS MCP (Playwright MCP) con Seneschal actual (sin cambios) end-to-end para
  validar Feature C funciona ya.
- Identificar 1 editor MCP funcional (o decidir que hay que construir uno).

**Entregable:** decisión concreta sobre editor MCP a usar (Feature A1).

### Fase 1 — Habilitación bidireccional (3-4 semanas)

Order dentro de la fase (cada PR compila + pasa `make qa`):

1. **Gap 1** — server→client notifications en MCP reader task +
   `ProactiveEvent::McpNotification` + handler opcional (sem 1)
2. **Gap 4** — `RequestPathTool` con fallback nativo +
   `ProactiveEvent::RequestUserInput` (sem 1-2)
3. **Gap 2** — `McpTransport` trait + `StdioMcpTransport` refactor +
   `HttpMcpTransport` (sem 2-3)
4. **Gap 6** — endpoints introspectivos en Control API (sem 3-4)

**Entregable Fase 1:** Feature B lista E2E (file/dir picker por tool LLM). Notifications
bidireccionales funcionando. Soporte MCP HTTP/SSE. Dashboard externo puede subscribirse a
state.

### Fase 2 — Refactor prompt_build a editor MCP (1-2 semanas)

5. **Gap 5** — `editor_mcp_server` config flag. Handler en `llm_task.rs:460` rutea a
   `editor_mcp__*` tools cuando flag presente. Fallback a comportamiento actual. (sem 5)

**Entregable Fase 2:** Feature A1 lista E2E cuando el editor MCP server (de Fase 0) esté
conectado.

### Fase 3 — Recursos/subscribe y collab editing (1-2 semanas, condicional)

6. **Gap 3** — `resources/subscribe` en McpClient (sem 6)
7. Implementar feature D (collab blog post editing) usando
   `editor.open_document(path=...)` + `notifications/document_changed` routing → próxima
   iteración LLM. (sem 6-7)

**Entregable Fase 3:** Feature D lista. Workflow voice-driven blog writing funcional con
editor externo.

### Fase 4 — BrowserOS, Terminal MCP y otros (open-ended, según demanda)

No requiere más cambios en Seneschal. Es puro discovery/integración del MCP server externo
correspondiente.

```
Fase 0 (1 semana) ────●── Fase 1 (3-4 semanas) ────●─ Fase 2 (1-2 sem) ─●─ Fase 3 (1-2 sem)
  Spike editor MCP        Gap 1 + 4 + 2 + 6          Gap 5             Gap 3 + Feature D
```

Total: ~6-8 semanas de trabajo en Seneschal core. Apps externas (editor MCP, BrowserOS,
etc.) en paralelo, timeline independiente.

---

## 7. Decisiones pendientes

Pendientes de definir tras la revisión del documento:

1. **¿Qué editor MCP server se usa para Feature A1 y D?**
   - (a) Extender VS Code con un custom MCP extension (TS, comunidad MCP creciendo)
   - (b) Construir un editor simple con Tauri + web frontend (más weeks trabajo pero control
     total UX)
   - (c) Otro editor existente con MCP que yo no conozca todavía

   Recomendación ejecutable tras Fase 0 spike.

2. **¿Mantener `set_prompt_build` actual (con `Arc<Mutex<PromptBuildState>>`)** como
   **fallback permanente** (Feature A1 funciona sin editor MCP), o **deprecarlo** cuando
   editor MCP esté disponible?
   - **Recomendación:** mantenerlo como fallback. No eliminar. La added complexity es baja.

3. **¿TUI evoluciona o se deprecia?**
   - **Recomendación:** mantener como modo "status-only" sin paneles. Sin features nuevas.
     No es priority. Eventualmente puede eliminarse si un dashboard MCP-native lo sustituye.

4. **¿Clients iOS/watchOS companion cambian?**
   - No. Siguen usando WebSocket + audio streaming. No son afectados por esta arquitectura.

5. **Gap 7 (Seneschal como MCP server)¿priorizarlo?**
   - **Recomendación:** postergar. Añade complejidad sin valor inmediato. Revisar en 2-3
     meses si surge caso de uso.

---

## 8. Riesgos y mitigaciones

| Riesgo | Probabilidad | Impacto | Mitigación |
|--------|--------------|---------|-----------|
| Editor MCP no existe maduro en ecosistema | Media | Alto (Feature A1/D bloqueadas) | Fase 0 spike valida antes de empezar Fase 2. Plan B: build un simple Tauri editor con MCP server (~2-3 sem extra). |
| Notifications server→client inestables con servers reales | Media | Medio | Gap 1 diseñado como opcional (`Option<NotificationHandler>`). Si falla, comportamiento actual (ignore) se mantiene. |
| `resources/subscribe` no soportado universalmente | Alta | Bajo | Gap 3 implemented como optimización opcional. Fallback a polling `editor.get_content` en intervalos. |
| Refactor `set_prompt_build` rompe tests | Baja | Medio | Mantener flag `editor_mcp_server=None` como default. Comportamiento actual intacto. Tests existentes siguen pasando. |
| MCP HTTP/SSE transport con cabeceras no estándar entre servers | Media | Medio | Diseñar `HttpMcpTransport` configurable con custom headers. Spike con ≥1 server real antes de finalizar Gap 2. |
| Colisión entre tools MCP y tools internas de Seneschal | Baja | Bajo | Prefix `{server}_mcp__{tool}` ya evita colisión. Documentar convención `editor_mcp__`, `browser_mcp__`, `picker_mcp__`. |

---

## 9. Diagrama final sinóptico de impacto en el código

```
ARCHIVO                                     CAMBIO
─────────────────────────────────────────────────────────────────────────
src/mcp/mod.rs                              Gap 1+2+3: notifications, transport trait, resources
src/mcp/transport.rs                (nuevo) Gap 2: McpTransport trait + Stdio + Http impls
src/agents/mod.rs                           Gap 1+4: McpNotification, RequestUserInput + UserInputKind
src/agents/session_manager.rs               Gap 6: list_active_sessions() introspect method
src/tools/request_path.rs           (nuevo) Gap 4: RequestPathTool + native fallback
src/tools/prompt_build.rs                   Gap 5: keep as fallback (no elimina)
src/tools/mod.rs                            Gap 4: registrar RequestPathTool
src/config.rs                               Gap 4+5: editor_mcp_server, picker configs
src/control/api.rs                          Gap 6: 4 nuevos routes
src/control/state.rs                        Gap 6: acceso a AcpSessionManager + proactive events
src/pipeline/llm_task.rs                     Gap 5: handler en :460 ruta a editor MCP si flag
src/tui/                                    (sin cambios + deprecation path)
src/remote/                                 (sin cambios — iOS/watchOS no afectados)
src/audio/, src/stt/, src/llm/, src/tts/    (sin cambios)
src/pipeline/{fsm, frames, state, sen, tts, consolidation} (sin cambios)
tests/                                      Nuevos tests para cada gap
doc/                                        Documento actualizado con cada fase
```

**Sin cambios estructurales en el pipeline de voz** (FSM, barge-in, consolidation, daemons).
Seneschal core queda intacto. Toda la complejidad nueva vive en `src/mcp/` (extendido) y
`src/tools/` (añadido).