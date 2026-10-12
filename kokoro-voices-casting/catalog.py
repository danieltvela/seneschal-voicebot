"""Catálogo de voces de Kokoro v1.0 (repo hexgrad/Kokoro-82M).

Datos tomados de VOICES.md del repo oficial (sep 2026).
Campos: nombre, codigo de idioma G2P, genero, nota de calidad del repo,
y extra de pip necesaria para las lenguas que no van por espeak-ng.
"""

# Nota de calidad segun VOICES.md; "" = sin nota oficial
# Orden de notas para filtrar (peor -> mejor)
GRADE_ORDER = ["", "F+", "D-", "D", "D+", "C-", "C", "C+", "B-", "B", "B+", "A-", "A", "A+"]

# (nombre, lenguaje, genero, grado, extra opcional)
VOICES = [
    # --- American English (lang_code a) ---
    ("af_heart",   "a", "F", "A",   ""),
    ("af_alloy",   "a", "F", "C",   ""),
    ("af_aoede",   "a", "F", "C+",  ""),
    ("af_bella",   "a", "F", "A-",  ""),
    ("af_jessica", "a", "F", "D",   ""),
    ("af_kore",    "a", "F", "C+",  ""),
    ("af_nicole",  "a", "F", "B-",  ""),
    ("af_nova",    "a", "F", "C",   ""),
    ("af_river",   "a", "F", "D",   ""),
    ("af_sarah",   "a", "F", "C+",  ""),
    ("af_sky",     "a", "F", "C-",  ""),
    ("am_adam",    "a", "M", "F+",  ""),
    ("am_echo",    "a", "M", "D",   ""),
    ("am_eric",    "a", "M", "D",   ""),
    ("am_fenrir",  "a", "M", "C+",  ""),
    ("am_liam",    "a", "M", "D",   ""),
    ("am_michael", "a", "M", "C+",  ""),
    ("am_onyx",    "a", "M", "D",   ""),
    ("am_puck",    "a", "M", "C+",  ""),
    ("am_santa",   "a", "M", "D-",  ""),
    # --- British English (lang_code b) ---
    ("bf_alice",   "b", "F", "D",   ""),
    ("bf_emma",    "b", "F", "B-",  ""),
    ("bf_isabella","b", "F", "C",   ""),
    ("bf_lily",    "b", "F", "D",   ""),
    ("bm_daniel",  "b", "M", "D",   ""),
    ("bm_fable",   "b", "M", "C",   ""),
    ("bm_george",  "b", "M", "C",   ""),
    ("bm_lewis",   "b", "M", "D+",  ""),
    # --- Spanish (lang_code e, espeak-ng) ---
    ("ef_dora",    "e", "F", "",    ""),
    ("em_alex",    "e", "M", "",    ""),
    ("em_santa",   "e", "M", "",    ""),
    # --- French (lang_code f, espeak-ng) ---
    ("ff_siwis",   "f", "F", "B-",  ""),
    # --- Hindi (lang_code h, espeak-ng) ---
    ("hf_alpha",   "h", "F", "C",   ""),
    ("hf_beta",    "h", "F", "C",   ""),
    ("hm_omega",   "h", "M", "C",   ""),
    ("hm_psi",     "h", "M", "C",   ""),
    # --- Italian (lang_code i, espeak-ng) ---
    ("if_sara",    "i", "F", "C",   ""),
    ("im_nicola",  "i", "M", "C",   ""),
    # --- Brazilian Portuguese (lang_code p, espeak-ng) ---
    ("pf_dora",    "p", "F", "",    ""),
    ("pm_alex",    "p", "M", "",    ""),
    ("pm_santa",   "p", "M", "",    ""),
    # --- Japanese (lang_code j, requiere misaki[ja]) ---
    ("jf_alpha",      "j", "F", "C+", "ja"),
    ("jf_gongitsune", "j", "F", "C",  "ja"),
    ("jf_nezumi",     "j", "F", "C-", "ja"),
    ("jf_tebukuro",   "j", "F", "C",  "ja"),
    ("jm_kumo",       "j", "M", "C-", "ja"),
    # --- Mandarin Chinese (lang_code z, requiere misaki[zh]) ---
    ("zf_xiaobei", "z", "F", "D", "zh"),
    ("zf_xiaoni",  "z", "F", "D", "zh"),
    ("zf_xiaoxiao","z", "F", "D", "zh"),
    ("zf_xiaoyi",  "z", "F", "D", "zh"),
    ("zm_yunjian", "z", "M", "D", "zh"),
    ("zm_yunxi",   "z", "M", "D", "zh"),
    ("zm_yunxia",  "z", "M", "D", "zh"),
    ("zm_yunyang", "z", "M", "D", "zh"),
]

LANG_NAMES = {
    "a": "English (US)",
    "b": "English (UK)",
    "e": "Spanish",
    "f": "French",
    "h": "Hindi",
    "i": "Italian",
    "p": "Portuguese (BR)",
    "j": "Japanese",
    "z": "Mandarin",
}

# Frases de muestra por lengua (100-200 fonemas, el punto dulce de Kokoro;
# para espeak-ng mejor corto porque no trocea texto largo).
SAMPLES = {
    "a": "Good evening, Daniel. This is a test of my voice. I speak at a steady pace, "
         "clear enough for you to judge the tone, the warmth, and the character of this voice.",
    "b": "Good evening, Daniel. This is a test of my voice. I speak at a steady pace, "
         "clear enough for you to judge the tone, the warmth, and the character of this voice.",
    "e": "Buenas noches, Daniel. Estoy probando cómo sueno. "
         "Esta frase es bastante larga para que notes el ritmo, la claridad y el acento.",
    "f": "Bonsoir Daniel. Je teste ma voix. Cette phrase est assez longue pour que tu "
         "remarques le rythme, la clarté et le caractère de cette voix.",
    "h": "नमस्ते डैनियल। मैं अपनी आवाज़ आज़मा रही हूँ। इस वाक्य से आपको मेरी आवाज़, "
         "मेरी स्पष्टता और मेरा अंदाज़ साफ़ सुनाई देगा।",
    "i": "Buonasera Daniel. Sto provando la mia voce. Questa frase è abbastanza lunga "
         "così puoi notare il ritmo, la chiarezza e il carattere di questa voce.",
    "p": "Boa noite, Daniel. Estou testando a minha voz. Esta frase é longa o suficiente "
         "para você notar o ritmo, a clareza e o caráter desta voz.",
    "j": "こんばんは、ダニエル。声を試しています。この文は長さがあるので、声の速さ、"
         "明瞭さ、そしてこの声の雰囲気がよくわかると思います。",
    "z": "晚上好，丹尼尔。我正在测试自己的声音。这句话足够长，"
         "你可以听出我的语速、清晰度和音色特点。",
}

BY_NAME = {v[0]: v for v in VOICES}


def voices_by_lang(prefix: str):
    return [v for v in VOICES if v[0].startswith(prefix + "_") or v[0] == prefix]
