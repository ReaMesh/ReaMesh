// The ReaMesh mark: four bottom-aligned bars. One geometry everywhere (title
// bar, onboarding, website, icons); only the scale changes. A logo, not a
// meter, so it never animates.
const COLORS = ["#f0846b", "#e8d35c", "#7bd88f", "#5b8def"];
const HEIGHTS = [22, 34, 28, 38];
const BAR = 10;
const GAP = 6;
const W = BAR * 4 + GAP * 3;
const H = 38;

export function BarMark({ height = 11, className }: { height?: number; className?: string }) {
  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      height={height}
      width={(height * W) / H}
      aria-hidden="true"
      className={className}
    >
      {COLORS.map((c, i) => (
        <rect
          key={c}
          x={i * (BAR + GAP)}
          y={H - HEIGHTS[i]}
          width={BAR}
          height={HEIGHTS[i]}
          rx={BAR / 2}
          fill={c}
        />
      ))}
    </svg>
  );
}
