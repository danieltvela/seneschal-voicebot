# Seneschal — Brand & Logo Guide

Visual identity for **Seneschal**, a mono-user voice AI assistant.
The name and mark are inspired by the **Steward of Gondor** (Tolkien's
*Lord of the Rings*): the seneschal serves the king — and in this project,
**the user is the king**. The Steward's banner is a **plain white standard
with no house shield or emblem**, which is exactly the restraint this mark
embodies.

> Trademark note: "Jarvis®" is a trademark of Marvel Studios/Disney. This is
> an independent fan project and is referred to only as **Seneschal**.

## Logo files

| File | Purpose |
|------|---------|
| `assets/logo.svg` | Primary horizontal lockup — icon + wordmark. |
| `assets/logo-icon.svg` | Icon only (banner mark) — favicon, avatars, app stores. |
| `assets/logo-wordmark.svg` | Wordmark only — footers, headers, documents. |
| `assets/concepts/seneschal_mark_stars_banner_v1.svg` | Earlier concept study (three stars over standard); kept for reference only. |

## The mark

The icon is a **plain white banner** hung from a bar, with a swallowtail lower
edge and **no emblem** — faithful to the Steward of Gondor's sigil. It
deliberately avoids crowns, shields, or crests: the steward bears no kingly
device.

## Color

The artwork uses `currentColor`, so it themes automatically.

| Token | Hex | Use |
|-------|-----|-----|
| `--banner` | `#FFFFFF` | Banner fill on **dark** fields (primary). |
| `--ink` | `#0B1220` | Banner fill on **light** fields. |
| `--field` | `#0B1220` | Preferred dark background. |
| `--field-2` | `#141C2E` | Dark field gradient stop. |
| `--silver` | `#9AA8BD` | Accent / edge lines. |

**How to theme:** set the SVG `color` property —
`color:#FFFFFF` on dark backgrounds, `color:#0B1220` on light backgrounds.

Keep to **one** foreground color plus the background; no extra palette.

## Typography (wordmark)

- Family: **Inter** (preferred), fallback `Work Sans`, `Segoe UI`,
  `Helvetica Neue`, Arial, sans-serif.
- Weight: 600. Letter-spacing: +1px.
- The wordmark is set in `currentColor` and follows the same theming as the
  icon.

## Clear space & minimum size

- **Clear space:** keep a margin around the lockup equal to the height of the
  banner icon (the square at the left). Nothing else should enter this zone.
- **Minimum size:** the icon must remain legible at **16×16 px** (favicon).
  Do not set the wordmark below ~18px tall in practice.

## Usage rules

**Do:**
- Keep the banner proportion intact (don't stretch/squash).
- Recolor only via `currentColor` (white on dark, ink on light).
- Maintain clear space.

**Don't:**
- Add crowns, shields, or any emblem to the banner (defeats the concept).
- Apply drop shadows, glows, or gradients to the mark.
- Rotate, skew, or outline the banner.
- Reorder icon/wordmark or change the gap arbitrarily.

## Licensing

- Logo artwork: released under **CC BY 4.0** (attribution: Seneschal project).
- Typeface: Inter is licensed under the **SIL Open Font License (OFL)**.
- The project code retains its own separate license.

## Generating raster exports

```bash
# favicon / social PNGs from the icon (requires resvg or rsvg-convert + icotool)
rsvg-convert -w 1024 -h 1024 assets/logo-icon.svg -o logo-icon-1024.png
rsvg-convert -w 512  -h 128  assets/logo.svg        -o logo-660x160.png
# optimize
svgo assets/logo.svg assets/logo-icon.svg assets/logo-wordmark.svg
```
