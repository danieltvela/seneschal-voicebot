/* Seneschal Learn — isometric scene engine + course UI.
   Own implementation (no third-party app code); lesson data © Seneschal project.
   Icon path data: subset of lucide (ISC). */
'use strict';

/* ---------- constants ---------- */
/* Projection — exact match of the original course: screenX = 14*(x+y),
   screenY = 7*(x-y) - z. This orientation makes the side faces ~82% of the
   diamond height, which is what gives the cubes their chunky look. */
const XU = 14;              // px per grid unit (x+y)
const YU = 7;               // px per grid unit (x-y)
const CUBE_H = 46;          // block height in px (original default: h:46)
const SLAB_H = 12;          // device/db slab height
const NS = 'http://www.w3.org/2000/svg';

/* Gradient pairs — Cormallen palette: silver structure, old-gold accent.
   Sides are darker than the tops so the faces read as 3D on the dark theme. */
const GRADS = {
  orange: { top: ['#f1e3c4', '#e4cf9d'], side: ['#cba96a', '#b3904f'] },
  green:  { top: ['#dfe9db', '#cddcc7'], side: ['#93ad8a', '#7c9673'] },
  blue:   { top: ['#dfe7f0', '#cbd9e8'], side: ['#93a9c2', '#7890ab'] },
  yellow: { top: ['#f3ecd4', '#e7dbb2'], side: ['#bfb37e', '#a89c62'] },
  purple: { top: ['#e7e0f0', '#d5c9e6'], side: ['#a393bd', '#8b7aa5'] },
  sand:   { top: ['#f2ede1', '#e9e1d0'], side: ['#d6c9b0', '#c0b294'] },
  slab:   { top: ['#f2ede1', '#e9e1d0'], side: ['#d6c9b0', '#c0b294'] },
  store:  { top: ['#ebe7f0', '#dcd6e4'], side: ['#b3abbe', '#9c92ab'] },
  violet: { top: ['#e5dcf2', '#d3c5e8'], side: ['#a08cc0', '#8873a8'] },
  slate:  { top: ['#d9dde3', '#c3c9d2'], side: ['#8f98a5', '#78828f'] },
  red:    { top: ['#f0d9d9', '#e4bcbc'], side: ['#cf8a8a', '#b97070'] },
  teal:   { top: ['#d6e6e2', '#bed8d2'], side: ['#83b3a7', '#6b9e91'] },
  amber:  { top: ['#ecd9ae', '#ddc488'], side: ['#c2a05c', '#a8894a'] },
  ink:    { top: ['#c6ccd6', '#aab2c0'], side: ['#6a7688', '#566273'] },
};
const INK = '#1f2430';          // label ink on pastel faces
const OUTLINE = '#334155';      // cube outline on dark theme

const KIND_COLORS = {
  req: '#9db4cf', res: '#9ccf8f', write: '#c9b06a', msg: '#d6bc82',
  kill: '#dd8a8a', ping: '#a8c0d8', heavy: '#d99a8a', poison: '#b3a1cc',
};

const CHAPTERS = { setup: 1, problem: 1, solution: 1, technique: 1, tradeoff: 1, insight: 1, recap: 1 };

const METRIC_META = {
  total:   { label: 'Total', icon: 'timer', fmt: v => v == null ? '—' : (v / 1000).toFixed(1).replace(/\.0$/, '') + ' s', level: v => v == null ? 'none' : v <= 200 ? 'good' : v <= 800 ? 'warn' : 'bad' },
  util:    { label: 'Utilization', icon: 'cpu', fmt: v => v == null ? '—' : Math.round(v) + '%', level: v => v == null ? 'none' : v < 75 ? 'good' : v < 88 ? 'warn' : 'bad' },
  backlog: { label: 'Backlog', icon: 'inbox', fmt: v => v == null ? '—' : v + ' jobs', level: () => 'none' },
  delay:   { label: 'Message delay', icon: 'clock', fmt: v => v == null ? '—' : String(v), level: v => v == null ? 'none' : v === '0 ms' ? 'good' : (typeof v === 'string' && /s$/.test(v) && !/ms$/.test(v)) ? 'bad' : 'good' },
  workers: { label: 'Workers', icon: 'server', fmt: v => v == null ? '—' : String(v), level: () => 'none' },
  lost:    { label: 'Lost jobs', icon: 'alert', fmt: v => v == null ? '—' : String(v), level: v => v == null ? 'none' : v === 0 ? 'good' : 'bad' },
  fresh:   { label: 'Data shown', icon: 'circleCheck', fmt: v => v == null ? '—' : String(v), level: v => v === 'fresh' ? 'good' : v === 'stale' ? 'bad' : 'none' },
  rows:    { label: 'Rows read', icon: 'database', fmt: v => v == null ? '—' : Number(v).toLocaleString('en-US'), level: () => 'none' },
  storage: { label: 'Storage / year', icon: 'database', fmt: v => v == null ? '—' : String(v), level: () => 'none' },
  eot:      { label: 'End-of-turn silence', icon: 'timer', fmt: v => v == null ? '—' : (v / 1000).toFixed(2).replace(/0$/, '') + ' s', level: v => v == null ? 'none' : v <= 200 ? 'good' : v <= 500 ? 'warn' : 'bad' },
  falseEnds:{ label: 'False splits', icon: 'split', fmt: v => v == null ? '—' : String(v), level: v => v == null ? 'none' : v === 0 ? 'good' : v <= 2 ? 'warn' : 'bad' },
  respond:  { label: 'Reply latency', icon: 'timer', fmt: v => v == null ? '—' : (v / 1000).toFixed(1).replace(/\.0$/, '') + ' s', level: v => v == null ? 'none' : v <= 300 ? 'good' : v <= 2000 ? 'warn' : 'bad' },
  pending:  { label: 'Pending announcements', icon: 'inbox', fmt: v => v == null ? '—' : String(v), level: v => v == null ? 'none' : v === 0 ? 'good' : v <= 2 ? 'warn' : 'bad' },
  locutions:{ label: 'Spoken fragments', icon: 'volume2', fmt: v => v == null ? '—' : String(v), level: v => v == null ? 'none' : v === 1 ? 'good' : v === 2 ? 'warn' : 'bad' },
  interval: { label: 'Capture interval', icon: 'clock', fmt: v => v == null ? '—' : String(v), level: v => v == null || v === 'off' ? 'none' : parseInt(v) <= 10 ? 'good' : 'warn' },
  shots:    { label: 'Screenshots', icon: 'eye', fmt: v => v == null ? '—' : Number(v).toLocaleString('en-US'), level: () => 'none' },
  rate:     { label: 'Sample rate', icon: 'activity', fmt: v => v == null ? '—' : String(v), level: () => 'none' },
  preroll:  { label: 'Pre-roll', icon: 'clock', fmt: v => v == null ? '—' : v + ' ms', level: () => 'none' },
  dropped:  { label: 'Dropped syllables', icon: 'alert', fmt: v => v == null ? '—' : String(v), level: v => v == null ? 'none' : v === 0 ? 'good' : 'bad' },
};

/* ---------- helpers ---------- */
const $ = sel => document.querySelector(sel);
function h(tag, attrs = {}, parent) {
  const n = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, v);
  if (parent) parent.appendChild(n);
  return n;
}
function icon(name, size = 14) {
  const paths = ICONS[name] || ICONS.box || [];
  const vb = 24, s = size / vb;
  let inner = '';
  for (const [tag, a] of paths) {
    const at = Object.entries(a).map(([k, v]) => `${k}="${v}"`).join(' ');
    inner += `<${tag} ${at}/>`;
  }
  return `<svg viewBox="0 0 ${vb} ${vb}" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="transform:scale(${s})">${inner}</svg>`;
}
function htmlToEl(html) { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstChild; }

/* iso projection: grid (x,y) → screen px */
const isoX = (x, y) => (x + y) * XU;
const isoY = (x, y, z = 0) => (x - y) * YU - z;

/* ---------- state ---------- */
let lessonId = COURSE.module.lessons[0].id;
let stepIdx = 0;            // 0-based
let paused = false;
let flowTimer = null;
let camAnim = null;
const progress = JSON.parse(localStorage.getItem('seneschal-learn-progress') || '{}');

/* ---------- routing ---------- */
function parseHash() {
  const m = location.hash.match(/^#\/([a-z-]+)\/(\d+)$/);
  if (m && COURSE.lessons[m[1]]) {
    lessonId = m[1];
    stepIdx = Math.min(Math.max(parseInt(m[2], 10) - 1, 0), COURSE.lessons[lessonId].steps.length - 1);
  }
}
function setHash() { location.hash = `#/${lessonId}/${stepIdx + 1}`; }

/* ---------- scene geometry ---------- */
function footprint(e) {
  if (e.type === 'device') return { d: e.d || 5, w: e.w || 5 };
  if (e.type === 'db') return { d: 3.4, w: 3.4 };
  if (e.type === 'ghost') return { d: e.s || 4, w: e.s || 4 };
  return { d: 4, w: 4 };  // block (original default: s:4)
}
function anchor(e) {
  if (e.type === 'zone') return { x: (e.x0 + e.x1) / 2, y: (e.y0 + e.y1) / 2, sx: isoX((e.x0 + e.x1) / 2, (e.y0 + e.y1) / 2), sy: isoY((e.x0 + e.x1) / 2, (e.y0 + e.y1) / 2) };
  const sx = isoX(e.x, e.y), sy = isoY(e.x, e.y);
  return { x: e.x, y: e.y, sx, sy };
}

function sceneBounds(scene) {
  let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
  const pad = 130; // room for labels/tags
  for (const e of Object.values(scene.entities || {})) {
    if (e.type === 'panel') continue;
    let pts = [];
    if (e.type === 'zone') {
      for (const [gx, gy] of [[e.x0, e.y0], [e.x1, e.y0], [e.x1, e.y1], [e.x0, e.y1]]) pts.push([isoX(gx, gy), isoY(gx, gy)]);
    } else {
      const f = footprint(e), a = anchor(e);
      const dx = (f.d + f.w) * XU / 2, dy = (f.d + f.w) * YU / 2;
      pts.push([a.sx - dx, a.sy], [a.sx + dx, a.sy], [a.sx, a.sy + dy], [a.sx, a.sy - dy]);
      pts.push([a.sx, a.sy - CUBE_H - 30]);
    }
    for (const [px, py] of pts) {
      minX = Math.min(minX, px); maxX = Math.max(maxX, px);
      minY = Math.min(minY, py); maxY = Math.max(maxY, py);
    }
  }
  return { minX: minX - pad, minY: minY - pad, maxX: maxX + pad, maxY: maxY + pad };
}

/* ---------- entity renderers (rounded iso-boxes, original style) ---------- */
/* rounded closed polygon — port of the original Sr(): per-corner radii via Q curves */
function roundedPath(pts, radii) {
  const n = pts.length;
  const rad = i => Array.isArray(radii) ? radii[i] : radii;
  const inFrom = (c, o, r) => {
    const dx = o[0] - c[0], dy = o[1] - c[1];
    const L = Math.hypot(dx, dy) || 1;
    const k = Math.min(r, L / 2) / L;
    return [c[0] + dx * k, c[1] + dy * k];
  };
  let d = '';
  for (let i = 0; i < n; i++) {
    const prev = pts[(i - 1 + n) % n], cur = pts[i], next = pts[(i + 1) % n];
    const a = inFrom(cur, prev, rad(i)), b = inFrom(cur, next, rad(i));
    d += `${i === 0 ? 'M' : 'L'}${a[0]},${a[1]} Q${cur[0]},${cur[1]} ${b[0]},${b[1]} `;
  }
  return d + 'Z';
}
/* rounded open path through points — port of Cr() */
function roundedCornerPath(pts, r = 10) {
  let d = `M${pts[0][0]},${pts[0][1]}`;
  for (let i = 1; i < pts.length - 1; i++) {
    const [x0, y0] = pts[i - 1], [x1, y1] = pts[i], [x2, y2] = pts[i + 1];
    const u = Math.hypot(x1 - x0, y1 - y0), v = Math.hypot(x2 - x1, y2 - y1);
    if (u < .01 || v < .01) continue;
    const f = Math.min(r, u / 2, v / 2);
    const p = [x1 + (x0 - x1) / u * f, y1 + (y0 - y1) / u * f];
    const q = [x1 + (x2 - x1) / v * f, y1 + (y2 - y1) / v * f];
    d += ` L${p[0]},${p[1]} Q${x1},${y1} ${q[0]},${q[1]}`;
  }
  const last = pts[pts.length - 1];
  return d + ` L${last[0]},${last[1]}`;
}
/* rounded iso-box geometry — port of Gr(): sil, top, sheen, right, bevel */
function makeBox(a, f, H, radius = 9) {
  const x0 = a.x - f.d / 2, x1 = a.x + f.d / 2, y0 = a.y - f.w / 2, y1 = a.y + f.w / 2;
  const T = [isoX(x0, y1), isoY(x0, y1, H)], R = [isoX(x1, y1), isoY(x1, y1, H)];
  const B = [isoX(x1, y0), isoY(x1, y0, H)], L = [isoX(x0, y0), isoY(x0, y0, H)];
  const Rd = [isoX(x1, y1), isoY(x1, y1)], Bd = [isoX(x1, y0), isoY(x1, y0)], Ld = [isoX(x0, y0), isoY(x0, y0)];
  const p = Math.max(1, Math.min(radius, H / 2 - .5));
  const mcy = isoY(a.x, a.y, H);
  return {
    ty: mcy, T, R, B, L, Rd, Bd, Ld,
    poly: [T, R, Rd, Bd, Ld, L],   // true silhouette hexagon (for link trimming)
    sil: roundedPath([T, R, Rd, Bd, Ld, L], [radius + 1, p, p, radius + 3, p, p]),
    top: roundedPath([T, R, B, L], [radius + 1, p, 6, p]),
    sheen: roundedPath([T, R, B, L].map(([x, y]) => [x, y - 3 + (y - mcy) * -.25]), [radius + 1, p, 6, p]),
    right: roundedPath([B, R, Rd, Bd], [6, p, p, radius + 3]),
    bevel: roundedCornerPath([L, B, R], 8),
  };
}
let _defsSvg = null;
function ensureDefs(svg) {
  if (_defsSvg === svg) return;
  const defs = svg.querySelector('defs');
  for (const [name, g] of Object.entries(GRADS)) {
    const top = h('linearGradient', { id: `g-${name}-top`, x1: 0, y1: 0, x2: .35, y2: 1 }, defs);
    h('stop', { offset: 0, 'stop-color': g.top[0] }, top);
    h('stop', { offset: 1, 'stop-color': g.top[1] }, top);
    const side = h('linearGradient', { id: `g-${name}-side`, x1: 0, y1: 0, x2: 0, y2: 1 }, defs);
    h('stop', { offset: 0, 'stop-color': g.side[0] }, side);
    h('stop', { offset: 1, 'stop-color': g.side[1] }, side);
  }
  const blurSm = h('filter', { id: 'blur-sm', x: '-20%', y: '-20%', width: '140%', height: '140%' }, defs);
  h('feGaussianBlur', { stdDeviation: 1.6 }, blurSm);
  const blurLg = h('filter', { id: 'blur-lg', x: '-30%', y: '-30%', width: '160%', height: '160%' }, defs);
  h('feGaussianBlur', { stdDeviation: 6 }, blurLg);
  _defsSvg = svg;
}
/* face painting — port of Kr() */
function paintBox(g, box, color, { stroke = OUTLINE, sw = 2.2 } = {}) {
  h('path', { d: box.sil, fill: `url(#g-${color}-side)` }, g);
  h('path', { d: box.right, fill: '#3a2a14', opacity: .12 }, g);
  h('path', { d: box.top, fill: `url(#g-${color}-top)` }, g);
  h('path', { d: box.sheen, fill: '#fff', opacity: .13, filter: 'url(#blur-sm)' }, g);
  h('path', { d: box.bevel, transform: 'translate(0 1.4)', fill: 'none', stroke: '#fff', 'stroke-opacity': .75, 'stroke-width': 1.6, 'stroke-linecap': 'round' }, g);
  h('path', { d: box.bevel, transform: 'translate(0 3.4)', fill: 'none', stroke: '#5a4123', 'stroke-opacity': .07, 'stroke-width': 2.4, 'stroke-linecap': 'round' }, g);
  h('path', { d: box.sil, fill: 'none', stroke, 'stroke-width': sw, 'stroke-linejoin': 'round' }, g);
}
/* soft ground shadow — port of qr() */
function paintShadow(g, box, H, opacity = .26) {
  const off = Math.min(9, 3 + H * .15);
  h('path', { d: box.sil, transform: `translate(0 ${off})`, fill: '#05070a', opacity, filter: 'url(#blur-lg)' }, g);
}

function renderIconAt(g, name, cx, cy, size, color) {
  const paths = ICONS[name] || [];
  const ig = h('g', { transform: `translate(${cx - size / 2} ${cy - size / 2}) scale(${size / 24})`, stroke: color || 'rgba(15,23,42,.85)', fill: 'none', 'stroke-width': 2, 'stroke-linecap': 'round', 'stroke-linejoin': 'round' }, g);
  for (const [tag, at] of paths) h(tag, at, ig);
}

function renderTagPills(g, tags, cx, yTop) {
  if (!tags || !tags.length) return yTop;
  let total = 0;
  const widths = tags.map(t => {
    const w = 18 + (t.text ? t.text.length * 6.3 : 0) + (t.icon ? 14 : 0);
    total += w + 6; return w;
  });
  let x = cx - total / 2 + 3;
  const y = yTop;
  tags.forEach((t, i) => {
    const w = widths[i];
    const tone = { red: '#dd8a8a', amber: '#d6bc82', green: '#9ccf8f' }[t.tone] || '#7d786e';
    h('rect', { x, y, width: w, height: 20, rx: 10, class: 'tag-pill-bg', stroke: tone, 'stroke-width': 1.2 }, g);
    let tx = x + 10;
    if (t.icon) { renderIconAt(g, t.icon, x + 15, y + 10, 11, tone); tx = x + 24; }
    if (t.text) {
      const txt = h('text', { x: tx + (w - (tx - x) - 8) / 2, y: y + 14, class: 'tag-pill-text' }, g);
      txt.textContent = t.text;
    }
    x += w + 6;
  });
  return y + 26;
}

function renderBlock(g, id, e) {
  const a = anchor(e), f = footprint(e);
  const grp = h('g', { class: `ent ent--${id}` }, g);
  const box = makeBox(a, f, CUBE_H, e.radius ?? 9);
  paintShadow(grp, box, CUBE_H);
  paintBox(grp, box, e.color || 'blue');
  // icon + label + sub laid out like the original (labels sit on the front faces)
  const o = box.ty, cx = a.sx;
  const lines = Array.isArray(e.label) ? e.label : [e.label || ''];
  const l = o + (f.d * 14 * .5 + CUBE_H) * .4;
  const scale = f.d >= 6 ? 1.15 : f.d < 3.5 ? .8 : 1;
  if (e.glyph) renderIconAt(grp, e.glyph, cx, lines.length ? o - 1 : o + 2, 24 * scale, INK);
  const labelSize = 13.5, subSize = 10;
  const yFor = (i) => {
    if (lines.length === 1) return e.sub ? [l, l + 14] : [l + 4.5];
    if (lines.length === 2) return e.sub ? [l - 9, l + 6.5, l + 20.5] : [l - 3.5, l + 12];
    return [l - 3.5 + i * 15];
  };
  lines.forEach((line, i) => {
    const t = h('text', { x: cx, y: yFor(i)[i], class: 'ent-label', 'font-size': labelSize, 'font-weight': 800 }, grp);
    t.textContent = line;
  });
  if (e.sub) {
    const t = h('text', { x: cx, y: lines.length === 1 ? l + 14 : (lines.length === 2 ? l + 20.5 : l - 3.5 + lines.length * 15), class: 'ent-sub', 'font-size': subSize, 'font-weight': 600 }, grp);
    t.textContent = e.sub;
  }
  const bottomY = a.sy + f.w * YU + 8;
  renderTagPills(grp, e.tags, a.sx, bottomY);
  return grp;
}

function renderDevice(g, id, e) {
  const a = anchor(e), f = footprint(e);
  const grp = h('g', { class: `ent ent--${id}` }, g);
  // rounded slab (original: Gr with h:16, radius:9)
  const box = makeBox(a, f, 16, 9);
  paintShadow(grp, box, 16, .2);
  paintBox(grp, box, 'sand');
  // standing phone with the original screen gradient feel
  const phW = 34, phH = 62, phX = a.sx - phW / 2, phY = a.sy - 16 - phH - 8;
  h('rect', { x: phX, y: phY, width: phW, height: phH, rx: 7, fill: '#0f172a', stroke: OUTLINE, 'stroke-width': 1.5 }, grp);
  const scr = h('linearGradient', { id: 'screen-grad', x1: 0, y1: 0, x2: 1, y2: 1 }, grp.ownerSVGElement.querySelector('defs'));
  h('stop', { offset: 0, 'stop-color': '#DCE3EC' }, scr);
  h('stop', { offset: 1, 'stop-color': '#C0CCDA' }, scr);
  h('rect', { x: phX + 4, y: phY + 9, width: phW - 8, height: phH - 20, rx: 3, fill: 'url(#screen-grad)' }, grp);
  h('circle', { cx: a.sx, cy: phY + phH - 6, r: 2, fill: '#475569' }, grp);
  // "you" pill below
  const label = e.label || 'you';
  const pw = 16 + label.length * 7 + 16;
  const py = a.sy + f.w * YU + 10;
  h('rect', { x: a.sx - pw / 2, y: py, width: pw, height: 22, rx: 11, class: 'tag-pill-bg' }, grp);
  renderIconAt(grp, 'user', a.sx - pw / 2 + 13, py + 11, 12, '#94a3b8');
  const t = h('text', { x: a.sx - pw / 2 + 24 + (pw - 40) / 2, y: py + 15, class: 'tag-pill-text' }, grp);
  t.textContent = label;
  renderTagPills(grp, e.tags, a.sx, phY - 30);
  return grp;
}

function renderDb(g, id, e) {
  const a = anchor(e), f = footprint(e);
  const grp = h('g', { class: `ent ent--${id}` }, g);
  const dy = f.w * YU;
  const bodyH = 44, topY = a.sy - bodyH - dy;
  const rx = (f.d + f.w) * XU * .31, ry = rx * .45;
  // body with the blue gradient + ink outline (original palette feel)
  h('path', { d: `M ${a.sx - rx} ${topY} V ${a.sy - dy} A ${rx} ${ry} 0 0 0 ${a.sx + rx} ${a.sy - dy} V ${topY}`, fill: 'url(#g-blue-side)', stroke: 'none' }, grp);
  h('ellipse', { cx: a.sx, cy: topY, rx, ry, fill: 'url(#g-blue-top)', stroke: OUTLINE, 'stroke-width': 1.6 }, grp);
  h('path', { d: `M ${a.sx - rx} ${topY + bodyH * .35} A ${rx} ${ry} 0 0 0 ${a.sx + rx} ${topY + bodyH * .35}`, fill: 'none', stroke: 'rgba(31,36,48,.25)' }, grp);
  h('path', { d: `M ${a.sx - rx} ${topY} V ${a.sy - dy} A ${rx} ${ry} 0 0 0 ${a.sx + rx} ${a.sy - dy} V ${topY}`, fill: 'none', stroke: OUTLINE, 'stroke-width': 1.6 }, grp);
  if (e.label) {
    const t = h('text', { x: a.sx, y: a.sy - dy + 14, class: 'ent-sub', 'font-size': 11, fill: '#e2e8f0' }, grp);
    t.textContent = e.label;
  }
  renderTagPills(grp, e.tags, a.sx, a.sy + dy / 2 + 6);
  return grp;
}

function renderZone(g, id, e) {
  const grp = h('g', { class: `ent ent--${id}` }, g);
  // flat rounded tile (original: Gr h:10 radius:8)
  const c = { x: (e.x0 + e.x1) / 2, y: (e.y0 + e.y1) / 2, sx: 0, sy: 0 };
  c.sx = isoX(c.x, c.y); c.sy = isoY(c.x, c.y);
  const f = { d: (e.x1 - e.x0), w: (e.y1 - e.y0) };
  const box = makeBox(c, f, 10, 8);
  h('path', { d: box.sil, fill: 'rgba(148,163,184,.06)', stroke: 'none' }, grp);
  h('path', { d: box.sil, fill: 'none', stroke: '#475569', 'stroke-width': 1.4, 'stroke-dasharray': '6 4' }, grp);
  const t = h('text', { x: box.L[0] + 8, y: box.L[1] + 4, class: 'zone-label', 'text-anchor': 'start' }, grp);
  t.textContent = e.label || '';
  return grp;
}

function renderGhost(g, id, e) {
  const a = anchor(e);
  const grp = h('g', { class: `ent ent--${id}` }, g);
  const w = (e.s || 6) * XU * .85, bh = Math.max(e.h || 90, 30);
  h('rect', { x: a.sx - w / 2, y: a.sy - bh - 14, width: w, height: bh, rx: 9, class: 'ghost-box' }, grp);
  const label = e.label || '';
  const t = h('text', { x: a.sx, y: a.sy + 6, class: 'ghost-label' }, grp);
  t.textContent = label;
  return grp;
}

/* ---------- links & flows ---------- */
function entityBBox(e) {
  const a = anchor(e), f = footprint(e);
  const hw = (f.d + f.w) * XU / 2, dh = (f.d + f.w) * YU / 2;
  let top, bottom;
  if (e.type === 'block') { top = a.sy - CUBE_H - dh; bottom = a.sy + dh; }
  else if (e.type === 'device') { top = a.sy - 16 - 90; bottom = a.sy + dh; }
  else if (e.type === 'db') { top = a.sy - 44 - 2 * dh - 12; bottom = a.sy + dh; }
  else if (e.type === 'ghost') { top = a.sy - Math.max(e.h || 90, 30) - 20; bottom = a.sy + 10; }
  else if (e.type === 'zone') { top = isoY(e.x1, e.y0); bottom = isoY(e.x0, e.y1); return { cx: a.sx, cy: (top + bottom) / 2, x0: isoX(e.x0, e.y0), x1: isoX(e.x1, e.y1), top, bottom }; }
  else return null;
  return { cx: a.sx, cy: a.sy, x0: a.sx - hw, x1: a.sx + hw, top, bottom };
}
void entityBBox;
/* trim link endpoints against the entity's REAL polygon (port of the original
   wr/Tr approach). Bounding boxes break on diagonally-staggered cubes, whose
   diamonds overlap horizontally — the polygon doesn't. */
function pointInPoly(x, y, poly) {
  let sign = 0;
  for (let i = 0; i < poly.length; i++) {
    const [ax, ay] = poly[i], [bx, by] = poly[(i + 1) % poly.length];
    const cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
    if (Math.abs(cross) < 1e-9) continue;
    const s = Math.sign(cross);
    if (sign === 0) sign = s; else if (s !== sign) return false;
  }
  return true;
}
function exitPoint(poly, cx, cy, tx, ty, pad = 3) {
  if (!poly || poly.length < 3) return [tx, ty];
  const dx = tx - cx, dy = ty - cy;
  const L = Math.hypot(dx, dy) || 1;
  const ux = dx / L, uy = dy / L;
  if (pointInPoly(tx, ty, poly)) return [tx, ty];   // target inside: don't trim
  let lo = 0, hi = L;
  for (let i = 0; i < 24; i++) {
    const mid = (lo + hi) / 2;
    if (pointInPoly(cx + ux * mid, cy + uy * mid, poly)) lo = mid; else hi = mid;
  }
  return [cx + ux * (hi + pad), cy + uy * (hi + pad)];
}
function entityPoly(e) {
  const a = anchor(e), f = footprint(e);
  if (e.type === 'block') return makeBox(a, f, CUBE_H, e.radius ?? 9).poly;
  if (e.type === 'device') return makeBox(a, f, 16, 9).poly;
  if (e.type === 'db') {
    const dy = f.w * YU, topY = a.sy - 44 - dy;
    const rx = (f.d + f.w) * XU * .31;
    return [[a.sx - rx, topY - 8], [a.sx + rx, topY - 8], [a.sx + rx, a.sy - dy], [a.sx - rx, a.sy - dy]];
  }
  if (e.type === 'ghost') {
    const w = (e.s || 6) * XU * .85, bh = Math.max(e.h || 90, 30);
    return [[a.sx - w / 2, a.sy - bh - 20], [a.sx + w / 2, a.sy - bh - 20], [a.sx + w / 2, a.sy + 10], [a.sx - w / 2, a.sy + 10]];
  }
  if (e.type === 'zone') {
    return [[isoX(e.x0, e.y0), isoY(e.x0, e.y0)], [isoX(e.x1, e.y0), isoY(e.x1, e.y0)], [isoX(e.x1, e.y1), isoY(e.x1, e.y1)], [isoX(e.x0, e.y1), isoY(e.x0, e.y1)]];
  }
  return null;
}
function linkPoints(scene, spec) {
  const ents = scene.entities;
  const ids = [spec.from, ...(spec.pts || []).filter(p => p !== spec.from && p !== spec.to), spec.to];
  const nodes = ids.map(id => ents[id]).filter(e => e && e.type !== 'panel');
  const centers = nodes.map(e => { const a = anchor(e); return [a.sx, a.sy]; });
  if (centers.length < 2) return [];
  const out = [];
  for (let i = 0; i < centers.length; i++) {
    const poly = entityPoly(nodes[i]);
    const next = centers[i + 1], prev = centers[i - 1];
    let p = centers[i];
    if (i === 0 && next) p = exitPoint(poly, centers[i][0], centers[i][1], next[0], next[1]);
    else if (i === centers.length - 1 && prev) p = exitPoint(poly, centers[i][0], centers[i][1], prev[0], prev[1]);
    out.push(p);
  }
  return out;
}

function renderLinks(g, scene) {
  const defs = g.ownerSVGElement.querySelector('defs') || h('defs', {}, g.ownerSVGElement);
  const groups = {};
  for (const [id, spec] of Object.entries(scene.links || {})) {
    const pts = linkPoints(scene, spec);
    if (pts.length < 2) continue;
    const d = pts.map((p, i) => (i ? 'L' : 'M') + p[0] + ' ' + p[1]).join(' ');
    const color = '#94a3b8';
    const mkId = 'arrow-' + id;
    const mk = h('marker', { id: mkId, markerWidth: 8, markerHeight: 8, refX: 6, refY: 4, orient: 'auto' }, defs);
    h('path', { d: 'M0,0 L7,4 L0,8 Z', fill: '#94a3b8' }, mk);
    const attrs = { d, class: 'link-line' + (spec.style ? ' style-' + spec.style : '') };
    if (spec.heads !== 'none') attrs['marker-end'] = `url(#${mkId})`;
    h('path', attrs, g);
    if (spec.heads === 'both') {
      const mk2 = h('marker', { id: mkId + '-s', markerWidth: 8, markerHeight: 8, refX: 1, refY: 4, orient: 'auto' }, defs);
      h('path', { d: 'M7,0 L0,4 L7,8 Z', fill: '#94a3b8' }, mk2);
      g.lastChild.setAttribute('marker-start', `url(#${mkId}-s)`);
    }
    if (spec.label) {
      const mid = pts[Math.floor(pts.length / 2) - (pts.length % 2 === 0 ? 1 : 0)] || pts[0];
      const mi = Math.max(0, Math.floor((pts.length - 1) / 2));
      const [mx, my] = pts.length === 2 ? [(pts[0][0] + pts[1][0]) / 2, (pts[0][1] + pts[1][1]) / 2] : pts[mi];
      const w = spec.label.length * 6.4 + 14;
      h('rect', { x: mx - w / 2, y: my - 9, width: w, height: 18, rx: 9, class: 'link-label-bg' }, g);
      const t = h('text', { x: mx, y: my + 4, class: 'link-label-text' }, g);
      t.textContent = spec.label;
    }
  }
}

/* ---------- flows engine ---------- */
function startFlows(svg, scene) {
  stopFlows();
  if (paused) return;
  const layer = svg.querySelector('.flows');
  if (!layer) return;
  const packets = [];
  const pairMap = {};   // "fromId>toId" → measurable path (flows route by entity ids)
  for (const [id, spec] of Object.entries(scene.links || {})) {
    const pts = linkPoints(scene, spec);
    if (pts.length < 2) continue;
    const p = h('path', { d: pts.map((pt, i) => (i ? 'L' : 'M') + pt[0] + ' ' + pt[1]).join(' '), fill: 'none', stroke: 'none' }, layer);
    pairMap[`${spec.from}>${spec.to}`] = { el: p, len: p.getTotalLength() };
  }
  const flowState = (scene.flows || []).map(f => ({ ...f, acc: 0, n: 0, rr: 0 }));
  let last = performance.now();
  function tick(now) {
    const dt = Math.min(.05, (now - last) / 1000); last = now;
    if (!paused) {
      for (const f of flowState) {
        const rate = f.rate || 1;
        f.acc += dt * rate;
        while (f.acc >= 1) {
          f.acc -= 1;
          const route = f.route || [];
          const kind = f.pattern ? f.pattern[f.n % f.pattern.length] : (f.kind || 'req');
          f.n++;
          const segs = [];
          let ok = route.length >= 2;
          for (let i = 0; i + 1 < route.length && ok; i++) {
            const seg = pairMap[`${route[i]}>${route[i + 1]}`];
            if (seg) segs.push(seg); else ok = false;
          }
          if (!ok) continue;
          const color = KIND_COLORS[kind] || '#94a3b8';
          const isSquare = kind === 'write' || kind === 'msg' || kind === 'poison';
          const el = isSquare
            ? h('rect', { x: -4, y: -4, width: 8, height: 8, rx: 2, fill: color }, layer)
            : h('circle', { r: 4, fill: color }, layer);
          packets.push({ el, segs, seg: 0, d: 0, back: false, speed: f.speed || 1, bounce: !!f.bounce });
        }
      }
      for (let i = packets.length - 1; i >= 0; i--) {
        const p = packets[i];
        const seg = p.segs[p.seg];
        if (!seg) { p.el.remove(); packets.splice(i, 1); continue; }
        p.d += 95 * dt * p.speed;
        const trim = Math.min(10, seg.len * .3);
        if (p.d >= seg.len - trim) {
          if (!p.back && p.seg < p.segs.length - 1) { p.seg++; p.d = 0; continue; }
          if (p.bounce && !p.back) { p.back = true; p.d = 0; continue; }
          if (p.back && p.seg > 0) { p.seg--; p.d = 0; continue; }
          p.el.remove(); packets.splice(i, 1); continue;
        }
        const dist = p.back ? seg.len - trim - p.d : p.d;
        const pt = seg.el.getPointAtLength(Math.max(0, Math.min(seg.len, dist)));
        p.el.setAttribute('transform', `translate(${pt.x} ${pt.y})`);
      }
    }
    flowTimer = requestAnimationFrame(tick);
  }
  flowTimer = requestAnimationFrame(tick);
}
function stopFlows() { if (flowTimer) { cancelAnimationFrame(flowTimer); flowTimer = null; } }

/* ---------- camera ---------- */
function fitView(svg, bounds, animate = true) {
  const rect = svg.getBoundingClientRect();
  const vw = Math.max(rect.width, 100), vh = Math.max(rect.height, 100);
  const bw = bounds.maxX - bounds.minX, bh = bounds.maxY - bounds.minY;
  const scale = Math.min(vw / bw, vh / bh, 1.15);
  const w = vw / scale, hgt = vh / scale;
  const cx = (bounds.minX + bounds.maxX) / 2 - w / 2;
  const cy = (bounds.minY + bounds.maxY) / 2 - hgt / 2;
  const target = { x: cx, y: cy, w, h: hgt };
  if (!animate) { svg.setAttribute('viewBox', `${target.x} ${target.y} ${target.w} ${target.h}`); return; }
  const from = svg.viewBox.baseVal;
  const start = { x: from.x, y: from.y, w: from.width, h: from.height };
  if (start.w === 0) { svg.setAttribute('viewBox', `${target.x} ${target.y} ${target.w} ${target.h}`); return; }
  const t0 = performance.now(), dur = 550;
  if (camAnim) cancelAnimationFrame(camAnim);
  function step(now) {
    const k = Math.min(1, (now - t0) / dur);
    const e = k < .5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2;
    const v = { x: start.x + (target.x - start.x) * e, y: start.y + (target.y - start.y) * e, w: start.w + (target.w - start.w) * e, h: start.h + (target.h - start.h) * e };
    svg.setAttribute('viewBox', `${v.x} ${v.y} ${v.w} ${v.h}`);
    if (k < 1) camAnim = requestAnimationFrame(step);
  }
  camAnim = requestAnimationFrame(step);
}

/* ---------- scene render ---------- */
function renderScene(stage, scene) {
  stage.querySelectorAll('svg.diagram, .scene-card').forEach(n => n.remove());
  const svg = h('svg', { class: 'diagram', preserveAspectRatio: 'xMidYMid meet' });
  h('defs', {}, svg);
  stage.appendChild(svg);
  // background grid
  const rect = h('rect', { x: -5000, y: -5000, width: 10000, height: 10000, fill: 'url(#grid)' }, svg);
  const defs = svg.querySelector('defs');
  const pat = h('pattern', { id: 'grid', width: 44, height: 22, patternUnits: 'userSpaceOnUse' }, defs);
  h('path', { d: 'M0 11 L22 0 L44 11 L22 22 Z', fill: 'none', stroke: 'rgba(51,65,85,.35)', 'stroke-width': 1 }, pat);
  ensureDefs(svg);

  const ents = scene.entities || {};
  const layerZone = h('g', {}, svg), layerLink = h('g', {}, svg), layerEnt = h('g', {}, svg), layerFlows = h('g', { class: 'flows' }, svg);
  for (const [id, e] of Object.entries(ents)) {
    if (e.type === 'zone') renderZone(layerZone, id, e);
  }
  renderLinks(layerLink, scene);
  for (const [id, e] of Object.entries(ents)) {
    if (e.type === 'block') renderBlock(layerEnt, id, e);
    else if (e.type === 'device') renderDevice(layerEnt, id, e);
    else if (e.type === 'db') renderDb(layerEnt, id, e);
    else if (e.type === 'ghost') renderGhost(layerEnt, id, e);
  }
  // entrance animation
  layerEnt.setAttribute('opacity', '0');
  requestAnimationFrame(() => {
    layerEnt.style.transition = 'opacity .45s ease';
    layerEnt.setAttribute('opacity', '1');
  });

  // bounds excluding the card, with extra left bias when a card exists
  let bounds = sceneBounds(scene);
  const hasCard = Object.values(ents).some(e => e.type === 'panel');
  if (hasCard) bounds.minX -= 420;
  fitView(svg, bounds, true);
  startFlows(svg, scene);

  // panel card
  const card = Object.values(ents).find(e => e.type === 'panel');
  if (card) {
    const div = htmlToEl(`<div class="scene-card"><h3>${icon(card.icon || 'info', 13)}<span>${card.title}</span></h3><pre></pre></div>`);
    const pre = div.querySelector('pre');
    pre.innerHTML = (card.lines || []).map(l => l.startsWith('#') ? `<span class="comment">${l.slice(1).trim()}</span>` : l).join('\n');
    stage.appendChild(div);
    // pick the corner with the least scene overlap
    const sr = stage.getBoundingClientRect();
    const b2s = p => [(p.x - svg.viewBox.baseVal.x) / svg.viewBox.baseVal.width * sr.width, (p.y - svg.viewBox.baseVal.y) / svg.viewBox.baseVal.height * sr.height];
    const [bx0, by0] = b2s({ x: bounds.minX, y: bounds.minY });
    const [bx1, by1] = b2s({ x: bounds.maxX, y: bounds.maxY });
    const cardW = 330, cardH = 240, m = 14;
    const corners = {
      tl: Math.max(0, Math.min(bx1, cardW + m) - Math.max(bx0, m)) * Math.max(0, Math.min(by1, cardH + m) - Math.max(by0, m)),
      tr: Math.max(0, Math.min(bx1, sr.width - m) - Math.max(bx0, sr.width - m - cardW)) * Math.max(0, Math.min(by1, cardH + m) - Math.max(by0, m)),
      bl: Math.max(0, Math.min(bx1, cardW + m) - Math.max(bx0, m)) * Math.max(0, Math.min(by1, sr.height - m) - Math.max(by0, sr.height - m - cardH)),
      br: Math.max(0, Math.min(bx1, sr.width - m) - Math.max(bx0, sr.width - m - cardW)) * Math.max(0, Math.min(by1, sr.height - m) - Math.max(by0, sr.height - m - cardH)),
    };
    const best = Object.entries(corners).sort((a, b) => a[1] - b[1])[0][0];
    div.style[best.startsWith('t') ? 'top' : 'bottom'] = m + 'px';
    div.style[best.endsWith('l') ? 'left' : 'right'] = m + 'px';
  }
}

/* ---------- UI render ---------- */
function renderSidebar() {
  const sb = $('#sidebar');
  const doneCount = COURSE.module.lessons.filter(l => (progress[l.id] || 0) >= COURSE.lessons[l.id].steps.length).length;
  sb.innerHTML = `<h2>${icon('book', 14)} Course · ${COURSE.module.lessons.length} lessons</h2>`;
  for (const l of COURSE.module.lessons) {
    const done = (progress[l.id] || 0) >= COURSE.lessons[l.id].steps.length;
    const a = htmlToEl(`<a class="lesson-item ${l.id === lessonId ? 'active' : ''}" href="#/${l.id}/1"><span class="${done ? 'done' : ''}">${done ? '✓ ' : ''}${l.title}</span></a>`);
    sb.appendChild(a);
  }
  const totalSteps = COURSE.module.lessons.reduce((a, l) => a + COURSE.lessons[l.id].steps.length, 0);
  const doneSteps = COURSE.module.lessons.reduce((a, l) => a + Math.min(progress[l.id] || 0, COURSE.lessons[l.id].steps.length), 0);
  sb.appendChild(htmlToEl(`<div class="progress-line"><span>Progress</span><span>${doneSteps} / ${totalSteps} steps</span></div>`));
}

function renderGuide(lesson, step) {
  const g = $('#guide');
  const goalsHtml = (lesson.goals || []).map(goal => {
    const hit = stepIdx + 1 >= goal.step;
    return `<li class="${hit ? 'hit' : ''}">${icon(hit ? 'circleCheck' : 'check', 14)}<span>${goal.text}</span></li>`;
  }).join('');
  const callout = step.callout ? `<div class="callout"><div class="co-label">${icon('bulb', 12)} Key idea</div>${step.callout.text}</div>` : '';
  const body = (step.body || []).map(p => `<p>${p}</p>`).join('');
  g.innerHTML = `
    <span class="chapter-chip ch-${step.chapter}">${step.chapter}</span>
    <h2>${step.title}</h2>
    <div class="body">${body}</div>
    ${callout}
    <h4>You'll learn</h4>
    <ul class="goals">${goalsHtml}</ul>
    <div class="nav-btns">
      <button id="g-prev" ${stepIdx === 0 ? 'disabled' : ''}>${icon('arrowLeft', 13)} Back</button>
      ${stepIdx < lesson.steps.length - 1
        ? `<button id="g-next" class="primary">Next: ${lesson.steps[stepIdx + 1].short} ${icon('arrowRight', 13)}</button>`
        : `<button id="g-next" class="primary">Finish ${icon('circleCheck', 13)}</button>`}
    </div>`;
  $('#g-prev').onclick = () => nav(-1);
  $('#g-next').onclick = () => nav(1);
}

function renderMetrics(lesson, step) {
  // METRIC_META keys not present fall back to a generic chip
  const row = $('#metric-row');
  const prev = row._values || {};
  row.innerHTML = '';
  for (const key of lesson.metrics || []) {
    const meta = METRIC_META[key] || { label: key, icon: 'activity', fmt: v => String(v), level: () => 'none' };
    const v = (step.metrics || {})[key];
    const lvl = meta.level(v);
    const changed = prev[key] !== undefined && prev[key] !== v;
    const div = htmlToEl(`<div class="metric level-${lvl} ${changed ? 'is-changed' : ''}">${icon(meta.icon, 13)}<span class="m-label">${meta.label}</span><span class="m-value">${meta.fmt(v)}</span></div>`);
    row.appendChild(div);
  }
  row._values = { ...prev, ...(step.metrics || {}) };
}

function renderLegend(lesson) {
  const lg = $('#legend');
  lg.innerHTML = '';
  for (const entry of lesson.legend || []) {
    const [kind, label] = Array.isArray(entry) ? entry : [entry, entry];
    lg.appendChild(htmlToEl(`<span><span class="lg-dot" style="background:${KIND_COLORS[kind] || '#94a3b8'}"></span>${label}</span>`));
  }
  lg.appendChild(htmlToEl(`<span class="spacer"></span>`));
  const replay = htmlToEl(`<button title="Replay animations">${icon('replay', 11)} Replay</button>`);
  const pause = htmlToEl(`<button title="Pause flows">${icon(paused ? 'play' : 'pause', 11)} ${paused ? 'Play' : 'Pause'}</button>`);
  replay.onclick = () => { const st = currentStep(); renderScene($('#stage'), st.scene); };
  pause.onclick = () => { paused = !paused; renderLegend(lesson); };
  lg.appendChild(replay); lg.appendChild(pause);
}

function renderStepper(lesson) {
  const st = $('#stepper');
  st.innerHTML = '';
  lesson.steps.forEach((s, i) => {
    if (i > 0) st.appendChild(htmlToEl('<div class="step-line"></div>'));
    const cls = i === stepIdx ? 'current' : (i < stepIdx || i < (progress[lessonId] || 0) ? 'visited' : '');
    const b = htmlToEl(`<button class="step-dot ${cls}"><span class="dot">${i + 1}</span><span class="s-label">${s.short}</span></button>`);
    b.onclick = () => { stepIdx = i; setHash(); render(); };
    st.appendChild(b);
  });
}

function currentStep() { return COURSE.lessons[lessonId].steps[stepIdx]; }

function render() {
  const lesson = COURSE.lessons[lessonId];
  const step = currentStep();
  renderSidebar();
  $('#main').innerHTML = `
    <div class="lesson-head">
      <div class="crumb">${COURSE.module.title} · Lesson ${COURSE.module.lessons.findIndex(l => l.id === lessonId) + 1} of ${COURSE.module.lessons.length}</div>
      <h1>${lesson.title}</h1>
      <div class="head-chips">
        <span class="chip">${icon('clock', 11)} ${lesson.minutes} min</span>
        <span class="chip">${icon('signal', 11)} ${lesson.level}</span>
        <span class="chip">${icon('listOrdered', 11)} ${lesson.steps.length} steps</span>
      </div>
    </div>
    <div class="metric-row" id="metric-row"></div>
    <div class="stage" id="stage"></div>
    <div class="legend" id="legend"></div>
    <div class="stepper" id="stepper"></div>`;
  renderMetrics(lesson, step);
  renderLegend(lesson);
  renderStepper(lesson);
  renderGuide(lesson, step);
  renderScene($('#stage'), step.scene);
  // progress
  progress[lessonId] = Math.max(progress[lessonId] || 0, stepIdx + 1);
  localStorage.setItem('seneschal-learn-progress', JSON.stringify(progress));
}

function nav(dir) {
  const lesson = COURSE.lessons[lessonId];
  const next = stepIdx + dir;
  if (next < 0) {
    const idx = COURSE.module.lessons.findIndex(l => l.id === lessonId);
    if (idx > 0) { lessonId = COURSE.module.lessons[idx - 1].id; stepIdx = COURSE.lessons[lessonId].steps.length - 1; setHash(); render(); }
    return;
  }
  if (next >= lesson.steps.length) {
    const idx = COURSE.module.lessons.findIndex(l => l.id === lessonId);
    if (idx < COURSE.module.lessons.length - 1) { lessonId = COURSE.module.lessons[idx + 1].id; stepIdx = 0; setHash(); render(); }
    return;
  }
  stepIdx = next; setHash(); render();
}

window.addEventListener('hashchange', () => { parseHash(); render(); });
window.addEventListener('keydown', e => {
  if (e.key === 'ArrowRight' || e.key === ' ') { e.preventDefault(); nav(1); }
  else if (e.key === 'ArrowLeft') { e.preventDefault(); nav(-1); }
});
window.addEventListener('resize', () => {
  const svg = document.querySelector('svg.diagram');
  if (svg) fitView(svg, sceneBounds(currentStep().scene), false);
});

parseHash();
render();
