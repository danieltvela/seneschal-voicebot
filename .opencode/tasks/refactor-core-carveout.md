# Seneschal Modular Carve-Out & Salvage Plan

## Context

- Origin: Direct user instruction — el proyecto (~25k LOC, ~110 `.rs`, ~30 docs) ha crecido desde un chatbot de voz mono-usuario hacia casi un "sistema operativo de IA" (MCP, plugins, multi-agente, dream, control API, remote, TUI), y ya no cumple el objetivo inicialBundle. Se pide analizar qué salvar y producir un plan de carve-out.
- Dirección elegida: **Análisis de salvables + plan de carve-out** (no rewrite en verde). Conservar la visión ampliada pero con **fronteras estrictas**: núcleo de voz como crate base, cada capa acretada como crate separado y feature-flagged.
- Secuencia: **Doc primero**. Reconciliar contradicciones y rellenar los 8 gaps de doc críticos antes de tocar código.
- Base: `master` actual (no se crea rama por ahora; el agente de build la creará si/el cuando se ejecute — proposición: `refactor/core-carveout`).

### Inventario de salvables (decisión tomada, fuente: explore)

| Estado | Módulo (`src/`) | Veredicto | Razón |
|--------|------------------|-----------|-------|
| 🟢 SALVAR (núcleo) | `audio/` (audio_capture, audio_transform, buffer, ambient_buffer, output) | Mover a `seneschal-core` | Pipeline de voz esencial, aislado, válido |
| 🟢 SALVAR | `stt/` (provider, whisper, no_speech_gate) | Mover a `seneschal-core` | Whisper+VAD es el backend principal y maduro |
| 🟢 SALVAR | `llm/` (client, session, provider) | Mover a `seneschal-core` | Contrato OpenAI-SSE bien documentado |
| 🟢 SALVAR | `tts/` (mod, sentence, avspeech, kokoro) | Mover a `seneschal-core` | AvSpeech+Kokoro+splitter; núcleo del pipeline |
| 🟢 SALVAR | `pipeline/` (frames, fsm, state, llm_task, sen_task, tts_task, consolidation) | Mover a `seneschal-core` | FSM y actores; corazón del producto |
| 🟢 SALVAR | `config.rs` (núcleo de config) | Dividir: parte común a `core`, resto a cada crate | Single source-of-truth de config |
| 🟢 SALVAR | `db/` | Crate `seneschal-memory` | SQLite + FTS5, dependencia de dream/profile/memory |
| 🟢 SALVAR (como crate optativo) | `tools/` (shell, clipboard, current_time, read_file, take_screenshot, quick_search, open_app) | Crate `seneschal-tools-core` | Herramientas LLM "razonable mínimo" |
| 🟡 SALVAR Pero aislar | `mcp/` (mod, config, transport) | Crate `seneschal-mcp` | Maduro y útil, pero separar del núcleo |
| 🟡 SALVAR Pero aislar | `agents/` (session_manager, hermes_events, opencode_*, config) | Crate `seneschal-agents` | Complejo; aislar para no contaminar el núcleo |
| 🟡 SALVAR Pero aislar | `plugins/` (manager, manifest, mcp_spawner, agent_bridge, prompt_injection, config_overrides) | Crate `seneschal-plugins` | Solo si se conserva la visión ampliada (decidida: sí) |
| 🟡 SALVAR Pero aislar | `control/` (api, state, broadcast, client) | Crate `seneschal-control` | API REST+SSE; útil para integración |
| 🟡 SALVAR Pero aislar | `remote/` (mod, protocol, server, tests) | Crate `seneschal-remote` | WebSocket para cliente Apple Watch |
| 🟡 SALVAR Pero aislar | `search/` (brave, tavily, exa, searxng, mod) | Crate `seneschal-search` | Necesita doc del trait `SearchProvider` (gap) |
| 🟡 SALVAR Pero aislar | `memory/`, `profile/`, `dream/` | Crate `seneschal-memory` (junto a `db/`) | S-Dream L1/L2, perfil, memorias |
| 🟡 SALVAR Pero aislar | `classifier/` (heuristic, keyword, pipeline, fallback) | Crate `seneschal-classifier` SOLO la parte funcional. Ver siguiente fila para descartar |
| 🔴 DESCARTAR | `classifier/embedding.rs`, `classifier/logistic.rs` | Eliminar | Feature `classifier-embedding` **vacío** en Cargo.toml; stubs que siempre devuelven error |
| 🔴 DESCARTAR | `tts/piper.rs` | Eliminar | Existe en disco pero NO declarado en `tts/mod.rs`; dead code |
| 🔴 DESCARTAR | `bin/bench_pipeline.rs.bak` | Eliminar | Backup `.bak` en el árbol de fuente |
| 🟠 REEVALUAR | `tui/` | Decisión de producto: mantener como "status-only" (ver `doc/ARCHITECTURE-MCP-LAYER.md`) o deprecar. Plan asume **status-only** minimal, feature-gated |
| 🟠 REEVALUAR | `daemon.rs` (proactividad periódica), `eyes.rs` (screenshots+visión), `screen_capture.rs`, `agent_session.rs` (PTY visible) | Mover a `seneschal-extras` con feature flag y duda si volver a integrar. Documentar antes de conservar |

### Resumen doc: capacidad de rebuild desde cero

- **Veredicto global: PARCIAL.** La **pipeline central** (STT, TTS, VAD, SentenceSplitter, FSM, barge-in, config/env-vars, Control API, remote, testing) está documentada a nivel spec — reconstruible.
- **Restan 8 gaps críticos** que impiden rebuild completo sin rellenar doc:

| # | Gap | Estado actual | Doc objetivo |
|---|-----|---------------|--------------|
| G1 | `classifier/` (cascada keyword→heuristic→fallback) | Sin doc | `doc/classifier.md` |
| G2 | `plugins/` internos (lifecycle, traits, PluginSwitchEvent, revert de config overrides) | Solo nivel usuario | `doc/plugins-internal.md` |
| G3 | `agents/` internos (AcpWriter, state-machine sesión, dual Hermes/OpenCode) | Parcial (PROTOCOL_INFO) | `doc/agents-internal.md` |
| G4 | `search/` `SearchProvider` trait + factory | Mencionado sin firma | `doc/search-providers.md` |
| G5 | `llm/provider.rs` `LlmProvider` trait + relación OpenAIClient/session | Sin doc | `doc/llm-provider.md` |
| G6 | `audio/filler.rs`, `audio/audio_transform.rs` (resampling, filler silence) | Sin doc | `doc/audio-internals.md` |
| G7 | `dream/` formato JSONL + algoritmo compaction L1→L2 + schema FTS5 | Nivel alto | `doc/s-dream-format.md` |
| G8 | Contradicciones de defaults (SENECHAL_LANGUAGE, VAD_SILENCE_MS, LLM_CONTEXT_TOKENS, TTS_PROVIDER, endpoints `/barge-in` vs `/control/barge_in`) | Conflictos | `doc/env-vars.md` + `doc/ARCHITECTURE.md` corregidos |

### Supuestos

- Se asume que el agente de build ejecutará `make qa` (fmt/lint/test/test-ci/test-e2e/build) tras cada checkpoint que compile; nunca se rompe `master` accediendo a verde.
- `Cargo.toml` actual se convierte en el workspace root; los crates hijos heredan `rust-toolchain.toml`.
- No se eliminan módulos en Fase 1–2 (solo doc). La eliminación ocurre en Fase 6 tras verde.
- El plan NO rewrittea módulos: solo **mueve** a crates y añade `features` + `pub use`; la lógica interna se preserva como fue escrita.

---

## Phase 0 — Decisión y registro de salvables (doc-only)

- [x] Step 0.1: Crear `doc/CARVEOUT-DECISIONS.md` con el inventario de salvables (la tabla de más arriba) y el layout destino de crates.
  - File(s): `doc/CARVEOUT-DECISIONS.md` (nuevo)
  - Change: Volcar el inventario y el target layout: `seneschal-core`, `seneschal-mcp`, `seneschal-agents`, `seneschal-plugins`, `seneschal-control`, `seneschal-remote`, `seneschal-search`, `seneschal-memory` (db+memory+profile+dream), `seneschal-tools-core`, `seneschal-classifier`, `seneschal-extras` (daemon/eyes/agent_session/screen_capture), `seneschal-tui` (status-only). Indicar qué se descarta (classifier embedding/logistic, piper.rs, .bak).
  - Acceptance: Existe el archivo; un nuevo contribuidor entiende el destino de cada `src/*` en <2 min.
- [x] Step 0.2: Añadir a `doc/CARVEOUT-DECISIONS.md` un diagrama ASCII del workspace y la lista de dependencias entre crates propuestas (p.ej. `seneschal-core` no depende de `seneschal-mcp`; `seneschal-plugins` depende de `seneschal-mcp`+`seneschal-agents`).
  - File(s): `doc/CARVEOUT-DECISIONS.md`
  - Change: Diagrama y matriz de dependencias.
  - Acceptance: La matriz es acíclica (verificable a ojo); `seneschal-core` aparece como hoja sin dependencias hacia otros crates del workspace.
- [ ] Commit checkpoint 0.2: `docs: record carve-out decisions and crate layout` (solo doc, no compila-cambia).

---

## Phase 1 — Reconciliar contradicciones de doc (G8)

- [x] Step 1.1: Fijar ground-truth de env vars leyendo `seneschal.pro.toml` y `src/config.rs`.
  - File(s): `doc/env-vars.md`
  - Change: Recorrer los 6 conflictos detectados: `SENECHAL_LANGUAGE`, `VAD_SILENCE_MS`, `LLM_CONTEXT_TOKENS`, `TTS_PROVIDER`, `AVSPEECH_VOICE`, `LLM_MAX_TOKENS`. Para cada uno, mostrar **un** valor canónico (el de `seneschal.pro.toml`/`config.rs` como verdad) y marcar como nota las fuentes que discrepaban. Añadir un párrafo "Single source of truth: `seneschal.pro.toml` + `src/config.rs::Default`".
  - Acceptance: `rg -n "VAD_SILENCE_MS" doc/` devuelve un único valor numérico.
- [x] Step 1.2: Unificar nombres de endpoints del Control API.
  - File(s): `doc/ARCHITECTURE.md`, `doc/MAIN_PROCESS.md`, `readme.md`
  - Change: Elegir el prefijo `/control/` y la forma `snake_case` (`/control/barge_in`, `/control/events`, etc.). Verificar contra `src/control/api.rs` cuál es el canónico y dejar todas las doc consistentes.
  - Acceptance: `rg -n "/barge-in\b|/barge_in|/events\b" doc/` es coherente (sin `/barge-in` sin prefijo).
- [x] Step 1.3: Alinear nombres del FSM.
  - File(s): `doc/PROCESS_ARCHITECTURE.md`
  - Change: Reemplazar `Idle → Stt → Llm → Responding` por `Idle, Listening, Thinking, Speaking, Paused` (los de `src/pipeline/fsm.rs` y `doc/CONSTITUTION.md`).
  - Acceptance: `rag` de FSM states en doc converge con `enum PipelineState` real.
- [x] Step 1.4: Revisar `readme.md` y `doc/doc.md` referencias a "Jarvis" / `VOICEBOT_LANGUAGE` y renombrar a "Seneschal" / `SENECHAL_LANGUAGE` según `AGENTS.md`.
  - File(s): `readme.md`, `doc/doc.md`
  - Change: Reemplazar tokens obsoletos.
  - Acceptance: `rg -n "Jarvis|VOICEBOT_" doc/ readme.md` sin matches en contexto técnico.
- [ ] Commit checkpoint 1.4: `docs: reconcile env var, endpoint and naming contradictions`.

---

## Phase 2 — Rellenar gaps de doc (G1–G7)

Cada paso produce una doc **espec-nivel** (interfaces, tipos, dataflow), leyendo el código fuente correspondiente.

- [x] Step 2.1: `doc/classifier.md` (G1) — cascada `heuristic → keyword → fallback`. Firmas del `Intent` enum, `ClassifierPipeline`, niveles, env vars (`CLASSIFIER_*` de `seneschal.pro.toml`); marcar `embedding.rs`/`logistic.rs` como **descartables (placeholders vacíos)**.
  - File(s): `doc/classifier.md` (nuevo)
  - Acceptance: Un reader puede replicar la cascada sin mirar el `.rs`.
- [x] Step 2.2: `doc/plugins-internal.md` (G2) — lifecycle del `PluginManager`, `PluginSwitchEvent`, formato `Manifest`, revert de `config_overrides`, inyección de prompts, relación con `mcp_spawner` y `agent_bridge`.
  - File(s): `doc/plugins-internal.md` (nuevo)
  - Acceptance: Diagrama de lifecycle activar→modificar-config→spawn-mcp→register-tools→deactivate→revert.
- [x] Step 2.3: `doc/agents-internal.md` (G3) — `AcpSessionManager`, `AcpWriter`, state machine de sesión, dual Hermes/OpenCode (`hermes_events.rs`, `opencode_events.rs`, `opencode_transport.rs`), `ProactiveEvent`.
  - File(s): `doc/agents-internal.md` (nuevo)
  - Acceptance: Estados de sesión y transiciones listados; diferencias Hermes vs OpenCode en tabla.
- [x] Step 2.4: `doc/search-providers.md` (G4) — `SearchProvider` trait (métodos), factory, credenciales/env vars por provider (`BRAVE_*`, `TAVILY_*`, etc.), configuración de rate-limit si aplica.
  - File(s): `doc/search-providers.md` (nuevo)
  - Acceptance: Firma del trait en bloque Rust idéntica a `src/search/mod.rs`.
- [x] Step 2.5: `doc/llm-provider.md` (G5) — `LlmProvider` trait, relación con `OpenAIClient` y `LlmSession`, cómo se selecciona provider, ThinkFilter (patrón real de tags — verificar en `src/llm/` antes de escribir).
  - File(s): `doc/llm-provider.md` (nuevo)
  - Acceptance: Indica el tag exacto de reasoning que se stripa (leer código para confirmar `<antThinking>` vs otro).
- [x] Step 2.6: `doc/audio-internals.md` (G6) — `AudioOutput` (incl. `null()`), resampling rubato (`audio_transform.rs`), ring buffer (`buffer.rs`), `filler.rs` (qué inyecta y cuándo), `ambient_buffer.rs` (política de eviction).
  - File(s): `doc/audio-internals.md` (nuevo)
  - Acceptance: Esquemas de datos y ventanas temporales documentados.
- [x] Step 2.7: `doc/s-dream-format.md` (G7) — formato JSONL de L2, schema FTS5 existente, triggers L1→L2 (idle 600s, interval 3600s, 3 AM), algoritmo de compaction de hechos de baja confianza, `recover_historical_context`.
  - File(s): `doc/s-dream-format.md` (nuevo)
  - Acceptance: Ejemplo JSONL completo y ejemplo query FTS5.
- [x] Step 2.8: Actualizar `doc/modules.md` para marcar los módulos recién documentados y los descartados (piper.rs, classifier embedding/logistic, .bak).
  - File(s): `doc/modules.md`
  - Change: Tabla con columna "estado" (salvar/aislar/descartar) que enlace a la nueva doc.
  - Acceptance: `doc/modules.md` no contiene entradas vivas para archivos descartados.
- [ ] Commit checkpoint 2.8: `docs: fill critical gaps (classifier, plugins, agents, search, llm-provider, audio-internals, s-dream)`.

---

## Phase 3 — Spec del layout workspace (doc-only, antes de mover código)

- [x] Step 3.1: Definir `doc/CARVEOUT-LAYOUT.md` con el `Cargo.toml` workspace raíz objetivo y un `crates/<name>/Cargo.toml` template (features, deps, `edition`).
  - File(s): `doc/CARVEOUT-LAYOUT.md` (nuevo)
  - Change: Listar cada crate con su `path`, su `[features]` propuesto, sus deps internas y la feature flag que lo activa desde el binario principal.
  - Acceptance: Cada crate del inventario tiene su sección; `seneschal-core` no declara dependencias hacia otros crates del workspace.
- [x] Step 3.2: Definir el map "archivo `src/X` → `crates/.../src/X`" como tabla exhaustiva. Incluir `main.rs` (qué importa de qué crate) y `lib.rs` (re-exports que sobreviven).
  - File(s): `doc/CARVEOUT-LAYOUT.md`
  - Change: Tabla 2 columnas `origen → destino` con todos los `.rs` listados por el explore agent.
  - Acceptance: Todos los ~110 `.rs` aparecen una sola vez en la tabla.
- [ ] Commit checkpoint 3.2: `docs: spec crate workspace layout and file map`.

---

## Phase 4 — Scaffold del workspace (primeTo compile)

- [x] Step 4.1: Convertir `Cargo.toml` raíz en workspace `[workspace] members=[...]`; crear `crates/` vacío con los subdirectorios y `Cargo.toml` mínimos (sin mover código todavía).
  - File(s): `Cargo.toml`, `crates/*/Cargo.toml` (nuevos vacíos)
  - Change: `members = ["crates/seneschal-core", ...]`; workspace hereda `rust-toolchain.toml`.
  - Acceptance: `cargo metadata --no-deps` resuelve el workspace sin error (aún sin código movido).
- [x] Commit checkpoint 4.1: `build: scaffold empty cargo workspace` — **verificar `cargo build` sigue verde** antes de seguir.

---

## Phase 5 — Carve-out del núcleo (`seneschal-core`)

El orden: mover el núcleo primero y dejarlo verde para tener una base estable donde colgar el resto.

- [ ] Step 5.1: Mover `src/audio/{audio_capture,audio_transform,buffer,ambient_buffer,output}.rs`, `src/stt/{provider,whisper,no_speech_gate}.rs`, `src/llm/{client,session,provider}.rs`, `src/tts/{mod,sentence,avspeech,kokoro}.rs`, `src/pipeline/*` a `crates/seneschal-core/src/...`.
  - File(s): `crates/seneschal-core/src/**` (mover, no reescribir)
  - Change: `git mv` los archivos. Ajustar `mod` declarations. Re-exportar públicamente lo que `main.rs`/`lib.rs` consumían.
  - Acceptance: `cargo build -p seneschal-core` pasa.
- [ ] Step 5.2: Reducir `src/config.rs` — extraer a `seneschal-core` solo los campos usados por el núcleo (audio, stt, llm, tts, pipeline). El resto queda en `src/config.rs` del binario principal.
  - File(s): `crates/seneschal-core/src/config.rs` (nuevo), `src/config.rs` (recortado)
  - Change: El crate core expone `CoreConfig`; el binario principal compone `CoreConfig` + config extendida.
  - Acceptance: Compila ambos; `cargo test -p seneschal-core` pasa.
- [ ] Step 5.3: Ajustar `main.rs` para importar de `seneschal-core`. Eliminar imports rotos.
  - File(s): `src/main.rs`
  - Change: `use seneschal_core::{...}` en lugar de paths `crate::`.
  - Acceptance: `cargo build` verde (resto de módulos acretados siguen en `src/`).
- [ ] Commit checkpoint 5.3: `refactor(core): carve out seneschal-core crate` — **`make qa` debe seguir verde** (igualar tests si cambiaron rutas de mod). Verificar y corregir antes de continuar.

---

## Phase 6 — Eliminar dead code (post-verde)

- [ ] Step 6.1: `git rm src/tts/piper.rs`, `src/bin/bench_pipeline.rs.bak`, `src/classifier/embedding.rs`, `src/classifier/logistic.rs`.
  - File(s): los 4 archivos
  - Change: Borrar y quitar referencias en `Cargo.toml` (feature `classifier-embedding`), `tts/mod.rs`, `bin/`.
  - Acceptance: `cargo build` verde; `rg piper src/tts/mod.rs` no existe.
- [x] Commit checkpoint 6.1: `chore: remove dead code (piper, classifier stubs, .bak)` — **QA verde**.

---

## Phase 7 — Carve-out de crates acretados (uno por commit, verde entre cada)

Orden por dependencia: `mcp` y `search` primero (hojas), luego `agents` (dep de mcp?), `memory` (db), `tools-core`, `classifier`, `plugins` (dep de mcp+agents), `control`, `remote`, `extras`, `tui`.

- [x] Step 7.1: `seneschal-search` — mover `src/search/*`. Exponer `SearchProvider` trait y factory. Mark feature en binario principal. Doc recalibrada con Step 2.4.
  - File(s): `crates/seneschal-search/src/**`
  - Acceptance: `cargo build -p seneschal-search` verde; `make qa` verde.
- [x] Step 7.2: `seneschal-mcp` — mover `src/mcp/{mod,config,transport}.rs`. Doc de `doc/ARCHITECTURE-MCP-LAYER.md` pasa a ser spec viva; marcar Gaps 1,3,6 como **no implementados** (separar "implemented" vs "proposed").
  - File(s): `crates/seneschal-mcp/src/**`
  - Acceptance: build + QA verde.
- [ ] Step 7.3: `seneschal-memory` — mover `src/db/`, `src/memory/`, `src/profile/`, `src/dream/`. Documentar formato (Step 2.7). Depende de `seneschal-core` solo por tipos mínimos de config (o `seneschal-core` expone un sub-crate de tipos — decisión diferida al agente de build si lo ve necesario).
  - File(s): `crates/seneschal-memory/src/**`
  - Acceptance: verde.
- [x] Step 7.4: `seneschal-agents` — mover `src/agents/*` y `src/agent_session.rs`. Separar transport Hermes vs OpenCode (ya separados por archivo). Doc 2.3.
  - File(s): `crates/seneschal-agents/src/**`
  - Acceptance: verde; ACP tests (si `test-ci` las incluye) pasan.
- [ ] Step 7.5: `seneschal-tools-core` — herramientas esenciales (`shell`, `clipboard`, `current_time`, `read_file`, `take_screenshot`, `open_app`, `quick_search`). Depende de `seneschal-search`. Exponer el trait `Tool` si vive aquí (verificar dónde está `tools/mod.rs`).
  - File(s): `crates/seneschal-tools-core/src/**`
  - Acceptance: herramientas no esenciales (`deep_research`, `run_agent`, `mcp_tool`, `recover_historical_context`, `switch_plugin`, `prompt_build`, `conversation_mode`) quedan en el binario principal por ahora (se reubican en sus crates cuando toquen sus fases).
- [ ] Step 7.6: `seneschal-classifier` — mover `src/classifier/{mod,heuristic,keyword,pipeline,fallback}.rs` (sin embedding/logistic, ya borrados). Doc 2.1.
  - File(s): `crates/seneschal-classifier/src/**`
  - Acceptance: verde.
- [ ] Step 7.7: `seneschal-plugins` — mover `src/plugins/*`. Depende de `seneschal-mcp` + `seneschal-agents`. Doc 2.2.
  - File(s): `crates/seneschal-plugins/src/**`
  - Acceptance: verde; plugin switch tests pasan.
- [ ] Step 7.8: `seneschal-control` — mover `src/control/*`. Endpoints canónicos de Step 1.2.
  - File(s): `crates/seneschal-control/src/**`
  - Acceptance: verde; tests e2e del control pasan.
- [ ] Step 7.9: `seneschal-remote` — mover `src/remote/*`.
  - File(s): `crates/seneschal-remote/src/**`
  - Acceptance: verde.
- [x] Step 7.10: `seneschal-extras` — mover `src/{daemon,eyes,screen_capture,device_monitor,i18n}.rs` y `src/tools/{deep_research,run_agent,recover_historical_context,switch_plugin,prompt_build,conversation_mode,subtask,noop,open_terminal}.rs` (herramientasopcional/unstable). Feature flag default off.
  - File(s): `crates/seneschal-extras/src/**`
  - Acceptance: verde con `--features extras`; sin feature, el binario principal sigue verde.
- [x] Step 7.11: `seneschal-tui` (status-only) — mover `src/tui/*` reducido al panel de conversación + estado (NO al `acp_panel` si `doc/ARCHITECTURE-MCP-LAYER.md` recomienda deprecación). Re-evaluar con user antes de finalizar: siになるstatus-only, dejarlo; si no, marcar deprecated y feature default off.
  - File(s): `crates/seneschal-tui/src/**`
  - Acceptance: verde con `--features tui`.
- [ ] Commit checkpoint tras CADA sub-step 7.x (verde intermedio). Commit final: `refactor: split accreted features into standalone crates`.

---

## Phase 8 — Feature flags y wiring final

- [ ] Step 8.1: Reescribir el `Cargo.toml` del binario principal para que cada crate acretado sea opt-in via `[features]` (`mcp`, `agents`, `plugins`, `control`, `remote`, `memory`, `classifier`, `extras`, `tui`). Default profile = núcleo + `tools-core` + `memory` (mínimo útil).
  - File(s): `Cargo.toml` (binario), `doc/build-features.md`
  - Change: Espejar el cambio en `doc/build-features.md` con la tabla actualizada.
  - Acceptance: `cargo build` (default) = núcleo lean; `cargo build --features full` = todo.
- [ ] Step 8.2: Ajustar `main.rs` para construir el app condicionalmente según features (los bloques `#[cfg(feature=...)]` ya existentes deben reubicarse a los nuevos paths).
  - File(s): `src/main.rs`
  - Acceptance: build default y build full pasan; `cargo run` sin features arranca el pipeline de voz.
- [ ] Step 8.3: Actualizar `Makefile`targets `qa`/`qa-full` y `doc/AGENTS.md` QA Workflow con los feature sets del nuevo layout.
  - File(s): `Makefile`, `AGENTS.md`
  - Acceptance: `make qa` y `make qa-full` siguen siendo los comandos canónicos.

---

## Phase 9 — Validación final y limpieza

- [ ] Step 9.1: Ejecutar `make qa-full` end-to-end y corregir.
  - Acceptance: verde completo.
- [ ] Step 9.2: Actualizar `CHANGELOG.md` (vía skill `/changelog` si hay milestone) con "modular workspace carve-out" como breaking change de arch.
  - File(s): `CHANGELOG.md`
  - Acceptance: entrada describe el nuevo layout y feature flags.
- [ ] Step 9.3: Actualizar `doc/CARVEOUT-DECISIONS.md` marcando cada fila como `[x] done` y los crates creados.
  - Acceptance: el inventario refleja el estado final.
- [ ] Commit final: `refactor: modular workspace carve-out complete` — propagar a `master` solo tras `make qa-full` verde.

---

## Riesgos y checkpoints

- **Checkpoint de compilación tras cada move**: nunca avanzar al siguiente Step 7.x sin `cargo build` verde.
- **Checkpoint de QA tras fases completas**: tras 5.3, 6.1, 7.x final, 8.x final.
- **Riesgo de acoplamiento oculto**: si `seneschal-core` termina necesitando tipos de `seneschal-mcp` (p.ej. el trait `Tool` vive en `tools/mod.rs` que dependía de MCP), desempatar introduciendo un `seneschal-traits` o moviendo el trait a `seneschal-tools-core`. Detallar la decisión en `doc/CARVEOUT-DECISIONS.md` si ocurre.
- **Riesgo de doc-stale tras mover**: cada crate debe incluir un `README.md` mínimo apuntando a la doc canónica de `doc/`. (Esta doc vive en `doc/` centralizada; los crate-README son solo índices.)
---

## Session Progress Log (2026-07-29)

### Completed (10 commits, 715 tests pass)

| Phase | Commit | What |
|-------|--------|------|
| 0 | `88238f4` | `doc/CARVEOUT-DECISIONS.md` — salvage inventory |
| 1 | `e2728de` | Doc reconciliation (6 contradictions, endpoints, FSM, naming) |
| 2 | `8340934` | 7 gap docs filled |
| 3 | `0fafd57` | `doc/CARVEOUT-LAYOUT.md` — workspace spec + file map |
| 4 | `c2c1363` | Workspace scaffold (13 crates, build green) |
| 5a | `df02959` | `seneschal-common` — shared types |
| 5b | `c33a088` | `seneschal-core` — voice pipeline (72 files moved) |
| 6 | `3d07c0b` | Dead code removed (piper, classifier stubs, .bak) |
| 7.1 | `517e85d` | `seneschal-search` — search providers |
| 7.2-7.4 | `1137547` | `seneschal-mcp`, `seneschal-control`, `seneschal-remote` |
| 7.10+7.8 | `ea82a50` | `seneschal-classifier` + `seneschal-memory` — classifier wrapper + S-DREAM daemon |
| 7.6 | `d0e69a7` | `seneschal-agents` — multi-agent session management (AcpWriter extracted to common) |
| 7.5 | `83e53c1` | `seneschal-tui` — terminal UI (ConversationMode extracted to common) |
| 7.7 | `c7f8a86` | `seneschal-plugins` — plugin lifecycle (McpToolProxy extracted to mcp) |

### Crates done (11/13)

| Crate | Files | Dependencies |
|-------|-------|-------------|
| `seneschal-common` | config, db, tools, events, classifier, i18n | leaf |
| `seneschal-core` | audio, stt, llm, tts, pipeline, memory, profile | common |
| `seneschal-search` | brave, tavily, exa, searxng | common |
| `seneschal-mcp` | mcp client (JSON-RPC 2.0) | common |
| `seneschal-control` | HTTP REST + SSE API | common, core |
| `seneschal-remote` | WebSocket server | common, core, control |
| `seneschal-classifier` | thin wrapper re-exporting from common | common |
| `seneschal-memory` | dream (S-DREAM daemon) | common, core |
| `seneschal-agents` | agents, agent_session, acp_writer | common |
| `seneschal-tui` | tui (status-only) | common, agents, core |
| `seneschal-plugins` | plugins (5/6 files; agent_bridge stays in src/) | common, agents, mcp |

### Pending (2 crates + Phase 8-9)

| Step | Crate / Task | Blocker / Notes |
|------|-------------|-----------------|
| 7.9 | `seneschal-tools-core` | 8 essential tools; depends on search, agents, mcp |
| 7.11 | `seneschal-extras` | daemon, eyes, screen_capture, device_monitor, remaining tools |
| 8 | Feature flags | `Cargo.toml` [features], `main.rs` conditional compilation |
| 9 | QA final | `make qa-full`, CHANGELOG |

### How to resume

1. Read this file (`.opencode/tasks/refactor-core-carveout.md`)
2. Read `doc/CARVEOUT-DECISIONS.md` and `doc/CARVEOUT-LAYOUT.md`
3. `cargo build --features tui,remote,control` — should be green
4. `cargo test --features tui,remote,control` — should be 695 passed, 0 failed
5. Continue from Step 7.6 (seneschal-agents) — unblocks tui and plugins. Or do 7.9 (tools-core) or 7.11 (extras) if feeling adventurous.
6. Key pattern for each crate:
   a. `mkdir -p crates/<name>/src/<module>` + `git mv src/<module>/*` (preserve directory structure)
   b. Write `Cargo.toml` with proper deps + `lib.rs` with `pub mod <module>;`
   c. Fix `crate::xxx` → `seneschal_xxx::module::` in moved files (sed or manual)
   d. Add dep to root `Cargo.toml`, remove `mod` from `src/lib.rs` and `src/main.rs`
   e. Fix remaining imports in `src/` files
   f. `cargo build --features tui,remote,control` + `cargo test` — green before commit
