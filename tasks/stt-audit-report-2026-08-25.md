# Auditoría y pruebas del sistema STT — Seneschal

**Fecha:** 2026-08-25 · **Máquina:** Mac mini M4 Pro (14 cores, Metal) · **Modelo:** `ggml-large-v3-turbo` (whisper-cpp-plus 0.1.4, feature `metal`) + Silero VAD v5.1.2
**Audiencia:** informe de fallos y recomendaciones. Todos los números son salidas reales de ejecución (logs en `/tmp/stt-*.log`, reproducible con `make test-stt`).

---

## 1. Resumen ejecutivo (máx. 10 líneas)

1. El síntoma "no arranca hasta que termino de hablar" es **diseño, no bug puntual**: la transcripción es 100% por lotes al cierre del segmento (VAD) y el provider **nunca emite parciales** (`SpeechEvent::Speech` no se usa en whisper). Latencia medida tras el fin real del habla: **~0.9–1.9 s** (cola VAD 300–600 ms + inferencia 450–950 ms, +~400 ms con autodetección de idioma).
2. El VAD funciona y cierra bien: `SpeechStart` a los **0.4 s** (2 probes de 200 ms) y el cap duro de 20 s corta el habla continua (27 s ES → 2 segmentos, sin pérdida de texto).
3. En habla continua en **inglés** el VAD cortó **a mitad de frase** (12.8 s de 26.3 s) en una pausa de coma: `vad_end_threshold=0.45` + `VAD_SILENCE_MS=300` son frágiles para prosodia EN.
4. Nombres de modelos: **"Qwen 3.8 27B" sale siempre mal** ("QEN 3.827B" / "QEM 3.827B" / "QN" / "WEN" en 4 configuraciones); recall de 15 tokens técnicos: 9–12/15 según idioma/pinning.
5. La prueba clave: un `initial_prompt` con dominio LLM **corrige la grafía "Qwen3.8-27B"** (10/15 → 11/15, +50 ms de coste) y la API de whisper-cpp-plus ya lo expone; el código product no lo usa.
6. Code-switching (issue #217): con `STT_LANGUAGE=auto` los segmentos con cambio de voz se segmentan y transcriben **correctamente en ambos idiomas** (p=0.992/0.997); el coste es +~400 ms de inferencia en el segmento detectado.
7. **Bug crítico de QA encontrado:** `make test-stt` (y `test-ci`, `test-e2e`, `test-llm`, `audit`, `coverage`) **no ejecutaban nada** y terminaban en "QA PASSED": `run_stage()` de `scripts/qa.sh` llamaba `stage_test-stt` (guiones → función inexistente). Corregido en esta auditoría.
8. **Bug crítico de config:** el default embebido `vad_model = "models/ggml-silero-vad.bin"` **no existe en disco** (el archivo es `ggml-silero-v5.1.2.bin`); solo funciona porque `.env` lo sobreescribe. Sin `.env`, el provider STT falla al arrancar.
9. Config muerta: `WHISPER_THREADS` (14 en .env) **nunca se aplica** (whisper-cpp-plus usa `num_cpus/2` por defecto); `STT_EARLY_REUSE_*` no la lee ningún código; la regla 1 de `NoSpeechGate` es inoperante (`no_speech_prob` es placeholder 0.0).
10. Veredicto: el STT "funciona" (RTF 0.03–0.11 en Metal, muy por debajo de 1.0) pero la **experiencia percibida** (sin parciales, cola VAD, auto-detect) y la **precision en jerga técnica** son los dos frentes a atacar; hay correcciones concretas baratas (P0/P1) documentadas abajo.

---

## 2. Alcance y método

### 2.1 Código leído (antes de diseñar pruebas)

| Archivo | Hallazgo clave |
|---|---|
| `crates/seneschal-core/src/stt/whisper.rs` | `WhisperSttProvider`: VAD Silero por probes de 200 ms (`VAD_PROBE_MS`), 2 probes consecutivos para confirmar (`vad_confirm_probes=2` → 400 ms), pre-roll 300 ms + pre-silence 200 ms, cap duro `MAX_SEGMENT_MS=20_000`, cierre por `silence_samples >= VAD_SILENCE_MS`. La inferencia es **bloqueante** (`spawn_blocking` → `full()` de whisper) y se dispara **solo al cierre del segmento**. `resolve_whisper_language()`: "auto"→`""` (autodetección, issue #217). |
| `crates/seneschal-core/src/stt/no_speech_gate.rs` | Regla 1 (no_speech) + regla 2 (compression_ratio > 2.4). |
| `crates/seneschal-core/src/stt/provider.rs` | `create_provider()`: el whisper usa `config.stt_language` (default `"auto"`), no `config.language`. |
| `src/main.rs` (loop de audio ~línea 2217) | Chunks de 100 ms → `process_audio`; consume `SpeechStart` (barge-in) / `Speech(parcial)` / `SpeechEnd`. El handler de parciales **existe** pero el provider whisper nunca lo emite. |
| `crates/seneschal-common/src/config.rs` + `seneschal.pro.toml` + `.env` | Defaults y overrides (detalle en §5). |
| whisper-cpp-plus 0.1.4 (registry) | `FullParams::initial_prompt()` y `n_threads()` existen y están sin usar por el product. `WhisperStream` (stream-pcm con modo VAD) existe como alternativa de streaming real. |

### 2.2 Fuentes de audio (documentado)

`say` se descartó para la jerga: pronuncia "Qwen 3.8 27B" de forma irreconocible y no controla pausas. Se usó **espeak-ng + ffmpeg** (16 kHz mono s16le) a velocidad reducida (150–175 wpm) para que Whisper transcriba bien las frases de control; los fixtures se generan bajo `target/stt-fixtures/` (re-producibles; `STT_FORCE_REGEN=1` obliga a regenerar). Limitación conocida: la pronunciación espeak de "Qwen" (fonemas es) no es humana; los errores medidos son una **cota de referencia**, y los resultados de `initial_prompt` (§3.3) son los más concluyentes porque comparan el mismo audio con/sin prompt.

Fixtures generados (duración real medida):

| Fixture | Duración | Uso |
|---|---|---|
| `cont_es.wav` | 26.99 s | Habla continua ES sin pausas >15 s (supera el cap de 20 s) |
| `cont_en.wav` | 26.28 s | Habla continua EN sin pausas |
| `codeswitch_mix.wav` | 8.73 s | Code-switching **a mitad de frase** (voz ES leyendo "the model performs really well") |
| `codeswitch_boundary.wav` | 8.18 s | 2 frases (ES + EN) con cambio de voz y pausa de 400 ms |
| `terms_es.wav` / `terms_en.wav` | 13.63 s / 12.54 s | 15 términos técnicos (Qwen 3.8 27B, Whisper Large V3 Turbo, Silero VAD, SGLang, vLLM, RTX PRO 6000, DFlash2) |
| `short_es.wav` / `short_en.wav` | 3.10 s / 2.91 s | Baseline de latencia/RTF |
| `qwen_repeated.wav` | 15.35 s | 5 repeticiones del nombre de modelo |

### 2.3 Harness de tests (nuevo, no commiteado)

- `crates/seneschal-core/tests/stt_common/mod.rs` — WAV I/O, detección de onset/offset por RMS, `run_stream()` (alimenta `process_audio` en chunks de 100 ms idénticos a producción y mide `SpeechStart`/`SpeechEnd` en tiempo de stream + wall-clock de la llamada que contiene la inferencia), `transcribe_complete` cronometrado, helpers de normalización/token-presence.
- `crates/seneschal-core/tests/stt_audit_continuous.rs` — 4 tests (continua ES/EN, baseline latencia/RTF, ruido).
- `crates/seneschal-core/tests/stt_audit_codeswitching.rs` — 5 tests (mix auto/es/en, boundary auto/es).
- `crates/seneschal-core/tests/stt_audit_terms.rs` — 5 tests (términos es/en/auto, `initial_prompt`, "Qwen" repetido).

Todos con `#[ignore]` y "stt" en el nombre → entran en `cargo test -- --ignored stt`. **Ejecución final verificada:** `make test-stt` → **14 passed, 0 failed** (`CARGO_EXIT=0`), + el test e2e legacy `stt_transcribes_wav_file` del root (ver §5.3).

---

## 3. Resultados por escenario (esperado vs obtenido)

### 3.1 Escenario 1 — Habla continua sin pausas (>15 s, cap de 20 s)

**Esperado:** VAD confirma a ~0.4 s; el cap de 20 s corta el stream; segmentos sin huecos; transcript no vacío; RTF < 1.0.

| Métrica | ES (26.99 s) | EN (26.28 s) |
|---|---|---|
| Onset de audio (RMS) | 0.000 s | 0.000 s |
| `SpeechStart` (commit VAD) | **0.400 s** | **0.400 s** |
| Segmentos | 2 (cap + cola) | 2 (**corte VAD a mitad de frase**) |
| seg0 | 0.400 → 19.800 s (19.40 s, **cap**) · infer **633 ms** · RTF **0.033** | 0.400 → 12.800 s (12.40 s, **VAD**) · infer **555 ms** · RTF **0.045** |
| seg1 | 20.600 → 27.089 s (6.49 s) · infer **486 ms** · RTF **0.075** | 14.200 → 26.483 s (12.28 s) · infer **516 ms** · RTF **0.042** |
| Transcripción | Completa y fiel (solo "detección del aula" en vez de "del habla") | Completa y fiel |
| Cola VAD + inferencia tras fin del habla | fin 26.55 s → `SpeechEnd`+transcript a 27.09 s ⇒ **~0.54 s** (segmento final) | cierre seg0 a 12.80 s con audio que seguía hasta 26.1 s |

Transcripts reales (seg0, truncados):
- ES: `"Estoy auditando el sistema de reconocimiento de voz porque la latencia resulta alta cuando hablo en continuo y además las transcripciones de los nombres de modelos de inteligencia artificial salen mal, por lo que voy a medir el tiempo de detección del aula, la precisión del modelo en español e inglés y el rendimiento real de la inferencia en esta máquina."`
- EN: `"I am auditing the speech recognition system because the latency is too high when I speak continuously and also the transcriptions of machine learning model names keep coming out wrong, so I will measure the voice activity detection timing,"` ← **corte en el coma**

**Conclusión:** el cap de 20 s funciona y no hay pérdida de audio; el commit del VAD (0.4 s) es correcto. **Pero en EN el VAD cerró el segmento en una pausa de coma (12.8 s de 26.3 s)**: con habla humana, una pausa de 300 ms en un coma cortaría el turno a mitad de frase → el pipeline lanzaría el LLM antes de que el usuario termine (barge-in auto, respuesta a media oración). En ES no ocurrió (espeak no marcó la pausa en ese rango) — es frágil, no determinista.

### 3.2 Escenario 1b — Baseline de latencia / RTF (transcripción one-shot, mismo path de product)

| Run | Duración audio | Tiempo | RTF |
|---|---|---|---|
| ES warm-up (1ª inferencia, Metal cold) | 3.10 s | **768 ms** | 0.248 |
| ES steady #2 / #3 | 3.10 s | 454 ms / 454 ms | **0.147** |
| EN warm-up / steady | 2.91 s | 447 ms / 449 ms | **0.154** |
| Segmento 8.4 s con auto-detect (mix) | 8.43 s | **951 ms** | 0.113 |
| Segmento 8.4 s con idioma pinado | 8.43 s | 522–543 ms | 0.062–0.064 |

**Modelo de latencia percibido (problema 1), medido:**
- El usuario termina de hablar → VAD necesita `VAD_SILENCE_MS=300` ms (cuantizado a probes de 200 ms ⇒ **400–600 ms reales**) → inferencia **bloqueante** del segmento entero (**450–950 ms**, +~400 ms si es el primer segmento con `auto`) → `SpeechEnd` con el transcript → LLM.
- **Total medido: ~0.9–1.9 s después de dejar de hablar, sin ningún texto intermedio** (el provider whisper nunca emite `SpeechEvent::Speech`).
- La cola del VAD también añade 400 ms *antes* de que el asistente entre en `LISTENING`/barge-in (`SpeechStart`).
- RTF muy bueno (0.03–0.11): **la inferencia NO es el cuello de botella; la falta de streaming de resultados lo es.**

### 3.3 Escenario 2 — Code-switching ES↔EN (issue #217)

Fixture `codeswitch_boundary` (voz ES: "El modelo Qwen 3.8 funciona muy bien en mi opinión" + 400 ms + voz EN: "and it runs fast on this machine so the latency is acceptable"):

| `STT_LANGUAGE` | Segments | Transcript seg0 (ES) | Transcript seg1 (EN) | Inferencia seg0/seg1 |
|---|---|---|---|---|
| **auto** | 2 ✓ | `"El modelo QEM 3.8 funciona muy bien en mi opinión."` | `"And it runs fast on this machine so the latency is acceptable."` ✓ | 902 ms (RTF 0.237) / 875 ms (RTF 0.284) |
| **es** (pin) | 2 | `"El modelo QEM 3.8 funciona muy bien en mi opinión."` | `"Y funciona rápido en esta máquina, así que la latencia es aceptable."` ✗ (EN forzado a ES) | 488 ms / 463 ms |

- Autodetección real (log de whisper-cpp): `auto-detected language: es (p = 0.992078)` y `en (p = 0.997233)` — **el fix de #217 resuelve el caso de cambio entre segmentos** (2 segmentos limpios, ambos idiomas correctos con `auto`).
- Coste medido de `auto`: **+~400 ms por segmento detectado** (902 vs 488 ms en el mismo audio) — relevante si el usuario alterna idiomas a menudo.
- **Con `es` pinado, el lado EN no se "pierde": se re-codifica a español** (comportamiento estándar de whisper con idioma forzado). En una sesión mixta con pin, la mitad EN de la conversación saldría en español o ininteligible.
- Fixture `codeswitch_mix` (voz ES leyendo inglés a mitad de frase — "spanglish" realista):
  - `auto`/`es`: `"Hoy voy a probar el QEN 3.827B porque temo del performance rea y wey en mi opinión y el SGDAM lo sirve sin problemas."` — el modelo se queda en ES y "españoliza" el inglés ("temo del performance" = "the model performs").
  - `en`: `"Today I'm going to try the QEM 3.827B because the model performs real way in my opinion and the SGDAM lo serves without problems."` — mejora el lado EN, degrada el ES.
  - Veredicto: con pronunciación **acento-español** de palabras EN, ningún pin resuelve bien ambos lados; es la naturaleza del decoder por segmento (cada segmento = 1 idioma). Con habla humana (pronunciación EN nativa) el caso boundary del §3.3 demuestra que `auto` sí funciona.

### 3.4 Escenario 3 — Términos técnicos y nombres de modelos

15 tokens esperados (`qwen, 3.8, 27, whisper, large, v3, turbo, silero, vad, sglang, vllm, rtx, pro, 6000, dflash2`), match insensible a whitespace/mayúsculas.

| Run | Transcript (frecuencias) | Recall |
|---|---|---|
| ES-voz, pin `es` | `"Estoy usando el modelo QEN 3.827B, también Whisper LARGE V3 Turbo, CILERO VAM, SGLAN, VLLM, la tarjeta RTX PRO6000i de FlashMOS."` | **10/15** |
| ES-voz, pin `en` | `"I'm using the WEN 3.827B, also Whisper LARG V3 Turbo, Cileo VAM, SGLAN, VLLM, the RTX PRO6000 flash MOS."` | 9/15 |
| ES-voz, `auto` | igual al pin `es` | 10/15 |
| EN-voz, pin `es` | `"I am using the model QN 3.827B, also Whisper Large V3 Turbo, Cilero VAD, SGLANG, VLLM, the RTX PR06000 card, and D-Flash 2."` | 12/15 |
| EN-voz, pin `en` / `auto` | `"I am using the model QN 3.827B, ... RTX PR06000 card, and D-Flash 2."` | **12/15** |
| RTF (13.6 s de audio) | pin: 535–549 ms (0.039–0.043) · auto: 966–977 ms (0.072–0.077) | — |

**Fallo dominante y sistemático (el problema 2 del usuario):**

| Término | Resultado típico | Patrón |
|---|---|---|
| **Qwen 3.8 27B** | **"QEN 3.827B" / "QEM 3.827B" / "QN" / "WEN"** (0/4 configuraciones lo adivinan) | prefijo "Qw-" no existe en ES/EN → fonética "quen/kwen"→QEN/QEM; además **"27B" se fusiona con "3.8" → "3.827B"** (pérdida del espacio y del concepto de parámetros) |
| Silero VAD | "CILERO VAM" (es) / "Cilero VAD" (en) | C↔S inicial + "VAD"→"VAM" en ES |
| SGLang | "SGLAN" / "SGLaung" | pérdida de "g" final |
| DFlash2 | "FlashMOS" (es) / "D-Flash 2" (en) | el prefijo "D" desaparece en ES |
| RTX PRO 6000 | "RTX PRO6000i" / "PR06000" | fusión y "P"→"PR", "O"→"0" |
| Whisper Large V3 Turbo | bien en la mayoría (LARGE/LARG en es) | — |

Repetido (5 veces, 15.3 s): `"QEM 3.827B. El modelo QEM 3.827B."` — **la confusión es determinista, no aleatoria**: es el prior del modelo sobre un token desconocido, no ruido.

**Prueba de `initial_prompt` (la recomendación que más funciona):** mismo fixture ES, `FullParams::initial_prompt("Transcripción de voz sobre tecnología: nombres de modelos de IA como Qwen, Qwen3.8, Qwen3.8-27B, Whisper, Silero, SGLang, vLLM, DFlash2. GPUs: RTX PRO 6000.")`:
- Sin prompt (549 ms): `"Estoy usando el modelo QEN 3.827B, también Whisper LARGE V3 Turbo, CILERO VAM, SGLAN, VLLM, la tarjeta RTX PRO6000i de FlashMOS."` → 10/15
- Con prompt (598 ms, +50 ms): `"Estoy usando el modelo Qwen3.8-27B, también Whisper, LARG, V3 Turbo, Silero, VAM, SGLaung, vLLM, la tarjeta RTX PRO 6000."` → **11/15, y "Qwen" correctamente graficado por primera vez** (silero también mejora).
- Conclusión: el prompt de dominio corrige el fallo exacto reportado ("Qwen 3.8 27B" / "Qwen3.8-27B") a coste mínimo. **El código product no usa `initial_prompt` en absoluto** (`transcribe()` en `stt/whisper.rs` no lo configura).

### 3.5 Extras: ruido y NoSpeechGate

3 s de ruido blanco + 1.5 s de silencio → **ningún segmento** (el VAD no confirmó: bien). Ojo: como el VAD ya filtra, la **regla 1 de `NoSpeechGate` es inoperable por construcción** — `no_speech_prob` es placeholder `0.0` (whisper-cpp-plus no expone `state.ptr`), así que la única regla viva es la de `compression_ratio > 2.4`.

---

## 4. Fallos detectados (por severidad)

### CRÍTICO
- **C1 — `make test-stt` y otros 5 QA stages no ejecutaban nada.** `run_stage()` en `scripts/qa.sh` hacía `stage_$1` con `$1=test-stt` → `stage_test-stt: command not found` → `|| true` → "QA PASSED" (0 stages). Afecta a `test-ci`, `test-e2e`, `test-stt`, `test-llm`, `audit`, `coverage`. **Corregido en esta auditoría** (normaliza guiones→underscore; ver §6).
- **C2 — El default de VAD no existe en disco.** `seneschal.pro.toml` (embebido en el binario) dice `vad_model = "models/ggml-silero-vad.bin"`; el archivo real es `models/ggml-silero-v5.1.2.bin`. Sin `.env` (instancia nueva, CI, `seneschal.dev.toml` limpio) `WhisperSttProvider::new` falla → **el STT no arranca**. Solo sobrevive gracias a `VAD_MODEL=...v5.1.2.bin` en `.env`.

### MAYOR
- **M1 — Problema 1 (latencia percibida): batch-only sin parciales.** El transcript solo existe al cierre del segmento; medido 0.9–1.9 s tras el fin del habla. `SpeechEvent::Speech` existe y `main.rs` ya la consume, pero el provider whisper no la emite jamás. Los configs `STT_EARLY_REUSE_ENABLED/MIN_TOKENS/REQUIRE_PUNCTUATION` (presentes en `.env` y `seneschal.pro.toml`) **no los lee ningún código** (feature fantasma).
- **M2 — Problema 2 (nombres de modelos): prior del modelo sin contexto de dominio.** "Qwen 3.8 27B" → "QEN 3.827B" de forma sistemática; recall 9–12/15 en jerga. Corregible barata con `initial_prompt` (demostrado: 10/15→11/15 y grafía correcta de "Qwen").
- **M3 — VAD corta a mitad de frase en habla continua EN** (12.8 s de 26.3 s, en una pausa de coma). `vad_end_threshold=0.45` + 300 ms de cola son frágiles con prosodia EN (pausas de coma 150–300 ms). Con habla humana → respuesta del LLM a media oración.
- **M4 — `auto` añade +~400 ms por segmento** (detecta idioma sobre los primeros 30 s de cada segmento). Aceptable para boundary (resuelve #217) pero caro si el usuario alterna idiomas cada frase; no hay opción de "detectar solo el primer segmento y fijar el resto".

### MENOR
- **m1 — `WHISPER_THREADS` es config muerta** (`.env`=14; nunca se aplica: `FullParams` usa `num_cpus/2`=7 en esta máquina).
- **m2 — `NoSpeechGate` regla 1 inoperable** (`no_speech_prob` siempre 0.0 → la condición nunca se cumple).
- **m3 — Tests e2e legacy del root se saltan en silencio bajo `make test-stt`** (`stt_transcribes_wav_file`, `full_pipeline_wav_to_db`): usan el default antiguo `ggml-silero-vad.bin` (no existe), printean "SKIP" y devuelven `Ok` → cuentan como "passed". No validan nada.
- **m4 — `doc/env-vars.md` documenta `VAD_MODEL` con el default viejo** (`ggml-silero-vad.bin`); coherente con el código pero engañosa vs el archivo real.
- **m5 — El warm-up (Metal cold) cuesta ~770 ms** en la primera inferencia (RTF 0.25 vs 0.15 steady). Si el usuario habla nada más arrancar el proceso, ese segmento paga el warm-up.

---

## 5. Recomendaciones priorizadas

### P0 (hacer ahora)
1. **Mantener el fix de `scripts/qa.sh`** (ya aplicado, no commiteado): `run_stage` traduce `test-stt`→`stage_test_stt` y `stage_test_stt` ahora corre `cargo test --workspace -- --ignored stt` (antes solo el root: los tests de `seneschal-core` se saltaban). Verificar con `make test-stt` (14 passed).
2. **Corregir el default de VAD** (C2): en `seneschal.pro.toml` cambiar `vad_model = "models/ggml-silero-vad.bin"` → `"models/ggml-silero-v5.1.2.bin"` (o renombrar el archivo y dejar el old name como symlink). 1 línea; sin esto no hay STT sin `.env`.
3. **Reducir la cola VAD** (M1, quick win): `VAD_SILENCE_MS=200` (ya es el valor default de `WhisperSTTVADConfig::default()`; `.env` lo pone a 300). Ahorra 100–200 ms del cierre con riesgo bajo; validar con los fixtures de §3.1 (el corte de coma de M3 podría empeorar → combinar con P1-2).

### P1 (esta semana)
4. **Añadir `initial_prompt` de dominio LLM** (M2): nuevo `STT_INITIAL_PROMPT` (config + env) aplicado en `transcribe()` vía `FullParams::initial_prompt()` (la API existe). Valor recomendado (probado): `"Transcripción de voz sobre tecnología. Nombres de modelos de IA: Qwen, Qwen3.8, Qwen3.8-27B, Whisper, Silero, SGLang, vLLM, DFlash2. GPUs: RTX PRO 6000."` Coste medido: +50 ms (~+9%); beneficio: corrige la grafía de "Qwen" y silero.
5. **Endurecer el umbral de cierre del VAD** (M3): bajar `vad_end_threshold` a 0.30 (o añadir `min_segment_ms` ~4 s antes de permitir cierre en habla continua). Re-testear con `cont_en.wav` (debe dejar de cortar a los 12.8 s) y con `short_*.wav` (que siga cerrando rápido en frases reales).
6. **Parciales reales** (M1, el fix definitivo del "no arranca hasta que termino"): dos opciones, de menor a mayor esfuerzo:
   - **(a) Rolling re-transcribe** en `WhisperSttProvider`: cada ~1.5 s en `in_speech`, re-decodificar las últimas N segundos (p. ej. 10 s) y emitir `SpeechEvent::Speech(parcial)` — `main.rs` ya lo consume (debug + TUI). Coste: +1 inferencia de 10 s cada 1.5 s (RTF 0.05 ⇒ ~500 ms de CPU/GPU, asumible en M4 con `metal`); el LLM aún espera al `SpeechEnd` (sin cambiar el contrato).
   - **(b) Migrar a `WhisperStream`** de whisper-cpp-plus (stream-pcm, modo VAD): transcripción incremental real; permite después que el LLM arranque sobre parciales estables y dar vida a `STT_EARLY_REUSE_*`. Más riesgo (cambio de provider interno); prototipar como spike.

### P2 (backlog)
7. **`WHISPER_THREADS`**: o aplicarlo (`FullParams::n_threads(threads)` en `transcribe()`) o borrarlo de config/docs (m1).
8. **`STT_EARLY_REUSE_*`**: implementar (junto con P1-6b) o eliminar de `.env`/`seneschal.pro.toml`/`Config` (config muerta).
9. **`NoSpeechGate`**: pedir a whisper-cpp-plus que exponga `no_speech_prob` (o fork con getter) y activar la regla 1; mientras tanto documentar que el gate solo protege contra gibberish por compresión (m2).
10. **Tests e2e legacy del root**: que lean `VAD_MODEL` con fallback a `ggml-silero-v5.1.2.bin` y que un skip de modelo **falle** en CI en vez de "pass" (m3).
11. **Warm-up del modelo al arrancar** (m5): inferencia de 0.5 s de silencio en el init del provider para que la primera frase del usuario no pague el Metal cold (770→450 ms).
12. **Comparativa Parakeet** (opcional): `models/parakeet-tdt-0.6b-v3-onnx` está en disco (`STT_PROVIDER=parakeet` existe). Parakeet TDT 0.6B v3 es más rápido (RTF ~0.01–0.03) pero **solo inglés**: no cubre ES ni code-switching → útil como fallback EN de baja latencia, no como fix de P1-4/P1-6. Medir antes de decidir.
13. **Documentar** en `doc/env-vars.md`: VAD real (`v5.1.2`), `WHISPER_THREADS` (muerta), `STT_INITIAL_PROMPT` (nueva).

---

## 6. Cambios aplicados en esta auditoría (no commiteados)

| Archivo | Cambio | Motivo |
|---|---|---|
| `crates/seneschal-core/tests/stt_common/mod.rs` (nuevo) | Harness compartido (fixtures, WAV, `run_stream`, timing, tokens) | Re-ejecutable con `make test-stt` |
| `crates/seneschal-core/tests/stt_audit_continuous.rs` (nuevo) | 4 tests: continua ES/EN, baseline, ruido | Escenario 1 |
| `crates/seneschal-core/tests/stt_audit_codeswitching.rs` (nuevo) | 5 tests: mix auto/es/en, boundary auto/es | Escenario 2 / issue #217 |
| `crates/seneschal-core/tests/stt_audit_terms.rs` (nuevo) | 5 tests: términos es/en/auto, `initial_prompt`, Qwen repetido | Escenario 3 |
| `scripts/qa.sh` (modificado, 2 hunks) | (1) `run_stage`: `tr '-' '_'` al llamar `stage_*` — **fix C1** (antes "QA PASSED" sin ejecutar nada); (2) `stage_test_stt`: `cargo test --workspace` — **fix del silent-skip de los crates** | Infraestructura QA; necesaria para que `make test-stt` ejecute el harness |

**No se modificó código product** (`src/`, `crates/*/src/`) ni `.env`. Los fixtures viven en `target/stt-fixtures/` (ignorado por git).

## 7. Reproducibilidad

```bash
cd ~/projects/ai/voicebot
make test-stt                       # 14 tests, ~1–2 min en M4 (Metal)
# o individualmente:
cargo test -p seneschal-core --test stt_audit_continuous -- --ignored stt --nocapture --test-threads=1
STT_FORCE_REGEN=1 make test-stt     # regenera los fixtures de audio
```

Logs de esta ejecución: `/tmp/stt-baseline.log`, `/tmp/stt-continuous.log`, `/tmp/stt-codeswitch.log`, `/tmp/stt-terms.log`, `/tmp/stt-final-full.log`.
