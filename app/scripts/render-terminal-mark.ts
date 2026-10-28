// Render proprietary/logo/mark.svg as text for terminals: mark.txt in braille
// (2×4 dots per character) and mark-blocks.txt in half blocks (1×2 per
// character) for consoles without a braille font. reamesh-relay prints them
// next to its start banner. Run `bun run icons:terminal` after editing the SVG.
//
// Each dot is lit when the mark covers at least half of its square, measured
// on a render much finer than the dot grid, so the outline doesn't depend on
// where a single sample pixel happens to fall.
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const COLUMNS = 14;
const ROWS = 7;
// Pixels per dot along each axis in the fine render.
const FINE = 12;

const resolve = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));
const svg = await Bun.file(resolve("../../proprietary/logo/mark.svg")).text();

/** A width×height grid of lit dots, the mark centred and fitted inside. */
function dots(width: number, height: number): boolean[][] {
  const img = new Resvg(svg, { fitTo: { mode: "height", value: height * FINE } }).render();
  const pad = Math.floor((width * FINE - img.width) / 2);
  const alpha = (x: number, y: number) => {
    const sx = x - pad;
    if (sx < 0 || sx >= img.width || y >= img.height) return 0;
    return img.pixels[(y * img.width + sx) * 4 + 3] / 255;
  };
  return Array.from({ length: height }, (_, dy) =>
    Array.from({ length: width }, (_, dx) => {
      let sum = 0;
      for (let y = dy * FINE; y < (dy + 1) * FINE; y++)
        for (let x = dx * FINE; x < (dx + 1) * FINE; x++) sum += alpha(x, y);
      return sum / (FINE * FINE) >= 0.5;
    }),
  );
}

// Braille dot bits, by row then column within the 2×4 cell.
const BRAILLE = [
  [0x01, 0x08],
  [0x02, 0x10],
  [0x04, 0x20],
  [0x40, 0x80],
];

function braille(): string {
  const d = dots(COLUMNS * 2, ROWS * 4);
  const lines: string[] = [];
  for (let r = 0; r < ROWS; r++) {
    let line = "";
    for (let c = 0; c < COLUMNS; c++) {
      let bits = 0;
      for (let y = 0; y < 4; y++)
        for (let x = 0; x < 2; x++) if (d[r * 4 + y][c * 2 + x]) bits |= BRAILLE[y][x];
      // U+2800 (no dots) rather than a space, so every cell comes from the
      // same font and the lines line up.
      line += String.fromCodePoint(0x2800 + bits);
    }
    lines.push(line);
  }
  return lines.join("\n") + "\n";
}

function blocks(): string {
  const d = dots(COLUMNS, ROWS * 2);
  const lines: string[] = [];
  for (let r = 0; r < ROWS; r++) {
    let line = "";
    for (let c = 0; c < COLUMNS; c++) {
      const top = d[r * 2][c];
      const bottom = d[r * 2 + 1][c];
      line += top && bottom ? "█" : top ? "▀" : bottom ? "▄" : " ";
    }
    lines.push(line);
  }
  return lines.join("\n") + "\n";
}

await Bun.write(resolve("../../proprietary/logo/mark.txt"), braille());
await Bun.write(resolve("../../proprietary/logo/mark-blocks.txt"), blocks());
