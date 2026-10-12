import type { CSSProperties } from "react";
import { cn } from "@/lib/utils";

// The ReaMesh logo: the calligraphic R, alone or with the wordmark. One
// geometry everywhere (title bar, onboarding); only the scale changes. Filled
// with currentColor so it takes the text colour of wherever it sits.
// Source: design/logo/mark.svg and design/logo/lockup-on-*.svg.
const MARK_W = 98.6;
const H = 100.8;
const MARK = [
  "M96.84,100.8h-18.87c-2.16,0-4.22-.89-5.72-2.44-6.63-6.85-11.99-15.2-15.53-24.77-.41-1.12-.8-2.24-1.14-3.36-.7-2.23-1.29-4.48-1.76-6.72-.56-2.68.51-5.41,2.62-7.16,1.09-.9,2.12-1.8,3.1-2.71.39-.36.77-.72,1.14-1.08,8.56-8.26,12.59-16.63,10.12-23.31-1.64-4.43-5.93-7.57-12.1-9.39-.07-.02-.16-.05-.25-.07-6.24-1.8-14.37-2.27-23.58-1.37-1.17.11-2.28-.57-2.64-1.69-1.4-4.44-2.33-9.19-2.74-14C29.38,1.27,30.51,0,31.98.02c11.8.18,22.67,1.65,30.96,6.07,1.02.55,1.99,1.12,2.92,1.73,5.61,3.63,9.79,8.5,11.98,14.44,4.03,10.95.55,23.44-8.32,34.66-1.38,1.77-2.91,3.51-4.57,5.21-1,1.03-1.31,2.56-.68,3.85,7.27,14.77,19.15,26.01,33.1,32.52,1.17.55.77,2.31-.52,2.31Z",
  "M30.67,29.28c-1.93-3.64-3.57-7.45-4.89-11.41-1.69-5.02-2.87-10.28-3.47-15.71C22.17.93,21.1,0,19.86,0H2.48C.04,0-.92,3.12,1.08,4.53c2.36,1.67,4.62,3.47,6.77,5.4,2.33,2.09,4.54,4.33,6.58,6.69.02,0,.02.02.02.02,1.74,1.99,3.36,4.09,4.88,6.27,1.14,1.64,2.24,3.35,3.23,5.08v.02c3.07,5.26,5.49,10.93,7.19,16.89,1.54,5.47,2.48,11.2,2.72,17.12.06,1.05.08,2.11.08,3.17,0,11.47-2.6,22.32-7.25,32.02-.8,1.66.45,3.59,2.29,3.59h16.24c1.15,0,2.17-.77,2.44-1.88,1.5-5.97,2.29-12.22,2.29-18.65,0-6.95-.93-13.68-2.67-20.08h0c-1.35-4.96-3.3-9.73-5.81-14.2l-9.39-16.71Z",
];
function Paths({ ds }: { ds: string[] }) {
  return ds.map((d) => <path key={d.slice(0, 16)} d={d} fill="currentColor" />);
}

export function RMark({ height = 12, className }: { height?: number; className?: string }) {
  return (
    <svg
      viewBox={`0 0 ${MARK_W} ${H}`}
      height={height}
      width={(height * MARK_W) / H}
      aria-hidden="true"
      className={className}
    >
      <Paths ds={MARK} />
    </svg>
  );
}

// R + "ReaMesh". `height` is the R's height; the wordmark scales with it.
// The wordmark is live text in the UI font, not outlines: the design files
// set it in SF Pro, which may not ship as artwork. It keeps their proportions
// (in units of the R's 100.8 height): cap height 49.64, baseline 24 above the
// R's foot, ink starting 9 right of the R, tracked to the drawn width.
// font-size-adjust pins the cap height whatever font the platform picks, and
// text-box trims the line box to cap height and baseline so the bottom margin
// places the baseline exactly.
export function Lockup({ height = 18, className }: { height?: number; className?: string }) {
  const u = height / H;
  const cap = 49.64 * u;
  return (
    <span className={cn("flex items-end", className)} role="img" aria-label="ReaMesh">
      <RMark height={height} />
      <span
        aria-hidden="true"
        className="whitespace-nowrap font-semibold"
        style={{
          fontSize: cap / 0.7,
          fontSizeAdjust: "cap-height 0.7",
          lineHeight: 1,
          letterSpacing: -0.02 * height,
          // 9 units of gap minus the R glyph's own left side bearing.
          marginLeft: 0.0288 * height,
          marginBottom: 24 * u,
          textBox: "trim-both cap alphabetic",
        } as CSSProperties}
      >
        ReaMesh
      </span>
    </span>
  );
}
