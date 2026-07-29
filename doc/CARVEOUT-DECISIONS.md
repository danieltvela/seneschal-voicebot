# Seneschal — Carve-Out Decisions

> Decision record for the modular workspace carve-out.  
> Origin: direct user instruction — project has outgrown the original goal (~25k LOC, ~110 `.rs`, ~30 docs).  
> Direction: **salvage inventory + carve-out**, keep extended vision with strict module boundaries.  
> Date: 2026-07-28

## 1. Salvage Inventory

| Estado | Módulo (`src/`) | Destino | Razón |
|--------|------------------|---------|-------|
| SALVAR | `audio/` (audio_capture, audio_transform, buffer, ambient_buffer, output, filler, speaker, mod) | `seneschal-core` | Pipeline de voz esencial |
| SALVAR | `stt/` (provider, whisper, no_speech_gate, parakeet, speech_recognizer, mod) | `seneschal-core` | Whisper+VAD backend principal |
| SALVAR | `llm/` (client, session, provider, manager, mod) | `seneschal-core` | Contrato OpenAI-SSE |
| SALVAR | `tts/` (mod, sentence, avspeech, kokoro) | `seneschal-core` | AvSpeech+Kokoro+splitter |
| SALVAR | `pipeline/` (mod, frames, fsm, state, llm_task, sen_task, tts_task, consolidation) | `seneschal-core` | FSM y actores del pipeline |
| SALVAR (dividir) | `config.rs` | Parte a `seneschal-core`, resto a cada crate | Single source-of-truth |
| SALVAR | `db/` | `seneschal-memory` | SQLite + FTS5 |
| SALVAR | `tools/` (shell, clipboard, current_time, read_file, take_screenshot, quick_search, open_app) | `seneschal-tools-core` | Herramientas razonable mínimo |
| AISLAR | `mcp/` (mod, config, transport) | `seneschal-mcp` | Maduro, útil, pero separar |
| AISLAR | `agents/` (mod, config, session_manager, session_events, hermes_events, opencode_events, opencode_transport) | `seneschal-agents` | Complejo; aislar |
| AISLAR | `plugins/` (mod, manager, manifest, mcp_spawner, agent_bridge, prompt_injection, config_overrides) | `seneschal-plugins` | Conservado con visión ampliada |
| AISLAR | `control/` (mod, api, state, broadcast, client) | `seneschal-control` | API REST+SSE |
| AISLAR | `remote/` (mod, protocol, server, tests) | `seneschal-remote` | WebSocket para clientes remotos |
| AISLAR | `search/` (mod, brave, tavily, exa, searxng, tests) | `seneschal-search` | SearchProvider trait |
| AISLAR | `memory/`, `profile/`, `dream/` | `seneschal-memory` | S-Dream L1/L2, perfil, memorias |
| AISLAR | `classifier/` (mod, heuristic, keyword, pipeline, fallback) | `seneschal-classifier` | Solo la parte funcional |
| AISLAR | `tui/` (mod, app, ui, events, input, acp_panel) | `seneschal-tui` | Status-only, feature-gated |
| AISLAR | `daemon.rs`, `eyes.rs`, `screen_capture.rs`, `agent_session.rs`, `device_monitor.rs`, `i18n.rs` | `seneschal-extras` | Opcional/unstable |
| AISLAR | `tools/` restantes (deep_research, run_agent, mcp_tool, recover_historical_context, switch_plugin, prompt_build, conversation_mode, subtask, noop, open_terminal) | `seneschal-extras` + crates correspondientes | Según el crate dueño |
| DESCARTAR | `classifier/embedding.rs`, `classifier/logistic.rs` | ELIMINAR | Feature `classifier-embedding` vacío en Cargo.toml |
| DESCARTAR | `tts/piper.rs` | ELIMINAR | No declarado en `tts/mod.rs`; dead code |
| DESCARTAR | `bin/bench_pipeline.rs.bak` | ELIMINAR | Backup stale en árbol fuente |

## 2. Target Workspace Layout

```
seneschal-voicebot/                  # Cargo workspace root
├── Cargo.toml                       # [workspace] members = [...]
├── src/
│   ├── main.rs                      # Binary entry point
│   ├── lib.rs                       # Re-exports
│   └── config.rs                    # Config compose (CoreConfig + extras)
├── crates/
│   ├── seneschal-core/              # Voice pipeline (no external deps)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs            # CoreConfig
│   │       ├── audio/
│   │       ├── stt/
│   │       ├── llm/
│   │       ├── tts/
│   │       └── pipeline/
│   ├── seneschal-search/            # Search providers
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-mcp/               # MCP client
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-agents/            # Multi-agent delegation
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-plugins/           # Plugin system
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-control/           # Control API
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-remote/            # Remote WebSocket
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-memory/            # DB + memory + profile + dream
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-tools-core/        # Essential LLM tools
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-classifier/        # Intent classifier
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── seneschal-extras/            # daemon, eyes, agent_session, etc.
│   │   ├── Cargo.toml
│   │   └── src/
│   └── seneschal-tui/               # Terminal UI (status-only)
│       ├── Cargo.toml
│       └── src/
│   └── README.md (opcional por crate)
```

## 3. Dependency Matrix (proposed)

```
                     ┌─────────┐
                     │  main   │ (binary)
                     └────┬────┘
            ┌─────────────┼─────────────┐
            ▼             ▼             ▼
    ┌───────────┐  ┌───────────┐  ┌───────────┐
    │core       │  │memory     │  │tools-core │
    └───────────┘  └───────────┘  └─────┬─────┘
                         │              │
              ┌──────────┼──────────────┤
              ▼          ▼              ▼
       ┌──────────┐ ┌──────────┐ ┌──────────┐
       │search    │ │mcp       │ │agents    │
       └──────────┘ └──────────┘ └──────────┘
              │          │              │
              └──────────┼──────────────┘
                         ▼
                  ┌──────────┐
                  │plugins   │
                  └──────────┘
```

```
┌────────────────────┬─────┬──────┬─────┬──────┬──────┬──────┬───────┬──────┬──────┬──────┬──────┬──────┐
│ depends on →       │core │search│mcp  │agents│plugs │ctrl  │remote│memory │tools │class │extras│tui   │
├────────────────────┼─────┼──────┼─────┼──────┼──────┼──────┼───────┼──────┼──────┼──────┼──────┼──────┤
│ seneschal-core     │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-search   │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-mcp      │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-agents   │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-plugins  │  -  │  -   │ yes │ yes  │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-control  │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-remote   │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-memory   │ yes │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-tools-c  │  -  │ yes  │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-classif  │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-extras   │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ seneschal-tui      │  -  │  -   │  -  │  -   │  -   │  -   │   -   │  -   │  -   │  -   │  -   │  -   │
│ main (binary)      │ yes │ yes  │ yes │ yes  │ yes  │ yes  │  yes  │ yes  │ yes  │ yes  │ yes  │ yes  │
└────────────────────┴─────┴──────┴─────┴──────┴──────┴──────┴───────┴──────┴──────┴──────┴──────┴──────┘
```

- `seneschal-core` es hoja: no depende de ningún otro crate del workspace.
- `seneschal-plugins` es el único crate interno que depende de otros crates del workspace (`mcp` + `agents`).
- Todos los demás crates son independientes entre sí (solo dependen de crates externos y de `seneschal-core` si se requiere types sharing).

## 4. Decisions Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-07-28 | Doc first, code second | Reconcile contradictions and fill gap docs before touching source. |
| 2026-07-28 | Keep extended vision with strict boundaries | Voice nucleus + agents + plugins + MCP are all useful; separate into feature-flag-gated crates. |
| 2026-07-28 | `seneschal-core` as leaf crate | Prevents circular dependency hell; core exposes types that other crates consume via their own `Cargo.toml` dep. |
| 2026-07-28 | `tools-core` separate from `tools/mod.rs` | Only essential tools (shell, clipboard, time, read_file, screenshot, open_app, quick_search). Rest stay with their respective crates or grab-bag extras. |
| 2026-07-29 | Carve-out complete — 13/13 crates | All modules moved to workspace crates. QA pipeline green. See `CHANGELOG.md`: [Unreleased] — Modular Workspace Carve-Out. |
| 2026-07-28 | Dead code removed before crate moves (Phase 6) | Clean removal avoids `git mv` noise on files we know are unused. |
