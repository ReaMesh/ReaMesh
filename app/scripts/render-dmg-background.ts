// Renders scripts/dmg-background.svg to a 2x PNG for the DMG window.
// Run via `bun scripts/render-dmg-background.ts` (make-dmg.sh does this);
// the caller stamps 144 dpi onto it with sips so Finder draws it at 1x size.
import { mkdirSync } from "node:fs";
import { Resvg } from "@resvg/resvg-js";

const svg = await Bun.file(new URL("./dmg-background.svg", import.meta.url).pathname).text();
const resvg = new Resvg(svg, {
  fitTo: { mode: "width", value: 1320 }, // 660pt @2x
  font: { loadSystemFonts: true, defaultFontFamily: "Helvetica Neue" },
});
mkdirSync("scripts/.build", { recursive: true });
await Bun.write("scripts/.build/dmg-background.png", resvg.render().asPng());
console.log("wrote scripts/.build/dmg-background.png");
