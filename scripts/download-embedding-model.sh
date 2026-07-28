#!/usr/bin/env bash
# download-embedding-model.sh — Download bge-small ONNX model for Nivel 2 classifier.
# Run once: bash scripts/download-embedding-model.sh
# Places model in models/bge-small-onnx/
set -euo pipefail

MODEL_DIR="$(cd "$(dirname "$0")/.." && pwd)/models/bge-small-onnx"
echo "Target: $MODEL_DIR"
mkdir -p "$MODEL_DIR"

# all-MiniLM-L6-v2 ONNX from Qdrant's HuggingFace mirror (50 MB)
BASE_URL="https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main/onnx"
FILES=("model.onnx" "tokenizer.json")

for file in "${FILES[@]}"; do
    dest="$MODEL_DIR/$file"
    if [ -f "$dest" ] && [ -s "$dest" ]; then
        echo "  $file already present — skipping"
        continue
    fi
    url="$BASE_URL/$file"
    echo "  Downloading $url ..."
    curl -fsSL -o "$dest" "$url" || {
        echo "ERROR: Failed to download $file from $url"
        rm -f "$dest"
        exit 1
    }
    size=$(stat -f%z "$dest" 2>/dev/null || stat -c%s "$dest" 2>/dev/null)
    echo "  $file — $size bytes OK"
done

echo "Done. Model available at $MODEL_DIR"
echo "Set CLASSIFIER_MODEL_PATH=$MODEL_DIR to enable Nivel 2."
