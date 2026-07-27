// Render windows-mark.svg into icon.ico for the Windows executable and taskbar.
// Run `bun run icons:windows` after editing the SVG. macOS uses icon.icns.
//
// An ICO has a 6-byte header and a 16-byte directory entry per image.
// Windows supports PNG-compressed entries at every size since Vista.
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

// Windows asks for whatever size the current DPI scaling works out to, not
// just the round ones: 20 and 40 are the 125% and 250% small-icon sizes, 96 is
// the 200% Explorer tile. Leaving them out is what makes an icon look
// pixelated: Windows resamples the nearest entry it does find rather than
// rendering the right size.
const SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];

// fileURLToPath, not URL.pathname: on Windows the latter yields "/C:/..." with
// a leading slash, which no file API accepts.
const resolve = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));

const svg = await Bun.file(resolve("../src-tauri/icons/windows-mark.svg")).text();

const pngs = SIZES.map((size) => {
  const png = new Resvg(svg, { fitTo: { mode: "width", value: size } }).render().asPng();
  return { size, png };
});

const ICONDIR = 6;
const ICONDIRENTRY = 16;
const header = Buffer.alloc(ICONDIR + ICONDIRENTRY * pngs.length);
header.writeUInt16LE(0, 0); // reserved
header.writeUInt16LE(1, 2); // 1 = icon (2 would be a cursor)
header.writeUInt16LE(pngs.length, 4);

let offset = header.length;
pngs.forEach(({ size, png }, i) => {
  const at = ICONDIR + i * ICONDIRENTRY;
  // 256 is stored as 0: the field is one byte, so 256 does not fit.
  header.writeUInt8(size >= 256 ? 0 : size, at + 0); // width
  header.writeUInt8(size >= 256 ? 0 : size, at + 1); // height
  header.writeUInt8(0, at + 2); // palette size (0 = truecolour)
  header.writeUInt8(0, at + 3); // reserved
  header.writeUInt16LE(1, at + 4); // colour planes
  header.writeUInt16LE(32, at + 6); // bits per pixel
  header.writeUInt32LE(png.length, at + 8);
  header.writeUInt32LE(offset, at + 12);
  offset += png.length;
});

const out = resolve("../src-tauri/icons/icon.ico");
await Bun.write(out, Buffer.concat([header, ...pngs.map((p) => p.png)]));
console.log(`wrote icon.ico (${pngs.map((p) => p.size).join(", ")}px)`);
