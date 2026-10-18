# Seneschal model services

Docker Compose definitions for the local LLM / STT / TTS servers used by
Seneschal. These services were moved here from the previous host-level
`docker-compose.yml` so the voice assistant stack can be versioned and
operated alongside the application.

The compose file contains **no host paths, tokens or credentials**. Every
machine-specific value is read from environment variables in
[`docker/.env`](#configuration), which is git-ignored.

The file defines **two vendor profiles**:

| Profile  | Model family | LLM | STT | TTS |
|----------|--------------|-----|-----|-----|
| `qwen`   | Qwen         | Qwen3.8-Flash-Next FP4 (TP=1) | Qwen3-ASR 1.7B (`qwen-asr`) | Qwen3-TTS 1.7B Base (`qwen-tts`) |
| `nvidia` | NVIDIA       | Nemotron 3.5 Super VL 120B-A12B NVFP4 (TP=2 or TP=1) | Nemotron 3.5 ASR Streaming 0.6B (`nemotron-asr`) | Magpie TTS Multilingual 357M (`magpie-tts`) |

Start one profile at a time. Both profiles publish an LLM on host port `8005`
and a TTS on host port `8025`, so they cannot run simultaneously.

> The qwen LLM is an NVIDIA FP4 quantization of a Qwen model. It is grouped
> under `qwen` because the model family is Qwen. NVIDIA-quantized checkpoints
> of other families (GLM, Gemma) and the image-generation services from the
> source file are intentionally **not** included.

## Layout

```
docker/
├── docker-compose.yml          # services + qwen/nvidia profiles
├── .env.example                # configuration template (safe to commit)
├── .gitignore                  # ignores .env
├── README.md
├── magpie-tts/
│   ├── build-image.sh          # builds the NeMo-Speech.cpp image (TTS + ASR)
│   ├── download-weights.sh     # downloads Magpie TTS weights
│   └── tts.yaml                # Magpie TTS runtime config
├── nemotron-asr/
│   ├── download-weights.sh     # downloads Nemotron 3.5 ASR weights
│   └── asr.yaml                # ASR-only runtime config
└── vllm-omni/
    └── qwen3_tts.base.yaml     # Qwen3-TTS vLLM-Omni deploy config
```

## Configuration

1. Create the local environment file from the template:

   ```bash
   cd docker
   cp .env.example .env
   chmod 600 .env
   ```

2. Edit `.env` and replace the example `/srv/seneschal/...` values with the
   real directories on your host:

   ```bash
   $EDITOR .env
   ```

3. Validate that Compose can resolve every variable:

   ```bash
   docker compose --profile qwen config -q
   docker compose --profile nvidia config -q
   ```

   If a required variable is missing, Compose stops with an error such as
   `required variable QWEN38_MODEL_DIR is missing a value`. Compose
   interpolates the whole file, so every path variable must be set even if
   you only plan to start one service.

4. Keep `.env` private. It is already listed in `docker/.gitignore`; do not
   commit it, and do not paste real paths or tokens into `docker-compose.yml`
   or this README.

### Variables

Model directories (absolute paths on the Docker host):

| Variable | Used by |
|---|---|
| `QWEN38_MODEL_DIR` | `qwen38-flash-next-fp4-local`, `...-tp2` |
| `QWEN_ASR_MODEL_DIR` | `qwen-asr` |
| `QWEN_TTS_MODEL_DIR` | `qwen-tts` |
| `NEMOTRON_VL_MODEL_DIR` | `nemotron-super-vl-120b-nvfp4`, `...-tp1` |
| `NEMOTRON_VL_DRAFT_DIR` | DFlash drafter for `nemotron-super-vl-120b-nvfp4` |
| `NEMOTRON_ASR_MODEL_DIR` | `nemotron-asr` |
| `MAGPIE_TTS_MODEL_DIR` | `magpie-tts` |

Runtime caches:

| Variable | Container mount |
|---|---|
| `HF_CACHE_DIR` | `/root/.cache/huggingface` |
| `VLLM_CACHE_DIR` | `/root/.cache/vllm` |
| `TRITON_CACHE_DIR` | `/root/.triton` |
| `FLASHINFER_CACHE_DIR` | `/root/.cache/flashinfer` |
| `LIL_CACHE_DIR` | `/cache` (Qwen3.8 JIT root) |
| `VLLM_OMNI_SPEAKERS_DIR` | `/root/.cache/vllm-omni/speakers` |
| `VLLM_OMNI_VOICES_DIR` | `/voices_precomputed` (read-only) |

`HF_CACHE_DIR` is usually by far the largest directory (hundreds of GB), so
it may point to a different, larger volume than the other caches.

Other settings:

| Variable | Default | Purpose |
|---|---|---|
| `HF_TOKEN` | empty | Hugging Face token for gated/private repos. Optional when checkpoints are already on disk. |
| `GPU_PRIMARY` | `0` | Primary GPU: Qwen LLM, `nemotron-asr`, and the first rank of the NVIDIA TP=2 LLM. |
| `GPU_SECONDARY` | `1` | Secondary GPU: `qwen-asr`, `qwen-tts`, `magpie-tts`, the second rank of the NVIDIA TP=2 LLM, and `nemotron-super-vl-120b-nvfp4-tp1`. |
| `MAGPIE_TTS_IMAGE` | `magpie-tts:latest` | Local image or registry copy used by `magpie-tts` and `nemotron-asr`. |

The helper scripts read the same model variables. Load `.env` into the shell
before running them:

```bash
cd docker
set -a
. ./.env
set +a
```

## Prerequisites

- Linux host with Docker Engine and Docker Compose v2.
- NVIDIA driver + `nvidia-container-toolkit` (`runtime: nvidia`).
- Two NVIDIA GPUs are assumed by the TP=2 services (the recipes were tuned on
  2x RTX PRO 6000 Blackwell 96 GB). TP=1 services use a single GPU. Override
  `GPU_PRIMARY` / `GPU_SECONDARY` if your indices differ.
- Model weights pre-downloaded on the host. The containers mount them
  read-only and do **not** download at runtime.
- Optional: a Hugging Face token in `HF_TOKEN` for gated repositories.

## Downloading model checkpoints

With `.env` loaded (`set -a; . ./.env; set +a`):

```bash
HF_HUB_DISABLE_XET=1 hf download nvidia/Qwen3.8-Flash-Next-FP4-Local \
  --local-dir "$QWEN38_MODEL_DIR"

HF_HUB_DISABLE_XET=1 hf download nvidia/NVIDIA-Nemotron-3.5-Super-VL-120B-A12B-NVFP4 \
  --local-dir "$NEMOTRON_VL_MODEL_DIR"

HF_HUB_DISABLE_XET=1 hf download nvidia/NVIDIA-Nemotron-3.5-Super-VL-120B-A12B-NVFP4-LiLiCorr \
  --local-dir "$NEMOTRON_VL_DRAFT_DIR"
```

The `hf` CLI uses `HF_TOKEN` from the environment when it is set.

## Preparing the NVIDIA voice assets

`magpie-tts` and `nemotron-asr` use a locally built
[`NVIDIA/NeMo-Speech.cpp`](https://github.com/NVIDIA/NeMo-Speech.cpp) image
(Apache-2.0, no NGC). The same image serves both TTS and ASR.

```bash
cd docker
set -a
. ./.env
set +a

# Build magpie-tts:latest and magpie-tts-builder:latest.
./magpie-tts/build-image.sh

# Download Magpie TTS weights into $MAGPIE_TTS_MODEL_DIR.
./magpie-tts/download-weights.sh

# Download Nemotron 3.5 ASR weights into $NEMOTRON_ASR_MODEL_DIR.
./nemotron-asr/download-weights.sh
```

Script overrides:

- `BUILD_DIR` (default `${XDG_CACHE_HOME:-$HOME/.cache}/nemo-speech-cpp`),
  `IMAGE`, `REGISTRY`, `CUDA_ARCH` for the image build.
- `MAGPIE_TTS_MODEL_DIR` / `NEMOTRON_ASR_MODEL_DIR` for the weight downloads
  (`MODELS_DIR` is also accepted for backwards compatibility).
- `MAGPIE_TTS_IMAGE` (Compose) to use a registry copy, e.g.
  `MAGPIE_TTS_IMAGE=localhost:5000/magpie-tts:latest`.

## Resource map

| Service | Profile | Role | Host port | GPU(s) | VRAM |
|---|---|---|---|---|---|
| `qwen38-flash-next-fp4-local` | `qwen` | LLM (TP=1) | 8005 | `GPU_PRIMARY` | ~0.96 of that GPU |
| `qwen-asr` | `qwen` | STT | 8024 | `GPU_SECONDARY` | 0.15 |
| `qwen-tts` | `qwen` | TTS | 8025 | `GPU_SECONDARY` | 0.09 + 0.06 |
| `nemotron-super-vl-120b-nvfp4` | `nvidia` | LLM (TP=2) | 8005 | both | 0.65 of both |
| `nemotron-super-vl-120b-nvfp4-tp1` | `nvidia` | LLM (TP=1) | 8007 | `GPU_SECONDARY` | 0.96 |
| `nemotron-asr` | `nvidia` | STT | 8026 | `GPU_PRIMARY` | small |
| `magpie-tts` | `nvidia` | TTS | 8025 | `GPU_SECONDARY` | small |

### What can run together

- **qwen:** the complete stack is designed to run together. The LLM takes
  ~0.96 of `GPU_PRIMARY`; `qwen-asr` (0.15) and `qwen-tts` (0.09 + 0.06) share
  `GPU_SECONDARY`. Start the whole profile with one command:
  `docker compose --profile qwen up -d`.
- **nvidia:** `nemotron-super-vl-120b-nvfp4` reserves 0.65 of each GPU, leaving
  roughly 35% free per GPU. The full NVIDIA stack can run together:
  TP=2 LLM + `nemotron-asr` + `magpie-tts`.
  The TP=1 variant (`8007`, 0.96 of `GPU_SECONDARY`) is an alternative to the
  TP=2 LLM, not an addition.

The qwen profile can be started as a whole. For the nvidia profile, name the
service because the two LLM variants are mutually exclusive:

```bash
docker compose --profile qwen   up -d
docker compose --profile nvidia up -d <service>
```

## Quick start: Qwen profile

```bash
cd docker

# LLM on GPU_PRIMARY; ASR + TTS on GPU_SECONDARY.
docker compose --profile qwen up -d

# Check health
docker compose ps
docker compose logs -f qwen38-flash-next-fp4-local
curl -s http://localhost:8005/health
curl -s http://localhost:8024/health
curl -s http://localhost:8025/health
```

Chat request:

```bash
curl -s http://localhost:8005/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"qwen38-flash-next","max_tokens":64,
       "messages":[{"role":"user","content":"hello"}]}' \
  | jq -r .choices[0].message.content
```

Voice services (already started by the profile):

```bash
# STT
curl -s http://localhost:8024/v1/audio/transcriptions \
  -F file=@sample.wav -F model=qwen3-asr | jq -r .text

# Qwen3-TTS Base has no built-in voices. Register a reference voice once
# (the uploaded voice persists in VLLM_OMNI_SPEAKERS_DIR):
curl -s -X POST http://localhost:8025/v1/audio/voices \
  -F name=speaker_test \
  -F consent=local-test-fixture \
  -F audio_sample=@sample.wav \
  -F ref_text='transcript of sample.wav'

# TTS (the first request warms up the model, ~90 s)
curl -s -X POST http://localhost:8025/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{"input":"hello, I am Seneschal","voice":"speaker_test","language":"English"}' \
  --output /tmp/tts.wav
```

## Quick start: NVIDIA profile

```bash
cd docker

# LLM, both GPUs (TP=2). First start can take a long time (JIT + ~70 GiB load).
docker compose --profile nvidia up -d nemotron-super-vl-120b-nvfp4

# STT + TTS can run alongside it.
docker compose --profile nvidia up -d nemotron-asr magpie-tts

docker compose ps
curl -s http://localhost:8005/health
curl -s http://localhost:8026/ready
curl -s http://localhost:8025/ready
```

Chat request (thinking is enabled by default, allow generous `max_tokens`):

```bash
curl -s http://localhost:8005/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"nemotron-3.5-super-vl","max_tokens":2048,
       "messages":[{"role":"user","content":"hello"}]}' \
  | jq -r .choices[0].message.content
```

Single-GPU alternative (port 8007; do not run with the TP=2 LLM):

```bash
docker compose --profile nvidia up -d nemotron-super-vl-120b-nvfp4-tp1

curl -s http://localhost:8007/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"nemotron-3.5-super-vl-tp1","max_tokens":2048,
       "messages":[{"role":"user","content":"hello"}]}' \
  | jq -r .choices[0].message.content
```

ASR / TTS requests:

```bash
# STT
curl -s http://localhost:8026/v1/models | jq .
curl -s -X POST http://localhost:8026/v1/audio/transcriptions \
  -F file=@sample.wav -F model=nemotron-3.5 | jq -r .text

# TTS (Magpie voices: John, Sofia, Aria, Jason, Leo)
curl -s -X POST http://localhost:8025/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{"input":"hello, I am Magpie","voice":"John","language":"en-US"}' \
  --output /tmp/magpie.wav
```

## Seneschal configuration

Set the matching provider variables in the project `.env`. Remote STT requires
the `openai-stt` feature; remote TTS requires `openai-tts`
(`cargo run --features full`, or the equivalent release build).

### Qwen profile

```dotenv
# LLM
LLM_URL=http://127.0.0.1:8005
LLM_MODEL=qwen38-flash-next

# STT (Qwen3-ASR)
STT_PROVIDER=openai
STT_URL=http://localhost:8024
STT_MODEL=qwen3-asr

# TTS (Qwen3-TTS Base)
# Base has no presets: register a voice first (see the Qwen quick start) and
# use its name here.
TTS_PROVIDER=openai
OPENAI_TTS_URL=http://localhost:8025
OPENAI_TTS_VOICE=speaker_test
OPENAI_TTS_LANGUAGE=auto
OPENAI_TTS_SAMPLE_RATE=24000
```

### NVIDIA profile

```dotenv
# LLM
LLM_URL=http://127.0.0.1:8005
LLM_MODEL=nemotron-3.5-super-vl
# For the TP=1 variant:
# LLM_URL=http://127.0.0.1:8007
# LLM_MODEL=nemotron-3.5-super-vl-tp1

# STT (Nemotron 3.5 ASR)
STT_PROVIDER=openai
STT_URL=http://localhost:8026
STT_MODEL=nemotron-3.5

# TTS (Magpie)
TTS_PROVIDER=openai
OPENAI_TTS_URL=http://localhost:8025
OPENAI_TTS_VOICE=John
OPENAI_TTS_LANGUAGE=en-US    # Magpie rejects "auto"/"Auto"
OPENAI_TTS_SAMPLE_RATE=22050 # NanoCodec rate
```

## Operations

```bash
# Status and logs (run from docker/)
docker compose --profile qwen ps
docker compose --profile nvidia ps
docker compose logs -f --tail=100 qwen-asr

# Stop a service
docker compose stop <service>

# Stop everything in one profile
docker compose --profile qwen stop
docker compose --profile nvidia stop

# Remove containers (keeps host model files)
docker compose --profile qwen down
docker compose --profile nvidia down
```

The LLM containers publish vLLM metrics. To inspect KV cache sizing and
concurrency:

```bash
docker compose logs <llm-service> 2>&1 \
  | grep -E 'KV cache size|Maximum concurrency|preempted|OOM|ERROR'
```

## Migrating from the previous stack

Container names mostly match the old stack. `qwen-asr` and `qwen-tts` replace
the old `seneschal-asr` and `seneschal-tts`, and
`qwen38-flash-next-fp4-local-tp2` was removed because it does not fit alongside
the voice services. The old stack and this stack cannot run at the same time.
Stop the old services before starting these. Set `OLD_STACK_DIR` to the
directory that contains the previous compose file:

```bash
cd "$OLD_STACK_DIR"
docker compose stop \
  qwen38-flash-next-fp4-local \
  qwen38-flash-next-fp4-local-tp2 \
  seneschal-asr \
  seneschal-tts \
  nemotron-super-vl-120b-nvfp4 \
  nemotron-super-vl-120b-nvfp4-tp1 \
  nemotron-asr \
  magpie-tts
```

Once the new stack is verified, the corresponding definitions can be removed
from the old `docker-compose.yml`. This repository does not modify that file.

## Notes and caveats

- **Exclusive services:** both profiles use host ports `8005` (LLM) and `8025`
  (TTS). `qwen-tts` and `magpie-tts` are mutually exclusive even within a
  single vendor run because they share `8025`.
- **`qwen-tts` served name:** the vLLM-Omni `qwen3_tts` pipeline does not
  accept a global `--served-model-name`, so the served model is `/model`.
- **`qwen-tts` voices:** the Base checkpoint has no presets. Register a voice
  with `POST /v1/audio/voices` (the `consent` field is required) and set
  `OPENAI_TTS_VOICE` to that registered name.
- **Host RAM:** the Qwen3.8 runtime keeps its PLE table in host RAM (~27 GiB).
  Check `free -g` before starting it. To move the table to disk, set
  `VLLM_PLE_TABLE_MEMORY=disk` and `B12X_DISK_BACKEND=io_uring`.
- **First start:** LLM services can take minutes to tens of minutes (JIT
  compilation + up to ~96 GiB of weights). Health checks use generous
  `start_period` values.
- **Configuration safety:** `docker-compose.yml` only contains `${VAR}`
  references. Real paths, tokens and GPU indices belong in `.env`, which is
  git-ignored. Run `git status` before committing to make sure `.env` is not
  staged. Note that `docker compose config` prints resolved values, including
  host paths, so do not paste its output into issues or logs.
- **Restart behavior:** Qwen3.8 services use `restart: on-failure` so a
  configuration error does not reload ~96 GiB in a loop. The rest use
  `unless-stopped`.
