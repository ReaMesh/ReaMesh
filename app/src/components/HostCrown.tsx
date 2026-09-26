import { Crown } from "lucide-react";
import { Hint } from "@/components/Hint";
import { t } from "@/lib/i18n";

/** The crown right after the host's name, with real text so the host is
 *  identifiable non-visually. `hint` adds the tooltip, which needs a
 *  HintProvider above it. */
export function HostCrown({ hint = true }: { hint?: boolean }) {
  const crown = (
    // Centred even in a row that lines its words up on their baseline.
    <span className="inline-flex shrink-0 items-center self-center">
      <Crown className="size-3 text-amber-400" aria-hidden="true" />
      <span className="sr-only">{t("sessionHost")}</span>
    </span>
  );
  return hint ? <Hint label={t("sessionHost")}>{crown}</Hint> : crown;
}
