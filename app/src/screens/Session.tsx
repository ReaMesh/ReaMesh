import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { Crown, Eye, EyeOff, TriangleAlert } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { PeerAvatar } from "@/components/PeerAvatar";
import { send } from "@/lib/connection";
import { forgetHostPass, getHostPass } from "@/lib/hostPass";
import { t } from "@/lib/i18n";
import { displayName, type ExtState, type Peer } from "@/lib/types";

function PeerRow({ peer }: { peer: Peer }) {
  return (
    <motion.div
      layout
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0 }}
      exit={{ opacity: 0, x: 10 }}
      transition={{ type: "spring", stiffness: 350, damping: 28 }}
      className="flex items-center gap-3 rounded-lg px-2 py-1.5"
    >
      <PeerAvatar peer={peer} />
      <span className="min-w-0 flex-1 truncate text-[13px] font-medium">
        {displayName(peer)}
      </span>
      {peer.is_owner && (
        <span title={t("sessionHost")}>
          <Crown className="size-3.5 text-amber-400" />
        </span>
      )}
      {peer.is_self && (
        <Badge variant="secondary" className="px-1.5 py-0 text-[10px]">
          {t("youBadge")}
        </Badge>
      )}
    </motion.div>
  );
}

// Reveal the host's session passphrase temporarily, then mask it again.
function HostPassword({ pass }: { pass: string }) {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    if (!shown) return;
    const id = setTimeout(() => setShown(false), 6000);
    return () => clearTimeout(id);
  }, [shown]);
  return (
    <button
      onClick={() => setShown((v) => !v)}
      className="flex shrink-0 items-center gap-1.5 self-start px-1 text-[11px] text-muted-foreground transition-colors hover:text-foreground"
    >
      {shown ? <EyeOff className="size-3" /> : <Eye className="size-3" />}
      {shown ? (
        <span className="font-mono">{pass}</span>
      ) : (
        <span>{t("showPassword")}</span>
      )}
    </button>
  );
}

export function Session({
  state,
  port,
}: {
  state: ExtState;
  port: number | null;
}) {
  const hosting = state.mode === "hosting";
  const [confirmStop, setConfirmStop] = useState(false);
  const others = state.peers.length - 1;
  const hostPass = hosting ? getHostPass(port) : "";
  const leave = () => {
    setConfirmStop(false);
    forgetHostPass(port);
    send({ cmd: "leave" });
  };
  // Confirm only when stopping the session would disconnect other people.
  const requestLeave = () => {
    if (hosting && others > 0) setConfirmStop(true);
    else leave();
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 p-4">
      <div className="flex items-center gap-3">
        <h2 className="min-w-0 flex-1 truncate text-[15px] font-semibold tracking-tight">
          {state.session || t("sessionFallback")}
        </h2>
        <Badge variant={hosting ? "default" : "secondary"} className="shrink-0">
          {hosting ? t("hostingBadge") : t("connectedBadge")}
        </Badge>
      </div>

      <div className="flex min-h-0 flex-col gap-2">
        <h3 className="px-1 text-[12px] font-medium text-muted-foreground">
          {state.peers.length === 1
            ? t("justYou")
            : t("peopleInSession", { count: state.peers.length })}
        </h3>
        <ScrollArea className="min-h-0 rounded-xl bg-card p-2">
          {/* initial={false} suppresses entrance animations for people
              already present when the screen mounts (they fade in with the
              whole screen), while later join/leave still animate. */}
          <AnimatePresence mode="popLayout" initial={false}>
            {state.peers.map((p) => (
              <PeerRow key={p.id} peer={p} />
            ))}
          </AnimatePresence>
        </ScrollArea>
      </div>

      {/* Absorbs slack so the peer card hugs its content and the Leave
          button stays pinned to the bottom. */}
      <div className="min-h-0 flex-1" />

      {hostPass && <HostPassword pass={hostPass} />}

      <Button variant="secondary" className="shrink-0" onClick={requestLeave}>
        {hosting ? t("stopHosting") : t("leaveSession")}
      </Button>

      <Dialog open={confirmStop} onOpenChange={setConfirmStop}>
        <DialogContent className="max-w-[340px] gap-4 rounded-2xl">
          <DialogHeader>
            <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-amber-500/15 sm:mx-0">
              <TriangleAlert className="size-5 text-amber-500" />
            </div>
            <DialogTitle className="text-[15px]">{t("stopHostingTitle")}</DialogTitle>
            <DialogDescription className="text-[13px] leading-relaxed">
              {others === 1
                ? t("stopHostingBodyOne")
                : t("stopHostingBodyMany", { count: others })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2 sm:gap-2">
            <Button
              variant="outline"
              className="flex-1"
              onClick={() => setConfirmStop(false)}
            >
              {t("cancel")}
            </Button>
            <Button className="flex-1" onClick={leave}>
              {t("stopHosting")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
