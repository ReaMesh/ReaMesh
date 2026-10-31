// The peer cursor glyphs of the in-REAPER arrange layer: the shapes the layer
// draws for what someone is doing (trimming an item edge, dragging a fade, a
// razor edit, ...), sampled to polygons with the same number of points each so
// the layer can morph from one to the next point by point.
//
//   bun tools/cursor_glyphs.ts      writes extension/src/cursor_glyphs.inc
//
// Every glyph is drawn around its hotspot at (0,0), in points, y down. A glyph
// is a list of layers; the shapes of one layer read as one silhouette (all
// their white outlines first, then all their fills), a later layer sits on
// top with its own white edge. A `tint` layer is a translucent fill with a
// thin edge, an `ink` layer plain white on top (a slot, a dot).

type Pt = [number, number];
type Shape = { d: string; rotate?: number; line?: number };
type Layer = { shapes: Shape[]; tint?: boolean; ink?: boolean };

const POINTS = 96;  // per part; must match kPoints in cursor_glyphs.h
const UNIT = 32;    // stored as int16 in 1/32 pt

// ---------- shape helpers (as in the design sheet) ----------

function roundPoly(pts: Pt[], radii: number[], dx = 0, dy = 0): string {
  const n = pts.length;
  let d = "";
  for (let i = 0; i < n; i++) {
    const P = pts[i], A = pts[(i + n - 1) % n], B = pts[(i + 1) % n], r = radii[i];
    const u1: Pt = [A[0] - P[0], A[1] - P[1]], u2: Pt = [B[0] - P[0], B[1] - P[1]];
    const l1 = Math.hypot(...u1), l2 = Math.hypot(...u2);
    u1[0] /= l1; u1[1] /= l1; u2[0] /= l2; u2[1] /= l2;
    const th = Math.acos(u1[0] * u2[0] + u1[1] * u2[1]), t = r / Math.tan(th / 2);
    const T1 = [P[0] + u1[0] * t + dx, P[1] + u1[1] * t + dy], T2 = [P[0] + u2[0] * t + dx, P[1] + u2[1] * t + dy];
    const cross = (P[0] - A[0]) * (B[1] - P[1]) - (P[1] - A[1]) * (B[0] - P[0]);
    d += `${i ? "L" : "M"}${T1[0]} ${T1[1]} A${r} ${r} 0 0 ${cross > 0 ? 1 : 0} ${T2[0]} ${T2[1]} `;
  }
  return d + "Z";
}

// A double arrow along x (or y), every corner rounded in proportion to the head.
function dbl(cx: number, cy: number, a: number, o: { t?: number; h?: number; hl?: number; vertical?: boolean } = {}): Shape {
  const t = o.t ?? 1.5, h = o.h ?? 4, hl = o.hl ?? 4.6;
  let p: Pt[] = [[-a, 0], [-a + hl, -h], [-a + hl, -t], [a - hl, -t], [a - hl, -h], [a, 0], [a - hl, h], [a - hl, t], [-a + hl, t], [-a + hl, h]];
  if (o.vertical) p = p.map(([x, y]) => [y, x]);
  const radii = [0.25, 0.2, 0.12, 0.12, 0.2, 0.25, 0.2, 0.12, 0.12, 0.2].map((k) => k * h);
  return { d: roundPoly(p.map(([x, y]) => [cx + x, cy + y]), radii) };
}
const rect = (x: number, y: number, w: number, h: number, r: number): Shape => ({
  d: `M${x + r} ${y} H${x + w - r} A${r} ${r} 0 0 1 ${x + w} ${y + r} V${y + h - r} A${r} ${r} 0 0 1 ${x + w - r} ${y + h} H${x + r} A${r} ${r} 0 0 1 ${x} ${y + h - r} V${y + r} A${r} ${r} 0 0 1 ${x + r} ${y} Z`,
});
const circle = (cx: number, cy: number, r: number): Shape => ({
  d: `M${cx - r} ${cy} A${r} ${r} 0 1 0 ${cx + r} ${cy} A${r} ${r} 0 1 0 ${cx - r} ${cy} Z`,
});
const line = (d: string, w: number): Shape => ({ d, line: w });
const rot = (s: Shape, deg: number): Shape => ({ ...s, rotate: deg });

// The peer pointer the layer has always drawn (pointerPath() in the layer):
// tip, two wings and the notch, all rounded, shifted back so the rounded tip
// still sits on the hotspot.
const pointer: Shape = (() => {
  const z = 20, wing = 0.37 * z, nn = (0.8 * (z + wing)) / 2;
  const half = Math.atan2(z, wing) - Math.PI / 4, inset = (1.6 / Math.sin(half) - 1.6) / Math.SQRT2;
  return { d: roundPoly([[0, 0], [wing, z], [nn, nn], [z, wing]], [1.6, 1.8, 2.4, 1.8], -inset, -inset) };
})();

const arrow = (cx: number, cy: number, a: number) => dbl(cx, cy, a, { t: 1.25, h: 3.9, hl: 4.4 });
const razorBlade: Layer[] = [
  { shapes: [rot({ d: "M1.2 0 H15.8 A1.2 1.2 0 0 1 17 1.2 V3.2 Q15.7 4.5 17 5.8 V7.8 A1.2 1.2 0 0 1 15.8 9 H1.2 A1.2 1.2 0 0 1 0 7.8 V5.8 Q1.3 4.5 0 3.2 V1.2 A1.2 1.2 0 0 1 1.2 0 Z" }, 32)] },
  { shapes: [rot(rect(3.2, 3.7, 10.6, 1.6, 0.8), 32), rot(circle(8.5, 4.5, 1.7), 32)], ink: true },
];
const trimSide = (dir: 1 | -1): Layer[] => {
  const x = dir > 0 ? -1.1 : -6.9;  // serifs reach into the item
  return [
    { shapes: [rect(-1.1, -10, 2.2, 20, 1.1), rect(x, -10, 8, 2.2, 1.1), rect(x, 7.8, 8, 2.2, 1.1)] },
    { shapes: [dbl(0, 0, 10.5)] },
  ];
};

// Key on the wire, then the layers. The first one is the plain pointer.
const GLYPHS: [string, Layer[]][] = [
  ["", [{ shapes: [pointer] }]],
  ["trim_l", trimSide(1)],
  ["trim_m", [
    { shapes: [rect(-3.6, -10, 2, 20, 1), rect(-9.6, -10, 8, 2.2, 1.1), rect(-9.6, 7.8, 8, 2.2, 1.1),
               rect(1.6, -10, 2, 20, 1), rect(1.6, -10, 8, 2.2, 1.1), rect(1.6, 7.8, 8, 2.2, 1.1)] },
    { shapes: [dbl(0, 0, 11.5)] }]],
  ["trim_r", trimSide(-1)],
  ["fade_in", [
    { shapes: [{ d: "M-13 11 C-7.5 10.5 -2.2 6.5 0 0 L0 11 Z" }] },
    { shapes: [dbl(0, -7, 8, { t: 1.25, h: 3.9, hl: 4.4 })] },
    { shapes: [circle(0, 0, 2.4)], ink: true }]],
  ["fade_out", [
    { shapes: [{ d: "M13 11 C7.5 10.5 2.2 6.5 0 0 L0 11 Z" }] },
    { shapes: [dbl(0, -7, 8, { t: 1.25, h: 3.9, hl: 4.4 })] },
    { shapes: [circle(0, 0, 2.4)], ink: true }]],
  ["fade_curve", [
    { shapes: [line("M-8.5 8 C-1.5 8 1.5 -8 8.5 -8", 2.4)] },
    { shapes: [arrow(0, 0, 9)] }]],
  ["fade_len", [
    { shapes: [line("M-10.5 -5.5 C-7.5 -5.5 -6.5 -13.5 -3.5 -13.5 L3.5 -13.5 C6.5 -13.5 7.5 -5.5 10.5 -5.5", 2.4)] },
    { shapes: [arrow(0, 0, 11.5)] }]],
  ["xfade", [
    { shapes: [line("M-10.5 7.5 C-4 7.5 4 -7.5 10.5 -7.5", 2.4), line("M-10.5 -7.5 C-4 -7.5 4 7.5 10.5 7.5", 2.4)] },
    { shapes: [circle(0, 0, 2.3)], ink: true }]],
  ["xfade_move", [
    { shapes: [line("M-6.5 -0.4 C-2.5 -0.4 2.5 -9.6 6.5 -9.6", 2.4), line("M-6.5 -9.6 C-2.5 -9.6 2.5 -0.4 6.5 -0.4", 2.4)] },
    { shapes: [arrow(0, 6, 11.5)] }]],
  ["slip", [
    { shapes: [rect(-11.5, -7.5, 23, 15, 3)], tint: true },
    { shapes: [dbl(0, 0, 8, { t: 1.2, h: 3.7, hl: 4.1 })] }]],
  ["razor", razorBlade],
  ["razor_move", [...razorBlade,
    { shapes: [dbl(14.5, 14, 6, { t: 1.1, h: 2.7, hl: 3 }), dbl(14.5, 14, 6, { t: 1.1, h: 2.7, hl: 3, vertical: true })] }]],
  ["razor_copy", [...razorBlade,
    { shapes: [rect(9.5, 9, 10, 10, 2.4)] },
    { shapes: [rect(13.7, 11.2, 1.6, 5.6, 0.8), rect(11.7, 13.2, 5.6, 1.6, 0.8)], ink: true }]],
];

// ---------- SVG path sampling (M L H V C Q A Z, absolute only) ----------

const STEP = 0.05;  // pt between samples before resampling

function arcPoints(p0: Pt, rx: number, ry: number, large: number, sweep: number, p1: Pt): Pt[] {
  // Endpoint to centre parameterisation (SVG implementation notes F.6.5), no rotation.
  const dx = (p0[0] - p1[0]) / 2, dy = (p0[1] - p1[1]) / 2;
  const lam = (dx * dx) / (rx * rx) + (dy * dy) / (ry * ry);
  if (lam > 1) { rx *= Math.sqrt(lam); ry *= Math.sqrt(lam); }
  const num = rx * rx * ry * ry - rx * rx * dy * dy - ry * ry * dx * dx;
  const den = rx * rx * dy * dy + ry * ry * dx * dx;
  let k = Math.sqrt(Math.max(0, num / den));
  if (large === sweep) k = -k;
  const cxp = (k * rx * dy) / ry, cyp = (-k * ry * dx) / rx;
  const cx = cxp + (p0[0] + p1[0]) / 2, cy = cyp + (p0[1] + p1[1]) / 2;
  const ang = (ux: number, uy: number, vx: number, vy: number) => Math.atan2(ux * vy - uy * vx, ux * vx + uy * vy);
  const t1 = ang(1, 0, (dx - cxp) / rx, (dy - cyp) / ry);
  let dt = ang((dx - cxp) / rx, (dy - cyp) / ry, (-dx - cxp) / rx, (-dy - cyp) / ry);
  if (!sweep && dt > 0) dt -= 2 * Math.PI;
  if (sweep && dt < 0) dt += 2 * Math.PI;
  const n = Math.max(2, Math.ceil((Math.abs(dt) * Math.max(rx, ry)) / STEP));
  const out: Pt[] = [];
  for (let i = 1; i <= n; i++) {
    const t = t1 + (dt * i) / n;
    out.push([cx + rx * Math.cos(t), cy + ry * Math.sin(t)]);
  }
  return out;
}

function samplePath(d: string): { pts: Pt[]; closed: boolean } {
  const tok = d.match(/[A-Za-z]|-?\d*\.?\d+(?:e-?\d+)?/g) ?? [];
  let i = 0;
  const num = () => parseFloat(tok[i++]);
  const pts: Pt[] = [];
  let cur: Pt = [0, 0], cmd = "", closed = false;
  const lineTo = (p: Pt) => {
    const n = Math.max(1, Math.ceil(Math.hypot(p[0] - cur[0], p[1] - cur[1]) / STEP));
    for (let k = 1; k <= n; k++) pts.push([cur[0] + ((p[0] - cur[0]) * k) / n, cur[1] + ((p[1] - cur[1]) * k) / n]);
    cur = p;
  };
  while (i < tok.length) {
    if (/[A-Za-z]/.test(tok[i])) cmd = tok[i++];
    switch (cmd) {
      case "M": cur = [num(), num()]; pts.push(cur); cmd = "L"; break;
      case "L": lineTo([num(), num()]); break;
      case "H": lineTo([num(), cur[1]]); break;
      case "V": lineTo([cur[0], num()]); break;
      case "C": {
        const c1: Pt = [num(), num()], c2: Pt = [num(), num()], p: Pt = [num(), num()], p0 = cur;
        for (let k = 1; k <= 400; k++) {
          const t = k / 400, u = 1 - t;
          pts.push([u * u * u * p0[0] + 3 * u * u * t * c1[0] + 3 * u * t * t * c2[0] + t * t * t * p[0],
                    u * u * u * p0[1] + 3 * u * u * t * c1[1] + 3 * u * t * t * c2[1] + t * t * t * p[1]]);
        }
        cur = p;
        break;
      }
      case "Q": {
        const c: Pt = [num(), num()], p: Pt = [num(), num()], p0 = cur;
        for (let k = 1; k <= 200; k++) {
          const t = k / 200, u = 1 - t;
          pts.push([u * u * p0[0] + 2 * u * t * c[0] + t * t * p[0], u * u * p0[1] + 2 * u * t * c[1] + t * t * p[1]]);
        }
        cur = p;
        break;
      }
      case "A": {
        const rx = num(), ry = num();
        num();  // x-axis rotation: always 0 here
        const large = num(), sweep = num(), p: Pt = [num(), num()];
        pts.push(...arcPoints(cur, rx, ry, large, sweep, p));
        cur = p;
        break;
      }
      case "Z": case "z": closed = true; i = tok.length; break;
      default: throw new Error(`unsupported path command ${cmd} in ${d}`);
    }
  }
  return { pts, closed };
}

// The outline of a stroked open path: one side forward, a round cap, the other
// side back, a round cap. The paths here are smooth, so offsetting is enough.
function strokeOutline(c: Pt[], w: number): Pt[] {
  const r = w / 2, n = c.length;
  const normal = (k: number): Pt => {
    const a = c[Math.max(0, k - 1)], b = c[Math.min(n - 1, k + 1)];
    const l = Math.hypot(b[0] - a[0], b[1] - a[1]);
    return [-(b[1] - a[1]) / l, (b[0] - a[0]) / l];
  };
  const left: Pt[] = [], right: Pt[] = [];
  for (let k = 0; k < n; k++) {
    const nm = normal(k);
    left.push([c[k][0] + nm[0] * r, c[k][1] + nm[1] * r]);
    right.push([c[k][0] - nm[0] * r, c[k][1] - nm[1] * r]);
  }
  const cap = (p: Pt, from: Pt): Pt[] => {
    const a0 = Math.atan2(from[1] - p[1], from[0] - p[0]), out: Pt[] = [];
    for (let k = 1; k < 48; k++) {
      const a = a0 - (Math.PI * k) / 48;
      out.push([p[0] + r * Math.cos(a), p[1] + r * Math.sin(a)]);
    }
    return out;
  };
  return [...left, ...cap(c[n - 1], left[n - 1]), ...right.reverse(), ...cap(c[0], right[right.length - 1])];
}

const area = (r: Pt[]) => r.reduce((s, p, k) => { const q = r[(k + 1) % r.length]; return s + p[0] * q[1] - q[0] * p[1]; }, 0) / 2;

function resample(ring: Pt[], m: number): Pt[] {
  if (area(ring) < 0) ring = [...ring].reverse();  // clockwise on screen (y down)
  const len: number[] = [0];
  for (let k = 1; k <= ring.length; k++) {
    const a = ring[k - 1], b = ring[k % ring.length];
    len.push(len[k - 1] + Math.hypot(b[0] - a[0], b[1] - a[1]));
  }
  const total = len[ring.length], out: Pt[] = [];
  let seg = 0;
  for (let k = 0; k < m; k++) {
    const s = (total * k) / m;
    while (len[seg + 1] < s) seg++;
    const a = ring[seg], b = ring[(seg + 1) % ring.length], f = (s - len[seg]) / (len[seg + 1] - len[seg] || 1);
    out.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
  }
  return out;
}

// ---------- output ----------

type Part = { kind: number; layer: number; pts: Pt[] };
const KIND = { solid: 0, tint: 1, ink: 2 };

const parts: Part[] = [];
const glyphs: { key: string; first: number; count: number; maxX: number; maxY: number }[] = [];
for (const [key, layers] of GLYPHS) {
  const first = parts.length;
  let maxX = -Infinity, maxY = -Infinity;
  layers.forEach((L, li) => {
    for (const s of L.shapes) {
      const { pts, closed } = samplePath(s.d);
      let ring = s.line ? strokeOutline(pts, s.line) : pts;
      if (!s.line && !closed) throw new Error(`open fill in ${key}`);
      if (s.rotate) {
        const a = (s.rotate * Math.PI) / 180, ca = Math.cos(a), sa = Math.sin(a);
        ring = ring.map(([x, y]) => [x * ca - y * sa, x * sa + y * ca]);
      }
      const r = resample(ring, POINTS);
      for (const [x, y] of r) { maxX = Math.max(maxX, x); maxY = Math.max(maxY, y); }
      parts.push({ kind: L.ink ? KIND.ink : L.tint ? KIND.tint : KIND.solid, layer: li, pts: r });
    }
  });
  glyphs.push({ key, first, count: parts.length - first, maxX, maxY });
}

const q = (v: number) => Math.round(v * UNIT);
let out = `// Generated by tools/cursor_glyphs.ts: the peer cursor glyphs as polygons of
// kPoints points each (int16, 1/${UNIT} pt, hotspot at 0,0, y down, clockwise).
// Do not edit; change the shapes there and run it again.

static const int16_t kGlyphPoints[][kPoints * 2] = {
`;
for (const p of parts) out += `    {${p.pts.map(([x, y]) => `${q(x)},${q(y)}`).join(",")}},\n`;
out += `};

// kind (0 solid, 1 tint, 2 ink), layer
static const uint8_t kGlyphPartInfo[][2] = {
`;
for (const p of parts) out += `    {${p.kind}, ${p.layer}},\n`;
out += `};

// wire key, first part, part count, right and bottom extent (pt)
static const GlyphDef kGlyphDefs[] = {
`;
for (const g of glyphs) out += `    {"${g.key}", ${g.first}, ${g.count}, ${g.maxX.toFixed(2)}f, ${g.maxY.toFixed(2)}f},\n`;
out += `};
`;

await Bun.write(new URL("../extension/src/cursor_glyphs.inc", import.meta.url), out);
console.log(`${glyphs.length} glyphs, ${parts.length} parts`);
