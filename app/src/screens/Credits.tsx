import { ArrowLeft } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "@/components/ui/button";
import { Divider, Group, SectionLabel } from "@/components/Group";
import { t } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import { cn, focusRing } from "@/lib/utils";
import credits from "@/assets/credits.json";

// Acknowledgements: the projects ReaMesh is built on. A short list of the
// ones that carry it, then every shipped package with its licence, generated
// at build time by scripts/gen-credits.ts. Reached from Settings (and, on
// macOS, from the native settings window's Advanced pane).

interface Credit {
  n: string;
  l: string;
  u?: string;
}

const ALL = credits as Credit[];
const byName = new Map(ALL.map((c) => [c.n, c]));

/** The foundations, under the names their projects go by. */
const BUILT_WITH: { label: string; pkg: string }[] = [
  { label: "Tauri", pkg: "tauri" },
  { label: "rust-libp2p", pkg: "libp2p" },
  { label: "Yrs", pkg: "yrs" },
  { label: "Tokio", pkg: "tokio" },
  { label: "React", pkg: "react" },
  { label: "Motion", pkg: "motion" },
  { label: "Radix UI", pkg: "radix-ui" },
  { label: "Tailwind CSS", pkg: "tailwindcss" },
  { label: "Lucide", pkg: "lucide-react" },
  { label: "BLAKE3", pkg: "blake3" },
  { label: "Zstandard", pkg: "zstd" },
  { label: "nlohmann/json", pkg: "nlohmann/json" },
];

/** "Apache-2.0 OR MIT" reads better as "Apache-2.0 / MIT" in a narrow row. */
const licence = (l: string) => l.replace(/\s+OR\s+/g, " / ").replace(/[()]/g, "");

function CreditRow({ label, c }: { label: string; c?: Credit }) {
  const body = (
    <div className="flex min-h-9 items-center gap-3 px-3 py-1.5">
      <span className="min-w-0 flex-1 truncate text-[13px]">{label}</span>
      {c && (
        <span className="max-w-[55%] shrink-0 truncate text-[12px] text-muted-foreground" title={c.l}>
          {licence(c.l)}
        </span>
      )}
    </div>
  );
  if (!c?.u) return body;
  const url = c.u;
  return (
    <button
      type="button"
      title={url}
      onClick={() => openUrl(url).catch(() => {})}
      className={cn("text-left transition-colors hover:bg-white/[0.03]", focusRing)}
    >
      {body}
    </button>
  );
}

export function Credits({ onBack }: { onBack: () => void }) {
  return (
    <div className={cn("flex flex-1 flex-col overflow-y-auto", isWindows && "scroll-slim")}>
      <div className="w-full space-y-[18px] px-4 pt-1 pb-5">
        <div className="relative flex h-8 items-center">
          <Button
            variant="ghost"
            size="sm"
            className="-ml-1 h-8 gap-1.5 px-2 text-[13px] text-foreground/65 hover:text-foreground"
            onClick={onBack}
          >
            <ArrowLeft className="size-4" aria-hidden="true" />
            {t("back")}
          </Button>
          <h1 className="pointer-events-none absolute inset-0 grid place-items-center text-[15px] font-semibold">
            {t("credits")}
          </h1>
        </div>

        <section className="space-y-2">
          <SectionLabel>{t("builtWith")}</SectionLabel>
          <Group>
            {BUILT_WITH.map((b, i) => (
              <div key={b.pkg} className="flex flex-col">
                {i > 0 && <Divider inset={false} />}
                <CreditRow label={b.label} c={byName.get(b.pkg)} />
              </div>
            ))}
          </Group>
        </section>

        <section className="space-y-2">
          <SectionLabel aside={ALL.length}>{t("allPackages")}</SectionLabel>
          <Group>
            {ALL.map((c, i) => (
              <div key={c.n} className="flex flex-col">
                {i > 0 && <Divider inset={false} />}
                <CreditRow label={c.n} c={c} />
              </div>
            ))}
          </Group>
        </section>
      </div>
    </div>
  );
}
