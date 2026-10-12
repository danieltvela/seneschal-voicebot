#!/usr/bin/env python3
"""Casting de voces Kokoro: sintetiza una o varias voces y las reproduce.

Cada WAV se cachea en out/ para que oír de nuevo una voz sea instantáneo.
El modelo (kokoro-v1_0.pth) se carga UNA vez por invocación, aunque pidas
varias voces — por eso "battle" es rápido tras la primera carga (~10 s CPU).

Ejemplos:
  uv run python cast.py list                     # catalogo
  uv run python cast.py say ef_dora              # una voz, su frase en español
  uv run python cast.py say af_heart -t "hola"   # texto propio
  uv run python cast.py battle ef_dora em_alex em_santa   # una tras otra
  uv run python cast.py mix em_alex:2,em_santa:1 --name castizo  # voz propia:
    la mezcla se guarda como voices/castizo.pt, utilizable como cualquier .pt
  uv run python cast.py best                     # las mejor puntuadas
  uv run python cast.py all --yes                # las 49 sin japonés/chino
"""
import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import NoReturn

import torch
import soundfile as sf

from catalog import VOICES, BY_NAME, LANG_NAMES, SAMPLES, GRADE_ORDER

ROOT = Path(__file__).resolve().parent
MODEL_DIR = ROOT / "model"
VOICES_DIR = ROOT / "voices"
OUT_DIR = ROOT / "out"
CUSTOM_FILE = VOICES_DIR / "custom.json"

SAMPLE_RATE = 24000

# aplay solo (la caja no tiene PulseAudio/PipeWire). Tarjetas vistas:
#   4 = "P20M" (Poly Sync 20-M, altavoz en uso), 3 = "Audio" (USB), 0-2 = HDMI.
ALSA_DEVICE = os.environ.get("KOKORO_ALSA_DEVICE", "default:CARD=P20M")


def die(msg: str) -> NoReturn:
    print(f"ERROR: {msg}", file=sys.stderr)
    sys.exit(1)


def load_custom() -> dict:
    """Registro de voces creadas con 'mix': nombre -> {lang, recipe}."""
    if CUSTOM_FILE.exists():
        try:
            return json.loads(CUSTOM_FILE.read_text())
        except json.JSONDecodeError:
            return {}
    return {}


# ---------------------------------------------------------------- catálogo

def cmd_list(args):
    missing = []
    print(f"{'VOZ':<14} {'LENGUA':<17} {'GEN':<4} {'GRADO':<6} LOCAL")
    print("-" * 52)
    for name, lang, gender, grade, extra in VOICES:
        if args.lang and lang not in set(args.lang):
            continue
        if args.gender and gender != args.gender.upper():
            continue
        have = (VOICES_DIR / f"{name}.pt").exists()
        if args.missing and have:
            continue
        missing.append(name) if not have else None
        flag = "sí " if have else ("FALTA" if not extra else "FALTA*")
        print(f"{name:<14} {LANG_NAMES[lang]:<17} {gender:<4} {grade or '-':<6} {flag}")
    if args.missing and missing:
        print(f"\nFaltan {len(missing)}: descarga con ->")
        print(f"  uv run python download_voices.py {' '.join(missing[:12])}{' ...' if len(missing) > 12 else ''}")
    if not args.missing:
        print("\n* japonés/chino piden extras: uv pip install 'misaki[ja,zh]' (pesado; no incluido).")
        print("  Grados = calidad de datos de entrenamiento según VOICES.md oficial.")
        custom = load_custom()
        if custom:
            print("\nVOCES PROPIAS (creadas con 'mix'):")
            for k, v in custom.items():
                have = "sí" if (VOICES_DIR / f"{k}.pt").exists() else "FALTA"
                print(f"  {k:<20} {LANG_NAMES.get(v['lang'], '?'):<17} {v['recipe']}  {have}")


# ---------------------------------------------------------------- síntesis

_MODEL = None


def get_model(device: str):
    global _MODEL
    if _MODEL is not None:
        return _MODEL
    cfg = MODEL_DIR / "config.json"
    wts = MODEL_DIR / "kokoro-v1_0.pth"
    if not cfg.exists() or not wts.exists():
        die("falta el modelo. Ejecuta: uv run python download_voices.py --model")
    from kokoro import KModel
    print("Cargando modelo Kokoro-82M (primera vez ~10-20 s)...", flush=True)
    _MODEL = KModel(config=str(cfg), model=str(wts)).to(device).eval()
    return _MODEL


_PIPELINES = {}


def get_pipeline(lang: str, device: str):
    if lang in _PIPELINES:
        return _PIPELINES[lang]
    from kokoro import KPipeline
    try:
        pipeline = KPipeline(lang_code=lang, model=get_model(device), device=device,
                             repo_id="hexgrad/Kokoro-82M")
    except ImportError as e:
        die(f"falta el extra para esta lengua ({e}). "
            "Prueba: uv pip install 'misaki[ja]' o 'misaki[zh]'")
    _PIPELINES[lang] = pipeline
    return pipeline


def _synthesize(name: str, lang: str, voice_file: Path, text: str, speed: float, device: str) -> Path:
    """Genera (o reutiliza del caché) el WAV de la voz `name` diciendo `text`."""
    safe = text.strip()
    out = OUT_DIR / f"{name}.wav"
    marker = OUT_DIR / f"{name}.txt"
    if (out.exists() and marker.exists() and marker.read_text() == safe and out.stat().st_size > 1000
            and voice_file.exists() and out.stat().st_mtime_ns > voice_file.stat().st_mtime_ns):
        # el .pt debe ser más viejo que el WAV: si has remezclado con el mismo
        # nombre, el caché antiguo ya no vale y se regenera solo.
        return out
    pipe = get_pipeline(lang, device)
    if not voice_file.exists():
        die(f"falta la voz {name}. Descárgala: uv run python download_voices.py {name}")

    audio_parts = []
    for _, _, audio in pipe(safe, voice=str(voice_file), speed=speed):
        if audio is not None:
            audio_parts.append(audio)
    if not audio_parts:
        die(f"no se generó audio para '{safe[:40]}' con {name}")
    wave = torch.cat(audio_parts).float().cpu().numpy() if len(audio_parts) > 1 else audio_parts[0].float().cpu().numpy()
    OUT_DIR.mkdir(exist_ok=True)
    tmp = out.with_name(out.stem + ".tmp.wav")
    sf.write(str(tmp), wave, SAMPLE_RATE, format="WAV")
    tmp.replace(out)
    marker.write_text(safe)
    secs = len(wave) / SAMPLE_RATE
    print(f"  {name}: {secs:.1f} s -> {out.relative_to(ROOT)}")
    return out


def _card_of(dev: str) -> str | None:
    """default:CARD=Audio -> 'Audio' (amixer acepta el nombre de tarjeta)."""
    m = re.search(r"CARD=([A-Za-z0-9._-]+)", dev)
    return m.group(1) if m else None


def _ensure_audible(dev: str):
    """Quita el mute del mezclador ALSA. Con el PCM muteado, aplay devuelve 0
    y no suena nada — el fallo silencioso que hace semanas dejaba el casting mudo."""
    card = _card_of(dev)
    if not card:
        return
    for ctl in ("PCM,0", "PCM", "Master"):
        r = subprocess.run(["amixer", "-c", card, "sget", ctl],
                           capture_output=True, text=True)
        if r.returncode != 0:
            continue
        if "[off]" in r.stdout:
            subprocess.run(["amixer", "-c", card, "sset", ctl, "on"],
                           capture_output=True)
            print(f"(tarjeta {card}: el mezclador estaba muteado; desmudeada)")
        elif re.search(r"Playback \d+ \[0%\]", r.stdout):
            subprocess.run(["amixer", "-c", card, "sset", ctl, "70%"],
                           capture_output=True)
            print(f"(tarjeta {card}: volumen a 0; subido a 70%)")
        break


def play(path: Path):
    """Suena un WAV por ALSA (bloqueante, como debe ser en un casting)."""
    dev = ALSA_DEVICE
    _ensure_audible(dev)
    r = subprocess.run(["aplay", "-D", dev, "-q", str(path)])
    if r.returncode != 0:
        print(f"(aplay falló con -D {dev}; prueba KOKORO_ALSA_DEVICE o --alsa. "
              "Dispositivos: aplay -L | grep -E '^(default|sysdefault)')",
              file=sys.stderr)


# ---------------------------------------------------------------- comandos

def resolve_voices(spec: str):
    names = [s.strip() for s in spec.split(",") if s.strip()]
    custom = load_custom()
    for n in names:
        if n not in BY_NAME and n not in custom:
            die(f"voz desconocida: {n} (usa 'list')")
    return names


def run_say(names, text, speed, device, no_play):
    custom = load_custom()
    for n in names:
        entry = BY_NAME.get(n)
        lang = entry[1] if entry else custom[n]["lang"]
        t = text if text is not None else SAMPLES[lang]
        if entry and entry[4] and text is None and lang in "jz":
            print(f"({n}: muestra en {'japonés' if lang=='j' else 'chino'}; requiere misaki extra)")
        w = _synthesize(n, lang, VOICES_DIR / f"{n}.pt", t, speed, device)
        if not no_play:
            play(w)


def cmd_say(args):
    names = resolve_voices(args.voice)
    run_say(names, args.text, args.speed, args.device, args.no_play)


def cmd_battle(args):
    names = resolve_voices(args.voices)
    print(f"Casting de {len(names)} voces por turnos. Ctrl-C para parar.")
    run_say(names, args.text, args.speed, args.device, args.no_play)


def cmd_mix(args):
    """Mezcla voicepacks oficiales ponderados en una voz nueva (.pt) y la suena.
    Los tensores de estilo [510,1,256] interpolan linealmente (técnica usada y
    documentada por el pack sueco Joakim/kokoro-sv-voices)."""
    parts = []
    for token in args.spec.split(","):
        token = token.strip()
        if not token:
            continue
        base, _, w = token.rpartition(":")
        try:
            w = float(w) if w else 1.0
        except ValueError:
            die(f"peso inválido en '{token}' (formato: voz:peso)")
        if w <= 0:
            die(f"el peso de {base} debe ser > 0")
        if base not in BY_NAME:
            die(f"voz desconocida: {base} (usa 'list')")
        f = VOICES_DIR / f"{base}.pt"
        if not f.exists():
            die(f"falta {base}.pt. Descárgala: uv run python download_voices.py {base}")
        parts.append((base, w, f))
    if len(parts) < 2:
        die("mix necesita al menos dos voces: voz1:peso,voz2:peso")
    name = args.name or "mix_" + "_".join(b for b, _, _ in parts)
    if name in BY_NAME:
        die(f"'{name}' es una voz oficial; elige otro nombre con --name")

    total = sum(w for _, w, _ in parts)
    vec = torch.zeros(510, 1, 256)
    for base, w, f in parts:
        vec += (w / total) * torch.load(f, weights_only=True)
    out_voice = VOICES_DIR / f"{name}.pt"
    torch.save(vec, out_voice)
    receta = " + ".join(f"{w / total:.2f}·{b}" for b, w, _ in parts)
    print(f"  voz nueva: voices/{name}.pt = {receta}")

    langs = {BY_NAME[b][1] for b, _, _ in parts}
    lang = BY_NAME[parts[0][0]][1]
    if len(langs) > 1:
        print(f"  (mezcla de lenguas: el G2P será el de la primera voz, '{lang}')")
    reg = load_custom()
    reg[name] = {"lang": lang, "recipe": receta}
    CUSTOM_FILE.write_text(json.dumps(reg, ensure_ascii=False, indent=2) + "\n")
    t = args.text if args.text is not None else SAMPLES[lang]
    w = _synthesize(name, lang, out_voice, t, args.speed, args.device)
    if not args.no_play:
        play(w)


BEST = ["af_heart", "af_bella", "af_nicole", "af_aoede", "af_kore", "af_sarah",
        "am_fenrir", "am_michael", "am_puck",
        "bf_emma", "bm_george", "bm_fable",
        "ef_dora", "em_alex", "em_santa",
        "ff_siwis"]


def cmd_best(args):
    print("Las 16 mejor valoradas en VOICES.md (las que importan para un voicebot).")
    run_say(BEST, args.text, args.speed, args.device, args.no_play)


def cmd_all(args):
    names = [v[0] for v in VOICES if v[1] not in "jz"]  # ja/zh piden extras
    if not args.yes:
        print(f"Vas a oír {len(names)} voces seguidas (~10 min).")
        print("Recomendado empezar por ./casting.sh best")
        print("Si quieres aun así: añade --yes")
        sys.exit(0)
    print(f"{len(names)} voces. Ctrl-C para parar en cualquier momento.")
    run_say(names, None, 1.0, args.device, args.no_play)


# ---------------------------------------------------------------- main

def main():
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--device", default=None,
                        help="cpu (por defecto; apenas 300 MB) o cuda")
    common.add_argument("--alsa", default=None,
                        help="dispositivo ALSA para aplay (default:CARD=Audio, CARD=P20M...)")
    common.add_argument("--no-play", action="store_true", default=None,
                        help="solo generar WAV, no reproducir")

    ap = argparse.ArgumentParser(prog="cast.py", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("list", help="catálogo de voces", parents=[common])
    p.add_argument("--lang", nargs="+", metavar="L")
    p.add_argument("--gender", choices="mfMF")
    p.add_argument("--missing", action="store_true", help="solo las que faltan por descargar")

    p = sub.add_parser("say", help="una o varias voces (voz1,voz2) dicen un texto", parents=[common])
    p.add_argument("voice")
    p.add_argument("-t", "--text", default=None, help="texto propio (por defecto, frase en el idioma de la voz)")
    p.add_argument("--speed", type=float, default=1.0)

    p = sub.add_parser("battle", help="varias voces, una tras otra, mismo texto", parents=[common])
    p.add_argument("voices")
    p.add_argument("-t", "--text", default=None)
    p.add_argument("--speed", type=float, default=1.0)

    p = sub.add_parser("mix", help="crea una voz nueva mezclando oficiales (voz:peso,voz:peso)", parents=[common])
    p.add_argument("spec", help="ej: em_alex:2,em_santa:1 — los pesos se normalizan solos")
    p.add_argument("--name", default=None, help="nombre de la voz resultante (se guarda en voices/<name>.pt)")
    p.add_argument("-t", "--text", default=None)
    p.add_argument("--speed", type=float, default=1.0)

    p = sub.add_parser("best", help="las 16 mejor puntuadas, una tras otra", parents=[common])
    p.add_argument("-t", "--text", default=None)
    p.add_argument("--speed", type=float, default=1.0)

    p = sub.add_parser("all", help="las 49 sin japonés/chino (¡largo!)", parents=[common])
    p.add_argument("--yes", action="store_true")

    args = ap.parse_args()
    if args.device is None:
        args.device = os.environ.get("KOKORO_DEVICE", "cpu")
    if args.no_play is None:
        args.no_play = False
    if args.alsa:
        globals()["ALSA_DEVICE"] = args.alsa

    {"list": cmd_list, "say": cmd_say, "battle": cmd_battle, "mix": cmd_mix,
     "best": cmd_best, "all": cmd_all}[args.cmd](args)


if __name__ == "__main__":
    main()
