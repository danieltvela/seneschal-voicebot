FROM ubuntu:24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_TERM_COLOR=always \
    CARGO_HOME=/root/.cargo \
    RUSTUP_HOME=/root/.rustup \
    RUSTFLAGS="-C link-arg=-lgomp"

RUN apt-get update -qq \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        build-essential \
        pkg-config \
        libssl-dev \
        libasound2-dev \
        cmake \
        libclang-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal \
    && . "$CARGO_HOME/env" && rustc --version && cargo --version

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY voicebot.pro.toml ./
COPY src ./src
COPY tests ./tests
COPY patches ./patches

RUN . "$CARGO_HOME/env" \
    && cargo build --release --bin voicebot --features "tui,remote,control"

FROM ubuntu:24.04 AS runtime

ENV DEBIAN_FRONTEND=noninteractive \
    VOICEBOT_HOME=/root/.voicebot \
    MODELS_DIR=/build/models

RUN apt-get update -qq \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        libasound2t64 \
        libgomp1 \
        espeak-ng \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/voicebot /usr/local/bin/voicebot
COPY .gitea/install-test/smoke-test.sh /usr/local/bin/voicebot-smoke-test
RUN chmod +x /usr/local/bin/voicebot-smoke-test
COPY uninstall.sh /app/uninstall.sh
RUN chmod +x /app/uninstall.sh

RUN mkdir -p "$MODELS_DIR" \
    && (curl -fsSL --max-time 120 \
            -o "$MODELS_DIR/kokoro-v1.0.onnx" \
            "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.onnx" \
         || echo "[smoke] WARN: kokoro-v1.0.onnx download failed; --list-voices will be skipped") \
    && (curl -fsSL --max-time 60 \
            -o "$MODELS_DIR/voices-v1.0.bin" \
            "https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin" \
         || echo "[smoke] WARN: voices-v1.0.bin download failed; --list-voices will be skipped") \
    && (curl -fsSL --max-time 60 \
            -o "$MODELS_DIR/ggml-silero-vad.bin" \
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx" \
         || echo "[smoke] WARN: silero_vad.onnx download failed")

ENV KOKORO_MODEL="$MODELS_DIR/kokoro-v1.0.onnx" \
    KOKORO_VOICES="$MODELS_DIR/voices-v1.0.bin" \
    VAD_MODEL="$MODELS_DIR/ggml-silero-vad.bin" \
    TTS_PROVIDER=kokoro

ENTRYPOINT ["/usr/local/bin/voicebot-smoke-test"]
