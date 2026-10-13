import { useEffect, useId, type CSSProperties } from "react";
import { animate, motion, useMotionValue, useReducedMotion, useTransform, type MotionValue } from "motion/react";
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

// The R filling in like ink: up the left stroke from the foot, then down the
// right one from the top, as one eased motion. Plays once on mount. Each
// stroke is masked by a level with a soft feathered edge whose geometry is
// rebuilt from one progress value, so nothing is transformed inside the mask
// (WebKit is unreliable there) and nothing keeps ticking once it is full.
//
// The level moves at a constant rate of ink, not of height: it slows down
// where a stroke is wide (the bowl's top arc, the foot) and speeds up where it
// is thin, and since both strokes share one rate there is no jump in speed at
// the handoff. The ink per row is measured once from the paths themselves.
const [RIGHT, LEFT] = MARK;
const FEATHER = 12;
const SCALE = 4;

/** Where the band's centre sits, from the start of the stroke's travel, for
 *  each 1/SCALE unit step: cumulative ink, normalised later. */
type Profile = { cum: Float64Array; total: number };

function profile(d: string, up: boolean): Profile {
  const rows = Math.ceil(H * SCALE);
  const ink = new Float64Array(rows);
  try {
    const canvas = document.createElement("canvas");
    canvas.width = Math.ceil(MARK_W * SCALE);
    canvas.height = rows;
    const ctx = canvas.getContext("2d");
    if (ctx) {
      ctx.scale(SCALE, SCALE);
      ctx.fill(new Path2D(d));
      const px = ctx.getImageData(0, 0, canvas.width, rows).data;
      for (let y = 0; y < rows; y++) {
        let sum = 0;
        for (let x = 0; x < canvas.width; x++) sum += px[(y * canvas.width + x) * 4 + 3];
        ink[y] = sum / 255;
      }
    }
  } catch {
    // No canvas: every row weighs the same, i.e. a plain linear sweep.
  }
  // The travel runs half a feather past both ends so the band starts and
  // ends fully outside the stroke. Every step carries a floor of ink, so the
  // edge never stalls in an empty row nor races through a thin one.
  const pad = Math.round((FEATHER / 2) * SCALE);
  const steps = rows + 2 * pad;
  const mean = ink.reduce((a, b) => a + b, 0) / rows || 1;
  const cum = new Float64Array(steps + 1);
  for (let i = 0; i < steps; i++) {
    const row = up ? rows - 1 - (i - pad) : i - pad;
    cum[i + 1] = cum[i] + (row >= 0 && row < rows ? ink[row] : 0) + 0.25 * mean;
  }
  return { cum, total: cum[steps] };
}

/** The band's centre (in mark units) once `ink` of the stroke's total has
 *  been laid down. */
function centre(pr: Profile, ink: number, up: boolean) {
  const { cum } = pr;
  let lo = 0;
  let hi = cum.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (cum[mid] <= ink) lo = mid;
    else hi = mid;
  }
  const f = cum[hi] > cum[lo] ? (ink - cum[lo]) / (cum[hi] - cum[lo]) : 0;
  const travelled = (lo + Math.min(1, Math.max(0, f))) / SCALE - FEATHER / 2;
  return up ? H - travelled : travelled;
}

// The solid part reaches half a unit into the band so their antialiased
// edges don't leave a hairline seam.
function solid(c: number, up: boolean) {
  const t = c - FEATHER / 2;
  return up ? `M0,${t + FEATHER - 0.5}H${MARK_W}V${H + 1}H0Z` : `M0,-1H${MARK_W}V${t + 0.5}H0Z`;
}
const band = (c: number) => `M0,${c - FEATHER / 2}H${MARK_W}V${c + FEATHER / 2}H0Z`;

let profiles: { left: Profile; right: Profile } | null = null;
function getProfiles() {
  profiles ??= { left: profile(LEFT, true), right: profile(RIGHT, false) };
  return profiles;
}

function Stroke({
  d,
  ink,
  pr,
  up,
  id,
}: {
  d: string;
  ink: MotionValue<number>;
  pr: Profile;
  up: boolean;
  id: string;
}) {
  const fill = useTransform(ink, (v) => solid(centre(pr, v, up), up));
  const feather = useTransform(ink, (v) => band(centre(pr, v, up)));
  return (
    <>
      <mask id={id}>
        <motion.path d={fill} fill="#fff" />
        <motion.path d={feather} fill={`url(#${id}-edge)`} />
      </mask>
      <linearGradient id={`${id}-edge`} x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stopColor={up ? "#000" : "#fff"} />
        <stop offset="1" stopColor={up ? "#fff" : "#000"} />
      </linearGradient>
      <path d={d} fill="currentColor" mask={`url(#${id})`} />
    </>
  );
}

export function RMarkFill({
  height = 40,
  delay = 0.15,
  duration = 2,
  className,
}: {
  height?: number;
  delay?: number;
  duration?: number;
  className?: string;
}) {
  const reduce = useReducedMotion();
  const { left, right } = getProfiles();
  const total = left.total + right.total;
  // One ink value for the whole mark; each stroke reads its own share.
  const ink = useMotionValue(reduce ? total : 0);
  const leftInk = useTransform(ink, (v) => Math.min(v, left.total));
  const rightInk = useTransform(ink, (v) => Math.max(0, v - left.total));
  const id = `rmark-fill-${useId().replace(/[^a-zA-Z0-9]/g, "")}`;

  useEffect(() => {
    if (reduce) {
      ink.set(total);
      return;
    }
    // Soft at both ends, close to linear in the middle: the handoff between
    // the strokes sits mid-way and must not be the fastest moment.
    const controls = animate(ink, total, { duration, delay, ease: [0.25, 0.1, 0.75, 0.9] });
    return () => controls.stop();
  }, [reduce, delay, duration, ink, total]);

  return (
    <svg
      viewBox={`0 0 ${MARK_W} ${H}`}
      height={height}
      width={(height * MARK_W) / H}
      aria-hidden="true"
      className={className}
    >
      <Stroke d={LEFT} ink={leftInk} pr={left} up id={`${id}-l`} />
      <Stroke d={RIGHT} ink={rightInk} pr={right} up={false} id={`${id}-r`} />
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
