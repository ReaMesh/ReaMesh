import { useEffect, useState } from "react";
import { ChevronRight, ShieldAlert, ShieldX } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import { cn, focusRing } from "@/lib/utils";

// Explain the Windows Firewall prompt before it appears under REAPER's name.
// Blocking it prevents discovery and incoming peer connections without an
// error in ReaMesh.
//
// `dismissible` is for the places that recur (the waiting-for-REAPER screen);
// the one-shot onboarding step shows it unconditionally.

const ACK_KEY = "reamesh.firewallAck";

/** Build the admin PowerShell command that replaces active REAPER block rules
 *  with an allow rule for private networks. */
export function firewallFixCommand(exe: string): string {
  return (
    `Get-NetFirewallApplicationFilter -Program "${exe}" | Get-NetFirewallRule | ` +
    `Where-Object Action -eq Block | Remove-NetFirewallRule; ` +
    `New-NetFirewallRule -DisplayName "REAPER" -Direction Inbound -Program "${exe}" ` +
    `-Action Allow -Profile Private`
  );
}

/** Show the repair command when Windows has an active inbound block rule for
 *  REAPER. A later allow rule cannot override that rule. */
export function FirewallBlocked() {
  // undefined = not checked yet, null = no block, string = blocked exe path
  // (may be "" when the rule carried no App segment).
  const [exe, setExe] = useState<string | null | undefined>(undefined);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!isWindows) return;
    invoke<string | null>("firewall_block")
      .then((v) => setExe(v ?? null))
      .catch(() => setExe(null));
  }, []);
  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(false), 4000);
    return () => clearTimeout(id);
  }, [copied]);
  if (!isWindows || exe === undefined || exe === null) return null;

  const copyFix = async () => {
    try {
      await navigator.clipboard.writeText(firewallFixCommand(exe));
      setCopied(true);
    } catch {
      // Clipboard denied; the settings button still works.
    }
  };

  return (
    <div role="alert" className="space-y-2 rounded-xl bg-red-500/10 p-3 text-left">
      <p className="flex items-start gap-2 text-[12px] font-semibold leading-relaxed text-red-400">
        <ShieldX className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
        {t("fwBlockedTitle")}
      </p>
      <p className="text-[12px] leading-relaxed text-red-400/90">{t("fwBlockedBody")}</p>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="ghost"
          size="sm"
          className="h-6 px-2 text-[11px] text-red-400 hover:bg-red-500/15 hover:text-red-300"
          onClick={() => invoke("open_firewall_settings").catch(() => {})}
        >
          {t("fwOpenSettings")}
        </Button>
        {exe && (
          <Button
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-[11px] text-red-400 hover:bg-red-500/15 hover:text-red-300"
            onClick={copyFix}
          >
            {copied ? t("fwFixCopied") : t("fwCopyFix")}
          </Button>
        )}
      </div>
    </div>
  );
}

export function FirewallNotice({
  dismissible = false,
}: {
  dismissible?: boolean;
}) {
  const [acked, setAcked] = useState(
    () => dismissible && localStorage.getItem(ACK_KEY) === "1",
  );
  const [showRecover, setShowRecover] = useState(false);
  if (!isWindows || acked) return null;

  const ack = () => {
    localStorage.setItem(ACK_KEY, "1");
    setAcked(true);
  };

  return (
    <div className="space-y-2 rounded-lg bg-amber-500/10 p-2.5 text-left">
      <p className="flex items-start gap-2 text-[12px] font-semibold leading-relaxed text-amber-500">
        <ShieldAlert className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
        {t("firewallTitle")}
      </p>
      {/* Keep body text above the 4.5:1 contrast target. */}
      <p className="text-[12px] leading-relaxed text-amber-500">{t("firewallBody")}</p>
      <p className="text-[11px] leading-relaxed text-amber-500">
        {t("firewallNamesReaper")}
      </p>
      {/* Keep the recovery steps collapsed until they are needed. */}
      <button
        type="button"
        aria-expanded={showRecover}
        onClick={() => setShowRecover((v) => !v)}
        className={cn(
          "flex w-full items-center gap-1 rounded text-left text-[11px] font-medium text-amber-500/90 underline-offset-2 hover:underline",
          focusRing,
        )}
      >
        <ChevronRight
          className={cn("size-3 shrink-0 transition-transform", showRecover && "rotate-90")}
          aria-hidden="true"
        />
        {t("firewallAlreadyDismissed")}
      </button>
      {showRecover && (
        <p className="pl-4 text-[11px] leading-relaxed text-amber-500">
          {t("firewallRecover")}
        </p>
      )}
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
