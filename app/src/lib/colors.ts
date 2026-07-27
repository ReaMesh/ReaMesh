// Concrete color values (mirror --muted-foreground / --found in index.css).
// Hardcoded rather than `var(...)` because Motion can only interpolate between
// real colors: a CSS variable would snap instead of easing grey→blue. The
// theme is always-dark (one value each), so there's no light-mode variant.
export const SCAN_COLOR = "#8b8b94"; // --muted-foreground
export const FOUND_COLOR = "#6aa6f0"; // --found
export const FG_COLOR = "#e7e7ea"; // --foreground

/** Choose readable initials from the background's relative luminance.
 *  Peers may send any color, including colors outside the local palette. */
export function inkFor(bg: string | undefined): string {
  const rgb = parseHex(bg);
  if (!rgb) return "rgba(0,0,0,0.8)";
  // WCAG relative luminance.
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  const l = 0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2]);
  return l > 0.4 ? "rgba(0,0,0,0.82)" : "#ffffff";
}

function parseHex(v: string | undefined): [number, number, number] | null {
  if (!v) return null;
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(v.trim());
  if (!m) return null;
  const h = m[1].length === 3 ? m[1].replace(/./g, (c) => c + c) : m[1];
  return [
    parseInt(h.slice(0, 2), 16),
    parseInt(h.slice(2, 4), 16),
    parseInt(h.slice(4, 6), 16),
  ];
}
