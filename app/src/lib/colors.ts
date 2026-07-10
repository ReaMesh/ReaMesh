// Concrete color values (mirror --muted-foreground / --found in index.css).
// Hardcoded rather than `var(...)` because Motion can only interpolate between
// real colors: a CSS variable would snap instead of easing grey→blue. The
// theme is always-dark (one value each), so there's no light-mode variant.
export const SCAN_COLOR = "#8b8b94"; // --muted-foreground
export const FOUND_COLOR = "#6aa6f0"; // --found
