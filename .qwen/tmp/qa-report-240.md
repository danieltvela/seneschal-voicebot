## Investigación: QA avanzado de `make qa` (no unit tests)

Todos los datos de abajo están verificados contra el árbol de trabajo en `master` (2026-09-12). Se cita archivo:línea para cada afirmación.

### 1. Qué es "avanzado" dentro del harness

`make qa` delega todo en `scripts/qa.sh`; el Makefile es solo un envoltorio fino. Los stages se agrupan en dos modos:

| Stage | Comando real | Entra en `make qa` | Entra en `make qa-full` |
|---|---|---|---|
| `fmt` | `cargo fmt --check --all` | sí | sí |
| `lint` | `cargo clippy --all-targets --no-deps -- -D warnings` | sí | sí |
| `test` | `cargo test --quiet` | sí | sí |
| `test-ci` | `cargo test --features full --quiet` | sí | sí |
| **`test-e2e`** | `cargo test --features full e2e -- --ignored` | **sí** | sí |
| `build` | `cargo build --features full` | sí | sí |
| **`test-stt`** | `cargo test --workspace -- --ignored stt` | no | **sí** |
| **`test-llm`** | `cargo test -- --ignored llm` | no | **sí** |
| **`audit`** | `cargo audit` | no | **sí** |
| **`coverage`** | `cargo llvm-cov --workspace --summary-only` | no | **sí** |

`fast` se aborta al primer fallo (`run_fast`, qa.sh:222-229); en `full` los cuatro avanzados corren con `|| true`, es decir **no bloquean** el veredicto (qa.sh:232-237). `test-e2e` es el único test avanzado que puede hacer fallar `make qa`.

Controles de escape: `QA_SKIP=<csv>`, `QA_KEEP_GOING=1`, `CARGO=<bin>`, `QA_NO_COLOR=1`.

---

### 2. `test-e2e` — el harness wiremock (27 tests)

Vive en `src/e2e_tests.rs` (1371 líneas), montado como `#[cfg(test)] mod e2e_tests` de `main.rs` (src/main.rs:78-79). Por eso el filtro `e2e` coincide con **todos** los tests del módulo (su ruta es `e2e_tests::<nombre>`), incluidos los que no llevan "e2e" en el nombre.

**Cómo se monta (`E2eHarness::with_system_prompt`, líneas 108-155):**
- LLM: `MockServer::start()` + `OpenAiLlmProvider::new(&server.uri(), "test-model", 400, 0.0)`. Inyección por constructor — **no** usa `LLM_URL` ni `Config::from_env()`. El mock responde a `POST /v1/chat/completions` con `text/event-stream`.
- TTS: `TtsEngine::Mock(MockTts)` con un `captured` que acumula cada frase → es el punto de observación principal.
- Salida de audio: `AudioOutput::null()`, que deja `device: None` (audio/output.rs:232-244) y `has_device() == false`. **No abre dispositivo CPAL.**
- DB: SQLite real en `tempfile::TempDir` → las aserciones sobre persistencia son reales.
- Lo que se ejecuta son las **tareas de producción de verdad**: `llm_task`, `sen_task`, `tts_task` conectadas por canales tokio (líneas 231-320). No hay simulación del pipeline.

**Qué comprueba, por familias (27 ignored tests):**
- **Pipeline básico (8):** respuesta llega a TTS y se guarda en DB; transcript vacío se descarta sin llamada al LLM ni escritura; modo ambiente sin wake word calla; con wake word responde; lista de múltiples wake words (issue #228) acepta cualquiera y rechaza si ninguna casa; respuesta multi-frase se parte en varias síntesis; dos turnos acumulan 2 mensajes `Assistant` en DB.
- **Transiciones de modo de conversación (3):** wake word del usuario principal pasa `Ambient → Active` y el segundo turno ya no necesita wake word; voz secundaria en `AmbientLocked` responde pero **no** destraba el modo; discurso de hablante secundario en `Active` no rompe el pipeline.
- **Barge-in (11):** durante LLM+TTS simultáneos, durante herramienta, durante pausa (debe descartar), spam de 5 cancelaciones seguidas, en respuesta multi-frase, en idle, antes del primer token, en mitad de stream, durante reproducción, durante encolamiento, y reset limpio de estado tras cancelar. Las aserciones fuertes son `assert_db_has_partial_save()` (la respuesta parcial **debe** quedar persistida) y "TTS ya había empezado".
- **Latencia (1):** `latency_transcript_to_tts_under_1500ms` — inyecta transcript y mide hasta `llm_post_finished` y hasta la primera frase TTS; puerta dura `< 1500 ms` en ambos.
- **Deep worker / issue #236 (2):** `e2e_deep_worker_delegation_flow` levanta **un segundo** `MockServer` para el endpoint del worker y verifica cuatro cosas: el ack "Procesando en segundo plano" se pronuncia una vez; los argumentos de `run_deep_task` quedan **abreviados** en el historial de voz (anti-#221); el endpoint del worker recibe ≥2 peticiones (flujo de thinking externo en dos pasos); el plan interno del worker **no se filtra** al historial de voz. `e2e_deep_worker_disabled_is_transparent` comprueba que sin la herramienta registrada el pipeline se comporta igual y `run_deep_task` no aparece en `tool_definitions()`.
- **STT real (2):** ver §3 — están aquí pero se auto-saltan.

**Qué NO comprueba:** ninguna aserción sobre el **cuerpo de las peticiones** enviadas al LLM (`src/e2e_tests.rs` no usa `.expect()` ni `body_json`; sólo `crates/seneschal-search/src/search/tests.rs:163-171` y `crates/seneschal-agents/src/opencode_transport.rs:808` emplean matchers/contadores). Tampoco cubre TTS real (Kokoro/AVSpeech), capturador de micrófono, hablante/speaker-id real, MCP, ni el modo `--list-devices`.

> Drift de docs: `doc/e2e.md:29` afirma que "the CPAL device handle must open successfully". Ya no es cierto — el harness usa `AudioOutput::null()` con `device: None`. `doc/e2e.md:80` además referencia `tests/fixtures/hola.wav`, que no existe.

---

### 3. `test-stt` — auditoría STT con Whisper real (14 tests)

Tres binarios de integración en `crates/seneschal-core/tests/` (el **único** directorio `tests/` del workspace), compartidos vía `#[path = "stt_common/mod.rs"]`:

- **`stt_audit_continuous.rs` (4):** habla continua >20 s en ES/EN → ≥2 segmentos, el primero cierra dentro de `19.0..=21.5 s` (respeta el tope duro `MAX_SEGMENT_MS=20_000`), ningún segmento de cabecera <5 s, `SpeechStart` a <1,5 s del onset RMS, `SpeechEnd` a <15 s del fin de voz, y `stt_audit_latency_baseline` con **puerta dura de RTF < 0,5** en régimen estacionario (líneas 194-198) más comprobación de inteligibilidad por token (`hola`/`sistema`, `hello`/`audio`). `stt_audit_noise_rejected` exige que ruido blanco determinista (LCG) + silencio sean rechazados por `NoSpeechGate`.
- **`stt_audit_codeswitching.rs` (5):** code-switching a mitad de frase (auto, pinned ES, pinned EN) y en frontera de segmento. Aserciones flojas: que exista segmento y el segundo sea inglés legible.
- **`stt_audit_terms.rs` (5):** recall de terminología técnica sobre 15 tokens — umbral `>= 8/15` con idioma fijado (líneas 55, 68) y `>= 6/15` en ambos idiomas con autodetección (línea 82). El A/B de `initial_prompt` es informativo.

**Miden:** segmentación/latencia VAD (chunks de 100 ms como en producción), RTF, y **recall de tokens por substring** — no hay WER en ninguna parte. Los campos de calidad (`avg_logprob`, `compression_ratio`, `no_speech_prob`) se imprimen pero casi no se asertan.

**Fixtures:** no están commiteados — se **generan** en `target/stt-fixtures/` llamando a `espeak-ng` y luego `ffmpeg` (16 kHz mono s16le). La primera ejecución exige ambos binarios en `PATH`; `STT_FORCE_REGEN=1` fuerza regeneración.

**Dependencias:** `models/ggml-large-v3-turbo.bin` (existe, 1,6 GB) + `models/ggml-silero-v5.1.2.bin` (existe). Coste estimado 1-2 min en M4 Pro/Metal; en Linux CPU-only con large-v3-turbo, mucho más.

---

### 4. `test-llm` — prácticamente nada

`cargo test --quiet -- --ignored llm` selecciona **exactamente un test**: `test_summarize_real_llm` (src/main.rs:2915). Sembrar 8 turnos en SQLite, comprueba que `needs_consolidation` salta a >75 % de 300 tokens, llama a un LLM **real**, y verifica que el resumen reduce a `keep_turns`, inyecta `[CONVERSATION SUMMARY]` en el system message y persiste.

---

### 5. `audit` y `coverage`

Ambos se auto-saltan con `[SKIP]` si falta la herramienta (`cargo-audit` / `cargo-llvm-cov`) — en esta máquina ninguno está instalado, así que hoy no corren. `audit` no tiene allowlist (`--ignore`) y `Cargo.toml:81` fija `sqlx = "=0.8.6"`, así que un advisory contra esa versión no se puede cerrar sin tocar el pin.

---

### 6. Hallazgos (por impacto)

**A. Alta — `test` y `test-ci` no usan `--workspace`: ejecutan 1 test.** `Cargo.toml` es `[workspace]` (líneas 1-17) **y** `[package]` (línea 21) sin `default-members`, así que `cargo test` a secas solo prueba el paquete raíz `seneschal`. Ese paquete tiene 30 funciones de test y **29 con `#[ignore]`**; la única ejecutable es `default_vad_model_path`. Los **752** `#[test]`/`#[tokio::test]` de `crates/` nunca se corren en `make qa`. El único stage que los ejercita es `coverage`… que se salta si no tienes `cargo-llvm-cov`. `make qa` en verde hoy significa, en la práctica, "el paquete raíz compila y un test pasa".

**B. Alta — `lint` tampoco usa `--workspace.** `cargo fmt --check --all` sí formatea todos los members, pero `cargo clippy --all-targets --no-deps` sin `--workspace` solo hace lint del paquete raíz: los 15 crates pasan sin revisar. (`build` sí los compila, porque son path deps, así que ahí el error de compilación sí salta.)

**C. Alta — `test-stt` puede dar `[PASS]` sin ejecutar una sola aserción.** Dos vías independientes:
1. `skip_if_no_models()` llama `std::process::exit(0)` **a mitad del binario de tests** (stt_common/mod.rs:83). Cargo lo interpreta como éxito y el resto de tests de ese binario se abandona en silencio.
2. Los dos tests STT del paquete raíz usan `VAD_MODEL` por defecto `models/ggml-silero-vad.bin` (src/e2e_tests.rs:936 y :984), un fichero que **no existe** en `models/` (solo está `ggml-silero-v5.1.2.bin`). Ambos hacen `return` temprano y cuentan como pasado.

Agravante: `default_vad_model_path` — el único test que `make test` ejecuta de verdad — **aserta** que la ruta canónica es `models/ggml-silero-vad.bin`, justo la que no existe, mientras `stt_common/mod.rs:63-68` usa `ggml-silero-v5.1.2.bin` con un comentario explicando que la otra no está en disco. Hay una decisión sin tomar aquí, no un simple typo.

**D. Media — 7 tests `#[ignore]` son inalcanzables para cualquier stage:**
- `src/main.rs:3024` `test_kv_cache_after_summarize` — su nombre no contiene `e2e`/`stt`/`llm`.
- `crates/seneschal-core/src/profile.rs:834` `test_extract_facts_real_llm` — sí lleva `llm`, pero `test-llm` corre **sin `--workspace`**, así que un test de `seneschal-core` no puede seleccionarse nunca.
- `crates/seneschal-mcp/src/mcp/transport.rs:525,585,638` — tres wiremock de transporte HTTP JSON-RPC, sin palabra clave en el nombre.
- `crates/seneschal-extras/src/run_agent.rs:1981,2008,2071` — handshake ACP; necesitan un binario `hermes acp` en `PATH` y no tienen ni `#[ignore = "..."]` con el motivo ni guardas de entorno.

**E. Media — `full_pipeline_wav_to_db`** (src/e2e_tests.rs:977) requiere Whisper real pero su ruta no contiene `stt`, así que `test-stt` (filtro `stt`) nunca lo coge; solo lo ve `test-e2e`, donde se auto-salta por el VAD de la letra C. Está en tierra de nadie.

**F. Media — gate de `test-llm` con cortocircuito y puertos contradictorios.** En qa.sh:152, `[[ -z "${LLM_URL:-}" ]] && ! curl …`: si `LLM_URL` está definido, el `curl` **ni se ejecuta** y el stage corre unconditional. Y los tres consumidores usan default `http://localhost:8080` (main.rs:2928, :3030, profile.rs:842) mientras qa.sh sondea `http://127.0.0.1:8000` y `doc/env-vars.md:26` documenta 8000. Sin servidor no hay skip elegante: fallan en un `.await.unwrap()`.

**G. Media — la puerta RTF < 0,5 está calibrada a Metal.** El propio comentario lo dice: "on the M4 Pro". `stt_audit_continuous.rs:194-198` es la única aserción sensible a wall-clock de todo el suite; en un runner Linux CPU-only fallará con seguridad. Además nada pasa `--test-threads=1`, aunque el harness documenta explícitamente que la máquina de estados VAD es stateful y cada test carga su propio modelo (~2-5 s) para evitar interferencia "cuando cargo corre tests en paralelo" (stt_common/mod.rs:102-106).

**H. Baja — `db-inspect` está fuera del workspace.** `crates/seneschal-core/…` son members, pero `db-inspect` no aparece en `Cargo.toml` (0 coincidencias) y no hay `exclude`: sus 21 tests de rutas sobre `tests/fixtures/seneschal.db` no los corre jamás `cargo test --workspace`. En `tests/` de la raíz solo hay WAVs, y 8 de los 10 no los referencia ningún test.

**I. Alta (contexto) — nada de esto está en CI.** En `.github/workflows/ci.yml` están comentados los jobs `check` (fmt+clippy), `test`, `build-linux` e `installer-smoke`; el único vivo es `Build · macOS` (`cargo build --release --bin seneschal --features avspeech`, línea 81). `install-test.yml.disable` está desactivado por nombre de fichero. **No existe `.gitea/workflows/`** pese a que `.runner` registra un act-runner en `tesla.local:3000` y `.opencode/skill/voicebot-ci-loop/SKILL.md` manda repetidamente consultar `.gitea/workflows/ci.yml`. Conclusión: `make qa` en el Mac del mantenedor es hoy el único gate real para todo lo descrito.

---

### 7. Estado de forma resumida

`make qa` comprueba, de verdad: formato en todo el workspace, lint del paquete raíz, compilación completa con `features full`, y 27 escenarios e2e de pipeline/barge-in/ambiente/deep-worker con wiremock y SQLite real — que es trabajo de calidad genuino y bien construido.

No comprueba: los 752 tests unitarios de los crates, el lint de esos crates, nada de STT real (salta por dos caminos distintos que producen verde), ni el resumen con LLM real. El harness está, pero cuatro de sus diez stages son decorativos en la configuración actual.
