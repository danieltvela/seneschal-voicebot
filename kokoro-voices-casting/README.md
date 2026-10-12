# Kokoro Voices Casting

Casting de las **54 voces de Kokoro 82M v1.0** (repo oficial `hexgrad/Kokoro-82M`).
Entra aquí, lista las voces, se descargan solas, y las oyes una a una para
encontrar la que más te guste. Nada de este directorio toca el proyecto
Seneschal del padre.

## Qué hay aquí

| Fichero | Para qué |
|---|---|
| `setup.sh` | Instalación completa (uv + Python 3.12 + deps + modelo + 54 voces) |
| `install_uv.sh` | Solo instala `uv` local en `bin/` (no toca nada fuera) |
| `casting.sh` | El frontend: `./casting.sh list`, `say`, `battle`, `best`, `all` |
| `cast.py` | Síntesis + reproducción (cachea cada WAV en `out/`) |
| `download_voices.py` | Descarga modelo/voces desde Hugging Face a `model/` y `voices/` |
| `catalog.py` | Las 54 voces con lengua, género, grado oficial y frase de muestra |
| `pyproject.toml` | Dependencias (torch **CPU**: no pisa la GPU compartida de tesla) |

## Instalación (una vez)

```bash
cd ~/projects/ai/voicebot/kokoro-voices-casting
./setup.sh
```

Baja: uv local (`bin/uv`), CPython 3.12 gestionado por uv, torch CPU
(~200 MB), modelo `kokoro-v1_0.pth` (~327 MB) y las 54 voces `.pt` (~27 MB).
Total ~400 MB, todo dentro de este directorio (más la caché de uv).

Variantes: `./setup.sh --no-model` (solo entorno) o
`./setup.sh --langs e a` (solo español + inglés US).

Requisito del sistema: `aplay` (viene con Debian) y la librería espeak-ng
que trae el propio paquete pip `espeakng-loader` — nada que instalar con apt.

## Uso diario

```bash
./casting.sh list                  # catálogo: lengua, género, grado, ¿descargada?
./casting.sh list --lang e         # solo español
./casting.sh list --missing        # qué falta y comando para bajarlo

./casting.sh say ef_dora           # oír Dora (español) con su frase de muestra
./casting.sh say em_alex -t "Buenas noches. ¿En qué puedo ayudarte?"
./casting.sh say af_heart,af_bella # dos voces, mismo texto, por turnos

./casting.sh best                  # las 16 mejor puntuadas, una tras otra (~5 min)
./casting.sh battle ef_dora em_alex em_santa   # duelo directo entre las 3 españolas
./casting.sh all --yes             # las 49 (sin ja/zh), maratón de ~10 min

./casting.sh mix em_alex:2,em_santa:1 --name castizo -t "Buenas noches."  # voz nueva propia
```

## Voces propias y español de España (investigado sep 2026)

**No existe voz masculina con acento español peninsular para Kokoro**, ni oficial
ni comunitaria: el español oficial son `ef_dora`/`em_alex`/`em_santa` (VOICES.md)
y la búsqueda en Hugging Face (full-text `es_M_`) no da ningún pack `.pt` en
castellano — los packs comunitarios que hay son alemán, sueco e inglés.

El acento sudamericano **no viene del G2P**: misaki fonemiza con espeak-ng `es`
(castellano de verdad: saca `/θ/` y `/β/` — "cinco" → `θˈinko`). Vive en el
tensor de estilo de la voz: el audio con que se entrenó ese hablante era latino.

Tres formas de conseguir voz propia, de barata a definitiva:

1. **Mezclar voces oficiales** (lo que hace `mix`, sin entrenar nada): los
   voicepacks son tensores de estilo `[510,1,256]` e interpolan linealmente.
   Cambian timbre y registro; el acento latino se diluye, no desaparece.
   El `.pt` resultante vale para cualquier backend Kokoro, incluido Seneschal.
   Las voces propias quedan en `voices/custom.json` y se usan igual que las
   oficiales: `./casting.sh say mayordomo`, `battle`, etc.
2. **Optimizar un voicepack contra grabaciones** (GPU, minutos): congelar el
   modelo y ajustar el tensor `[510,1,256]` hasta que suene como el hablante de
   referencia. No hay herramienta oficial: el extractor de estilo de Kokoro no
   se publicó; solo existen experimentos comunitarios.
3. **Fine-tune con voz propia** (GPU + 30-60 min de audio limpio, mínimo 2-3 h):
   la receta demostrada es [kikiri-tts](https://github.com/semidark/kikiri-tts)
   (dos etapas StyleTTS2); `Joakim/kokoro-sv-voices` creó así 10 voces suecas
   nombradas a partir de 8,6 h de audio en una GPU. Para castellano: grabarte
   a ti mismo en sala tranquila, o corpora con licencia (Mozilla Common Voice
   `es-ES` es CC0; CVC/IULML son no comerciales). Salida: un `.pth` + `.pt`
   cien por cien compatibles con este casting y con el voicebot.

Detalles:

- Cada voz se oye **en su idioma** con una frase de muestra diseñada para
  juzgar ritmo y timbre (los grados del catálogo vienen de VOICES.md oficial).
- El audio se cachea en `out/<voz>.wav`: repetir una voz es instantáneo.
  Borra el fichero (o su `.txt` gemelo) para regenerarla con otro texto.
- **Ctrl-C** corta la reproducción en cualquier momento.
- Audio: sale por el Poly Sync (`P20M`) por defecto. Cambia con
  `--alsa default:CARD=Audio` (USB) o `KOKORO_ALSA_DEVICE=...`.
  Lista salidas: `aplay -L | grep -E '^(default|sysdefault)'`.
- Si no oyes nada pero el comando termina bien: la tarjeta estaba **muteada
  por mezclador** (ALSA no lo reporta; `aplay` devuelve 0 igual). El script
  lo detecta y desmudea solo antes de cada reproducción. Inspección manual:
  `amixer -c Audio sget 'PCM,0'` → debe decir `[on]`.
- Velocidad: `--speed 0.9` mitiga el "rushing" en frases largas.
- El modelo se carga ~10 s la primera vez de cada sesión de casting, luego
  cada voz tarda ~1x realtime en CPU. Con `--device cuda` usaría la GPU
  (no hace falta aquí, y la caja es compartida).

## Japanese y chino

No incluidas por defecto: piden extras pesados de G2P (`fugashi`,
`pyopenjtalk`, `pypinyin`…). Si un día las quieres:

```bash
uv pip install 'misaki[ja,zh]'
./casting.sh say jf_alpha
```

## Estructura de la voz

Una vez elijas, la selección vive en una sola línea: el `.pt` elegido
(`voices/af_heart.pt`, 500 KB) + `kokoro-v1_0.pth` es exactamente lo que
cualquier backend Kokoro (incluido el `KModel` de Python y su homónimo
Rust/ONNX del voicebot) necesita para hablar con esa voz.

## Grados (VOICES.md oficial, resumen)

- **A**: `af_heart` — la mejor del proyecto entero
- **A-/B-**: `af_bella`, `af_nicole`, `bf_emma`, `ff_siwis`
- **C+/C**: la mayoría (buenas, menos datos)
- **D y por debajo**: voces con minutos de entrenamiento — óyelas igual,
  a veces el oído elige lo que la nota no anticipa.
- Español (`ef_dora`, `em_alex`, `em_santa`): sin nota oficial; G2P por
  espeak-ng. Julgado aquí, no en el papel.
