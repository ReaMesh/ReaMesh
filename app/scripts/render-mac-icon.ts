// Render the legacy macOS icons (icon.icns, icons/*.png, ReaMesh-icon.png;
// the Windows Square*/StoreLogo tiles come from render-windows-icon.ts)
// from the Icon Composer layer in ReaMesh.icon. Run `bun run icons:mac` after
// editing the layer or icon.json.
//
// macOS 26+ draws the icon from resources/Assets.car, which only Xcode's
// actool can compile (on a Mac):
//   xcrun actool src-tauri/ReaMesh.icon --compile <out> --app-icon ReaMesh \
//     --platform macosx --target-device mac --minimum-deployment-target 26.0 \
//     --output-partial-info-plist <out>/partial.plist
// then copy <out>/Assets.car to src-tauri/resources/. Everything older reads
// the icns this script writes, so it approximates the system render on its
// own: the near-black tile on Apple's 824/1024 grid with its lit rim and drop
// shadow, the white layer with the light-to-grey falloff, dark edge and shadow
// of the glass material. The icns tile is icon.json's top-to-bottom gradient
// (macOS only); the PNGs, which other platforms' bundles use, keep the flat
// tile.
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const resolve = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));

const layerSvg = await Bun.file(resolve("../src-tauri/ReaMesh.icon/Assets/reamesh-R-weiss.svg")).text();
const icon = await Bun.file(resolve("../src-tauri/ReaMesh.icon/icon.json")).json();

const attr = (name: string) => Number(layerSvg.match(new RegExp(`\\s${name}="([\\d.]+)"`))![1]);
const [, , vbW, vbH] = layerSvg.match(/viewBox="([^"]+)"/)![1].split(/\s+/).map(Number);
const paths = [...layerSvg.matchAll(/\sd="([^"]+)"/g)].map((m) => m[1]);

// Icon Composer: the layer is centred on the 1024pt canvas at its SVG size
// times the group (and layer) scale, then moved by the group translation.
const group = icon.groups[0];
const scale = group.position.scale * group.layers[0].position.scale;
const [dx, dy] = group.position["translation-in-points"];
const w = attr("width") * scale;
const h = attr("height") * scale;

// The canvas maps onto the 824pt tile of the 1024 macOS icon grid.
const TILE = 824;
const INSET = (1024 - TILE) / 2;
const k = TILE / 1024;
const x = INSET + (512 - w / 2 + dx) * k;
const y = INSET + (512 - h / 2 + dy) * k;
const sx = (w * k) / vbW;
const sy = (h * k) / vbH;

// icon.json's background: a linear gradient from top to bottom, given as two
// "srgb:r,g,b,a" stops.
const hex = (c: string) =>
  "#" +
  c
    .replace(/^srgb:/, "")
    .split(",")
    .slice(0, 3)
    .map((v) => Math.round(Number(v) * 255).toString(16).padStart(2, "0"))
    .join("");
const [tileTop, tileBottom] = (icon.fill["linear-gradient"] as string[]).map(hex);

const FLAT_TILE = "#1a1a1b";

// An 8-bit gradient over 16 grey levels shows as bands on an 824px tile;
// a faint luminance noise (at most ~2 levels) breaks them up.
const grain = `
    <filter id="grain" x="0" y="0" width="100%" height="100%" color-interpolation-filters="sRGB">
      <feTurbulence type="fractalNoise" baseFrequency="0.9" numOctaves="2" seed="7" stitchTiles="stitch"/>
      <feColorMatrix type="matrix" values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0.03 0 0 0 -0.006"/>
      <feComposite operator="in" in2="SourceGraphic"/>
    </filter>`;

const svgFor = (gradient: boolean) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <defs>${gradient ? grain : ""}
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="${tileTop}"/>
      <stop offset="1" stop-color="${tileBottom}"/>
    </linearGradient>
    <linearGradient id="rim" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.30"/>
      <stop offset="0.5" stop-color="#ffffff" stop-opacity="0.06"/>
      <stop offset="1" stop-color="#ffffff" stop-opacity="0.18"/>
    </linearGradient>
    <linearGradient id="glyph" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffffff"/>
      <stop offset="0.45" stop-color="#e2e2e4"/>
      <stop offset="1" stop-color="#8e8e92"/>
    </linearGradient>
    <filter id="tileShadow" x="-20%" y="-20%" width="140%" height="140%"${
      // Filters work in linearRGB by default, which at 8 bits collapses the
      // dark gradient into a few wide steps.
      gradient ? ' color-interpolation-filters="sRGB"' : ""
    }>
      <feDropShadow dx="0" dy="10" stdDeviation="12" flood-color="#000" flood-opacity="0.5"/>
    </filter>
    <filter id="glyphShadow" x="-20%" y="-20%" width="140%" height="140%">
      <feDropShadow dx="0" dy="12" stdDeviation="12" flood-color="#000" flood-opacity="0.8"/>
    </filter>
  </defs>
  <rect x="${INSET}" y="${INSET}" width="${TILE}" height="${TILE}" rx="185" fill="${gradient ? "url(#tile)" : FLAT_TILE}" filter="url(#tileShadow)"/>${
    gradient
      ? `
  <rect x="${INSET}" y="${INSET}" width="${TILE}" height="${TILE}" rx="185" fill="#fff" filter="url(#grain)"/>`
      : ""
  }
  <rect x="${INSET + 1.5}" y="${INSET + 1.5}" width="${TILE - 3}" height="${TILE - 3}" rx="183.5" fill="none" stroke="url(#rim)" stroke-width="3"/>
  <g filter="url(#glyphShadow)">
    <g transform="translate(${x} ${y}) scale(${sx} ${sy})">
${paths.map((d) => `      <path d="${d}" fill="none" stroke="#000" stroke-opacity="0.85" stroke-width="${5 / sx}" stroke-linejoin="round"/>`).join("\n")}
${paths.map((d) => `      <path d="${d}" fill="url(#glyph)"/>`).join("\n")}
    </g>
  </g>
</svg>`;

const render = (svg: string, size: number) =>
  Buffer.from(new Resvg(svg, { fitTo: { mode: "width", value: size } }).render().asPng());

const cache = (svg: string) => {
  const pngs = new Map<number, Buffer>();
  return (size: number) => {
    if (!pngs.has(size)) pngs.set(size, render(svg, size));
    return pngs.get(size)!;
  };
};
const png = cache(svgFor(false));
const icnsPng = cache(svgFor(true));

const out = (rel: string, data: Buffer) => Bun.write(resolve(`../src-tauri/${rel}`), data);

await out("ReaMesh-icon.png", png(1024));
await out("icons/icon.png", png(512));
for (const s of [32, 64, 128]) await out(`icons/${s}x${s}.png`, png(s));
await out("icons/128x128@2x.png", png(256));

// icns: 'icns' + total length, then (type, length incl. 8-byte header, PNG).
const ICNS: [string, number][] = [
  ["icp4", 16],
  ["icp5", 32],
  ["ic11", 32],
  ["icp6", 64],
  ["ic12", 64],
  ["ic07", 128],
  ["ic08", 256],
  ["ic13", 256],
  ["ic09", 512],
  ["ic14", 512],
  ["ic10", 1024],
];
const entries = ICNS.map(([type, size]) => {
  const data = icnsPng(size);
  const head = Buffer.alloc(8);
  head.write(type, 0, "ascii");
  head.writeUInt32BE(data.length + 8, 4);
  return Buffer.concat([head, data]);
});
const head = Buffer.alloc(8);
head.write("icns", 0, "ascii");
head.writeUInt32BE(8 + entries.reduce((n, e) => n + e.length, 0), 4);
await out("icons/icon.icns", Buffer.concat([head, ...entries]));
console.log("wrote icon.icns, ReaMesh-icon.png and the icons/*.png sizes");
