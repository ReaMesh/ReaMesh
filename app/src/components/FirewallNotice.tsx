import { useState } from "react";
import { ShieldAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";

// Explain the Windows Firewall prompt before it appears under REAPER's name.
// Blocking it prevents discovery and incoming peer connections without an
// error in ReaMesh.
//
// `dismissible` is for the places that recur (the waiting-for-REAPER screen);
// the one-shot onboarding step shows it unconditionally.

const ACK_KEY = "reamesh.firewallAck";

export function FirewallNotice({
  dismissible = false,
}: {
  dismissible?: boolean;
}) {
  const [acked, setAcked] = useState(
    () => dismissible && localStorage.getItem(ACK_KEY) === "1",
  );
  if (!isWindows || acked) return null;

  const ack = () => {
    localStorage.setItem(ACK_KEY, "1");
    setAcked(true);
  };

  return (
    <div className="space-y-2 rounded-lg bg-amber-500/10 p-2.5 text-left">
      <p className="flex items-start gap-2 text-[12px] font-semibold leading-relaxed text-amber-500">
        <ShieldAlert className="mt-0.5 size-3.5 shrink-0" />
        {t("firewallTitle")}
      </p>
      <p className="text-[12px] leading-relaxed text-amber-500/85">
        {t("firewallBody")}
      </p>
      <p className="text-[11px] leading-relaxed text-amber-500/70">
        {t("firewallNamesReaper")}
      </p>
      {dismissible && (
        <Button
          variant="ghost"
          size="sm"
          className="h-6 w-full text-[11px] text-amber-500 hover:bg-amber-500/15 hover:text-amber-500"
          onClick={ack}
        >
          {t("firewallGotIt")}
        </Button>
      )}
    </div>
  );
}
