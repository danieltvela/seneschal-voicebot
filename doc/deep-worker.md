# Deep Worker (Native Secondary LLM)

> Issue #236. The deep worker is a heavy subagent that runs **inside** the
> Seneschal process with its own LLM endpoint, its own tool-calling loop and
> its own tool pool. The voice model stays lean and delegates everything
> heavy through a single tool: `run_deep_task`.

```
Voz → LLM de habla (sin thinking, pocas tools: voz + run_deep_task)
        └─ run_deep_task → DEEP WORKER (async, endpoint propio, thinking,
                            loop de tool-calling propio; pool: MCP, agentes,
                            ficheros, shell, pantalla, búsqueda)
                              └─ run_agent → Hermes vía ACP (#235: flota Herdr)
```

**Principio rector**: el modelo de habla NO se satura de tools ni
instrucciones; todo lo pesado va al worker. El worker usa modelos con
thinking que el voicebot no puede usar en el loop de voz (demasiado lentos).

## Arquitectura

| Pieza | Dónde | Notas |
|---|---|---|
| Motor (loop de tool-calling, cancelación, eventos) | `crates/seneschal-worker/src/engine.rs` | `DeepWorkerEngine` |
| Tool `run_deep_task` (voz → worker) | `crates/seneschal-worker/src/run_deep_task.rs` | `is_background`: ack inmediato + resultado diferido |
| Tool anidada `run_agent` (worker → Hermes/ACP) | `crates/seneschal-worker/src/agent_delegate.rs` | Trait `AgentDelegate`; impl en `seneschal-extras` |
| Factoría del cliente secundario | `seneschal-core/src/llm/provider.rs::create_deep_worker_provider` | Mismo estilo que `create_provider` |
| Wiring | `src/main.rs` (bloque `Deep Worker`) | Registros MCP/agentes/plugins → pool del worker |
| Delegación awaitable | `crates/seneschal-extras/src/run_agent.rs::run_await` | Núcleos `acp_task_core`, `opencode_prompt_core`, `hermes_prompt_core` compartidos con la ruta de voz |

### Flujo de una tarea

1. El modelo de voz decide (sin router intermedio) y llama
   `run_deep_task(task)`. La rama background de `llm_task` emite el ack
   ("estoy en ello"), arranca el sonido de proceso y persiste el intercambio
   **abreviado** (anti-#221).
2. El worker ejecuta en background: plan (thinking, sin tools) → loop de
   tools → respuesta final sintetizada para voz.
3. El resultado llega como `ProactiveEvent::AgentResult` (sin
   `tool_call_id`) → `AnnouncementWindow` → **una sola locución** (anti-#218).
   El historial del habla nunca ve el tool-calling del worker.
4. La tarea es **detached**: ni el barge-in ni ninguna otra interacción de
   voz la cancelan — igual que los subagentes externos, el worker sigue
   trabajando mientras el usuario habla de otra cosa. Unidades concurrentes
   coexisten. Las únicas cancelaciones son: el comando inline `cancel`
   (`run_deep_task {"task":"cancel"}`), el timeout de la tarea, y el apagado
   del proceso.

### Thinking modes

- **`external`** (default): dos inferencias — paso 1 sin tools con thinking
  (plan), paso 2 loop de tools sin thinking siguiendo el plan. Rodea la
  restricción de `client.rs` (los kwargs de thinking NO se envían junto a
  tools por el conflicto Jinja de las cuantizaciones mlx). Cero riesgo.
- **`inline`**: una sola pasada enviando `chat_template_kwargs` junto a
  tools (flag `RequestOptions::thinking_with_tools`). **Opt-in**: solo
  backends verificados (vLLM con `--enable-reasoning --reasoning-parser`).
  Verificar en tesla (`:8006`) antes de habilitar.

- Servidores con reasoning parser (SGLang `:8005`, vLLM con
  `--reasoning-parser`): el thinking llega por `delta.reasoning_content` con
  `content` vacío. `client.rs` lo expone como `StreamToken::Reasoning`; el
  plan step lo usa como plan cuando `content` viene vacío. La voz y el loop
  de tools IGNORAN estos tokens (nunca se locutan).
- **Estilo de flag** (`DEEP_WORKER_THINKING_STYLE`, mismo valor aplica a la
  voz con `LLM_THINKING_STYLE`): `kwargs` (default histórico,
  `chat_template_kwargs.enable_thinking`; vLLM/SGLang/llama.cpp), `effort`
  (`reasoning_effort: "high"`; o-series/gpt-5 — nunca envía flag de disable),
  `ollama` (`think: true|false`), `none` (el servidor manda; thinking inline
  en `content`, lo strippea el ThinkFilter). `auto` (default) resuelve por
  nombre de modelo: `o1/o3/o4*` o `gpt-5*` → `effort`; `modelo:tag` →
  `ollama`; `deepseek-r1*` → `none`; resto → `kwargs`. Un estilo explícito
  siempre gana a la heurística.

**Política recomendada** (voz rápida, worker profundo, mismo servidor):
voz con `LLM_THINKING=false` + `LLM_THINKING_WITH_TOOLS=true` — el flag de
disable se envía SIEMPRE (también junto a tools), anulando un servidor cuyo
default es thinking ON. Worker con `DEEP_WORKER_THINKING=true` +
`DEEP_WORKER_THINKING_MODE=inline` — thinking ON en todas las pasadas; en
servidores con reasoning parser el thinking llega por canal aparte y el
loop lo ignora, así que no contamina la salida.

### Pool de tools del worker

Con `DEEP_WORKER_ENABLED=1` estas tools cargan en el pool del worker (el
modelo de voz NO las ve):

- **MCP**: todos los servidores `[[mcp_servers]]`/`MCPS`.
- **Agentes externos**: `run_{agent}` se convierte en la tool anidada
  `run_agent(agent, task)` (mismos handles/sesiones ACP).
- **Ficheros**: `read_file`, `write_file`, `list_dir`.
- **Shell**: `run_shell` (solo con `SHELL_ENABLED=1`).
- **Pantalla**: `take_screenshot` (visión contra el endpoint del worker).
- **Búsqueda**: `quick_search` y `deep_research`.
- **Plugins**: sus secciones de prompt van al system prompt del worker; sus
  servidores MCP y agentes también cargan en el pool.

### Observabilidad

- Progreso (plan, tool X, done) como filas de agente `deep_worker` en TUI y
  Control (canal `SessionEvent` reutilizado, sin variantes nuevas).
- Comandos inline desde voz: `run_deep_task {"task": "status"}` /
  `{"task": "cancel"}`.

## Configuración

Claves planas `deep_worker_*` (estilo `classifier_fallback_*`) en
`seneschal.pro.toml` / `seneschal.dev.toml` + overrides `DEEP_WORKER_*`.
Tabla completa de variables: [`env-vars.md`](env-vars.md).

```toml
deep_worker_enabled = true
deep_worker_url = "http://127.0.0.1:8006"   # vLLM con reasoning-parser
deep_worker_model = "qwen3-dense"
deep_worker_thinking = true
deep_worker_thinking_mode = "external"
deep_worker_max_tokens = 2048
deep_worker_temperature = 0.6
deep_worker_max_iterations = 10
deep_worker_timeout_ms = 300000
deep_worker_vision_enabled = true           # requiere modelo multimodal
```

Validación: `DEEP_WORKER_URL` y `DEEP_WORKER_MODEL` son obligatorios cuando
`DEEP_WORKER_ENABLED=1`; `DEEP_WORKER_THINKING_MODE` debe ser
`external`|`inline` (siempre).

## Contratos de regresión cero

- `DEEP_WORKER_ENABLED=0` (default): wiring idéntico al previo — MCP,
  agentes y plugins en el registro de voz, visión contra el LLM de voz,
  sin `run_deep_task`. Criterio 1 de la issue.
- Modos `acp`/`cli`/`remote`/`visible` intactos: los núcleos de delegación
  se compartieron, no se duplicaron (`run_agent.rs`).
- Los resultados del worker nunca persisten como tool exchanges del habla
  (rutan por `tool_call_id: None` → announcement).

## Tests

- Unit: `crates/seneschal-worker` (motor con `MockProvider`: external
  2-pasos, inline kwargs, última iteración sin tools, timeout, cancel,
  barge-in, delegate), `config.rs` (defaults/env/validación), `client.rs`
  (payload thinking+tools), `tools-core` (write_file/list_dir),
  `deep_worker_bridge` (delegate).
- E2E wiremock: `e2e_deep_worker_delegation_flow`,
  `e2e_deep_worker_disabled_is_transparent` (`cargo test e2e -- --ignored`).
