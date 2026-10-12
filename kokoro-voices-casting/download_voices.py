#!/usr/bin/env python3
"""Descarga el modelo Kokoro-82M v1.0 y los ficheros de voz (.pt) localmente.

Hace cache en ./model y ./voices via huggingface_hub (verificación de integridad
integrada). Idempotente: lo que ya está, no lo vuelve a bajar.

Ejemplos:
  uv run python download_voices.py --model          # config.json + kokoro-v1_0.pth
  uv run python download_voices.py --all            # las 54 voces
  uv run python download_voices.py --langs e a      # solo español + inglés US
  uv run python download_voices.py af_heart ef_dora # voces concretas
"""
import argparse
import sys
from pathlib import Path

from huggingface_hub import hf_hub_download

from catalog import VOICES, BY_NAME

REPO_ID = "hexgrad/Kokoro-82M"
ROOT = Path(__file__).resolve().parent
MODEL_DIR = ROOT / "model"
VOICES_DIR = ROOT / "voices"


def fetch_model():
    for fn in ("config.json", "kokoro-v1_0.pth"):
        p = hf_hub_download(repo_id=REPO_ID, filename=fn, local_dir=str(MODEL_DIR))
        print(f"  model/{fn}: ok")


def fetch_voice(name: str):
    dest = VOICES_DIR / f"{name}.pt"
    if dest.exists() and dest.stat().st_size > 10_000:
        print(f"  {name}.pt: ya existe")
        return
    # local_dir=raiz conserva la estructura del repo -> cae en voices/<name>.pt
    hf_hub_download(repo_id=REPO_ID, filename=f"voices/{name}.pt",
                    local_dir=str(ROOT))
    print(f"  {name}.pt: ok")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("names", nargs="*", help="voces concretas (p.ej. af_heart ef_dora)")
    ap.add_argument("--model", action="store_true", help="bajar config.json + pesos del modelo")
    ap.add_argument("--all", action="store_true", help="las 54 voces")
    ap.add_argument("--langs", nargs="+", metavar="L",
                    help="voces de estas lenguas (a b e f h i p j z)")
    args = ap.parse_args()

    if args.model:
        print("Descargando modelo (repo HF hexgrad/Kokoro-82M)...")
        fetch_model()

    names = list(args.names)
    if args.all:
        names = [v[0] for v in VOICES]
    if args.langs:
        for l in args.langs:
            if l not in "abefhijpz":
                sys.exit(f"Lengua desconocida: {l} (válidas: a b e f h i p j z)")
        names += [v[0] for v in VOICES if v[1] in set(args.langs)]

    seen, todo = set(), []
    for n in names:
        if n not in BY_NAME:
            sys.exit(f"Voz desconocida: {n}. Usa './casting.sh list' para verlas.")
        if n not in seen:
            seen.add(n)
            todo.append(n)

    if not todo and not args.model:
        ap.print_help()
        sys.exit(0)

    if todo:
        print(f"Descargando {len(todo)} voz/veces...")
        for n in todo:
            fetch_voice(n)
    print("Hecho.")


if __name__ == "__main__":
    main()
