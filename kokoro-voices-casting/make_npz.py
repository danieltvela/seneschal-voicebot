#!/usr/bin/env python3
"""Empaqueta las voces propias del casting (.pt) en un NPZ para backends ONNX.

El mundo Kokoro se divide en dos familias de formatos de voz:
  - Python `kokoro` (KPipeline/KModel): lee cada voz como .pt individual.
    Para él NO hace falta este script: se pasa directamente el .pt.
  - Pila ONNX (kokorox/kokoro.cpp/kokoro.js...): lee UN solo fichero NPZ
    (zip) con una entrada por voz: nombre -> tensor [510,1,256] float32.
    El oficial `voices-v1.0.bin` es, de hecho, un NPZ renombrado.

Este script parte del NPZ oficial y añade/actualiza las mezclas de este
directorio (las .pt listadas en voices/custom.json), escribiendo un NPZ
nuevo sin tocar el original.

Uso:
  uv run python make_npz.py --base ../models/voices-v1.0.bin --out ../models/voices-v1.0-custom.bin
  uv run python make_npz.py                     # out/voices-custom.npz (solo las propias)
"""
import argparse
import json
from pathlib import Path

import numpy as np
import torch

ROOT = Path(__file__).resolve().parent
VOICES_DIR = ROOT / "voices"


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--base", default=None,
                    help="NPZ oficial (p.ej. voices-v1.0.bin). Opcional: sin él, solo las propias.")
    ap.add_argument("--out", default=str(ROOT / "out" / "voices-custom.npz"))
    args = ap.parse_args()

    voices = {}
    if args.base:
        with np.load(args.base) as z:
            for name in z.files:
                voices[name] = np.asarray(z[name], dtype=np.float32)
        print(f"  base: {len(voices)} voces desde {args.base}")

    custom_file = VOICES_DIR / "custom.json"
    custom = json.loads(custom_file.read_text()) if custom_file.exists() else {}
    for name in custom:
        pt = VOICES_DIR / f"{name}.pt"
        if not pt.exists():
            print(f"  AVISO: {name} en custom.json pero falta {pt}; se omite")
            continue
        t = torch.load(pt, weights_only=True).float().numpy()
        assert t.shape == (510, 1, 256), f"{pt} tiene forma {t.shape}, esperaba (510,1,256)"
        voices[name] = np.ascontiguousarray(t, dtype=np.float32)
        print(f"  propia: {name} ({custom[name]['recipe']})")

    if not voices:
        raise SystemExit("nada que empaquetar: ni --base ni mezclas en voices/custom.json")

    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "wb") as f:
        np.savez(f, **voices)
    print(f"  escrito: {out} ({len(voices)} voces)")
    print("\nEn backends ONNX: apunta tu variable de voces a este fichero y usa")
    print("KOKORO_VOICE=mayordomo_fable (o el nombre que hayas dado a tu mezcla).")


if __name__ == "__main__":
    main()
