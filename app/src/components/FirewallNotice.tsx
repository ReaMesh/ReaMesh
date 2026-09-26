import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
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
    <div role="alert" className="flex shrink-0 flex-col gap-2 rounded-xl bg-card p-3">
      <p className="text-[12px] leading-normal text-red-400">{t("fwBlocked")}</p>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="secondary"
          size="sm"
          className="h-7 px-2.5 text-[12px]"
          onClick={() => invoke("open_firewall_settings").catch(() => {})}
        >
          {t("fwOpenSettings")}
        </Button>
        {exe && (
          <Button variant="ghost" size="sm" className="h-7 px-2.5 text-[12px]" onClick={copyFix}>
            {copied ? t("fwFixCopied") : t("fwCopyFix")}
          </Button>
        )}
      </div>
    </div>
  );
}

/** Two sentences about the prompt. Why it names REAPER, and how to undo a
 *  dismissed one, wait behind the "Closed the prompt already?" link. */
export function FirewallNotice({
  dismissible = false,
  className,
}: {
  dismissible?: boolean;
  className?: string;
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
  const link = "rounded text-[11px] transition-colors hover:text-foreground";

  return (
    // Margins instead of a flex gap: the recovery text must bring its own
    // spacing with it as it unfolds, not before.
    <div className={cn("flex flex-col text-left", className)}>
      <p className="text-[12px] leading-normal text-foreground/65">{t("firewallBody")}</p>
      <div className="mt-2.5 flex gap-4">
        {dismissible && (
          <button type="button" onClick={ack} className={cn(link, "font-semibold text-foreground", focusRing)}>
            {t("firewallGotIt")}
          </button>
        )}
        <button
          type="button"
          aria-expanded={showRecover}
          onClick={() => setShowRecover((v) => !v)}
          className={cn(link, "text-muted-foreground", focusRing)}
        >
          {t("firewallAlreadyDismissed")}
        </button>
      </div>
      <AnimatePresence initial={false}>
        {showRecover && (
          <motion.div
            key="recover"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ type: "spring", stiffness: 400, damping: 38 }}
            className="overflow-hidden"
          >
            <p className="pt-2.5 text-[11px] leading-normal text-muted-foreground">
              {t("firewallNamesReaper")} {t("firewallRecover")}
            </p>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
