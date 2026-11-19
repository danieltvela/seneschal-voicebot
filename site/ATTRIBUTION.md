# Attribution & License status

The **Learn** course (`site/learn/`) is original Seneschal content: our own text,
lesson data and engine. Its *format* (step-through animated isometric lessons) is
inspired by **designed-to-scale.vercel.app** ("System Design — an interactive course"),
**© Dilum Sanjaya** ([@DilumSanjaya](https://x.com/DilumSanjaya), github.com/dilums).
No code, assets or lesson text from that site ships with this repository.

## License status (checked 2026-11-19)

- The site declares **no license**: no LICENSE/©/terms in the HTML, none in the
  bundle. No public source repository exists (his GitHub `dilums` — 116 repos,
  none is designed-to-scale; GitHub search: 0 results).
- No license stated ⇒ **all rights reserved** by default. Attribution alone does
  not grant redistribution rights.
- That site was mirrored internally for study (2026-11-19, `doc/interactive-web/`,
  since removed); nothing from the mirror is published here.

## What is ours vs his

- **Ours (original content)**: the seven lessons in module "Voice Systems" —
  *AI Voice Assistant*, *Subagent: Deep Worker*, *S-DREAM*, *Knowing When You're Done*,
  *The Announcement Window*, *EYES — Visual Awareness*, *The Ear — Audio Internals* —
  written for the Seneschal project (text and lesson data), plus all local patches
  documented in `README.md`.
- **His (© Dilum Sanjaya)**: the original site's app engine, theme, chrome and
  its 40 lessons — none of it included here (the engine was reimplemented from
  the format; see `learn/js/app.js`). The only third-party asset is the icon
  path subset (lucide, ISC) in `learn/js/icons.js`.

## If we ever publish

Ask the author for permission (DM on X/LinkedIn — his site invites contact).
Alternative: reimplement the engine from the format (ideas are not copyrightable;
his implementation is). Do not publish his code or lessons without consent.

## Resolution (2026-11-19): own engine built

Decision: rewrite the engine instead of publishing his code. The public-facing
course lives in `site/learn/` — a from-scratch viewer (own isometric SVG renderer
+ UI, Cormallen theme) rendering our seven lessons from `site/learn/js/lessons-data.js`.
Served by `seneschal-site.service` (:8124).
