import { useEffect } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFact,
  DialogFacts,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { t } from "@/lib/i18n";
import { MAX_RELAYS, parseRelayCode, relayHost } from "@/lib/relay";
import { clearRelayLink, useRelayLink } from "@/lib/relayLink";
import { addRelay, useRelays } from "@/lib/relays";

// Asks before a relay code that arrived as a link goes into Settings → Relay.
// A link can come from anywhere, so it is never added on its own: the dialog
// names the relay and where it is, and only Add stores it.
export function RelayLinkDialog({ hosting }: { hosting: boolean }) {
  const link = useRelayLink();
  const relays = useRelays();
  const relay = link ? parseRelayCode(link) : null;
  // What can't be added isn't asked about, only said.
  const refusal = !link
    ? ""
    : !relay
      ? t("relayInvalid")
      : relays.some((r) => r.peer === relay.peer)
        ? t("relayDuplicate")
        : relays.length >= MAX_RELAYS
          ? t("relayListFull", { max: MAX_RELAYS })
          : "";
  useEffect(() => {
    if (!refusal) return;
    toast(refusal);
    clearRelayLink();
  }, [refusal]);

  const open = !!relay && !refusal;

  const add = async () => {
    clearRelayLink();
    if (!relay) return;
    if (await addRelay(relay.code)) toast(hosting ? t("relayNextHost") : t("relayAdded"));
    else toast(t("relayDuplicate"));
  };

  return (
    <Dialog open={open} onOpenChange={(o) => !o && clearRelayLink()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("relayLinkTitle")}</DialogTitle>
          <DialogDescription>{t("relayLinkSub")}</DialogDescription>
        </DialogHeader>
        {relay && (
          <DialogFacts>
            {relay.label && <DialogFact label={t("relayLinkName")}>{relay.label}</DialogFact>}
            <DialogFact label={t("relayLinkAddress")}>
              <span className="truncate">{relayHost(relay)}</span>
            </DialogFact>
          </DialogFacts>
        )}
        {/* Said by the code itself: asking the relay would mean dialing it, and
            a link alone must not make the app contact anyone. */}
        {relay?.list && (
          <p className="text-[12.5px] leading-normal text-muted-foreground">{t("relayLinkList")}</p>
        )}
        <DialogFooter>
          <Button variant="secondary" onClick={clearRelayLink}>
            {t("cancel")}
          </Button>
          <Button onClick={() => add().catch((e) => toast.error(String(e)))}>
            {t("relayAddShort")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
