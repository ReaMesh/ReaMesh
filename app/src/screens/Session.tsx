import { useEffect, useState } from "react";
import { AnimatePresence, motion, useAnimate } from "motion/react";
import {
  Ban,
  Check,
  Crown,
  Download,
  Eye,
  EyeOff,
  Snowflake,
  Ticket,
  TriangleAlert,
  UserX,
} from "lucide-react";
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
import { BanList } from "@/components/BanList";
import { Hint, HintProvider } from "@/components/Hint";
import { PeerAvatar } from "@/components/PeerAvatar";
import { Rolling, TrackNumber } from "@/components/Rolling";
import { send } from "@/lib/connection";
import { formatBytes, useTransfer } from "@/lib/format";
import { forgetHostPass, getHostPass } from "@/lib/hostPass";
import { resolveLang, t } from "@/lib/i18n";
import { displayName, type ExtState, type Peer } from "@/lib/types";
import { LinkNotice } from "@/components/LinkNotice";
import { cn, focusRing } from "@/lib/utils";

// Forward the ref so popLayout can measure and remove an exiting row from flow.
function PeerRow({
  ref,
  peer,
  flash,
  port,
  onKick,
  onBan,
}: {
  ref?: React.Ref<HTMLDivElement>;
  peer: Peer;
  flash?: boolean;
  port: number | null;
  /** Host-only moderation; both absent on a guest's roster and on the self row. */
  onKick?: () => void;
  onBan?: () => void;
}) {
  const moderate = !peer.is_self && !!onKick && !!onBan;
  // Click-to-follow, same as the overlay HUD: only meaningful for someone
  // else who's actually on a track.
  const clickable = !peer.is_self && peer.track != null && peer.track > 0;
  const reveal = () => send({ cmd: "reveal_peer", peer_id: peer.id }, port ?? undefined);
  // Briefly tint peers who joined while the window was unfocused.
  const [flashScope, animateFlash] = useAnimate();
  useEffect(() => {
    if (!flash || !flashScope.current) return;
    const tint = /^#[0-9a-fA-F]{6}$/.test(peer.color)
      ? [`${peer.color}40`, `${peer.color}00`]
      : ["rgba(255,255,255,0.12)", "rgba(255,255,255,0)"];
    animateFlash(
      flashScope.current,
      { backgroundColor: tint },
      { duration: 2, ease: "easeOut" },
    );
  }, [flash, peer.color, flashScope, animateFlash]);
  return (
    <Hint label={clickable ? t("revealPeerTrack", { name: displayName(peer) }) : undefined}>
      <motion.div
        ref={ref}
        layout
        initial={{ opacity: 0, x: -10 }}
        animate={{ opacity: 1, x: 0 }}
        exit={{ opacity: 0, x: 10 }}
        transition={{ type: "spring", stiffness: 350, damping: 28 }}
        onClick={clickable ? reveal : undefined}
        onKeyDown={
          clickable
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  reveal();
                }
              }
            : undefined
        }
        role={clickable ? "button" : undefined}
        tabIndex={clickable ? 0 : undefined}
        className={cn(
          "group relative flex items-center gap-3 rounded-lg px-2 py-1.5",
          clickable && "cursor-pointer transition-colors hover:bg-secondary/60",
        )}
      >
        <span
          ref={flashScope}
          aria-hidden
          className="pointer-events-none absolute inset-0 rounded-lg"
        />
        <PeerAvatar peer={peer} />
        <span className="min-w-0 flex-1 truncate text-[13px] font-medium">
          {displayName(peer)}
        </span>
        {peer.frozen && !peer.is_self && (
          // The peer is online, but their REAPER has stopped responding.
          <Hint label={t("reaperNotRespondingPeer")}>
            <span className="flex shrink-0 items-center">
              <Snowflake className="size-3.5 text-sky-300" aria-hidden="true" />
              <span className="sr-only">{t("reaperNotRespondingPeer")}</span>
            </span>
          </Hint>
        )}
        {peer.is_owner && (
          // The crown needs real text for the host to be identifiable non-visually.
          <Hint label={t("sessionHost")}>
            <span className="flex shrink-0 items-center">
              <Crown className="size-3.5 text-amber-400" aria-hidden="true" />
              <span className="sr-only">{t("sessionHost")}</span>
            </span>
          </Hint>
        )}
        {peer.is_self && (
          <Badge variant="secondary" className="px-1.5 py-0 text-[10px]">
            {t("youBadge")}
          </Badge>
        )}
        {moderate && (
          // Show moderation controls on hover or keyboard focus. Kicks take effect
          // immediately; the parent confirms blocks before sending them.
          <span
            className="mr-1 flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 has-[:focus-visible]:opacity-100"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => e.stopPropagation()}
          >
            <Hint label={t("kickPeer", { name: displayName(peer) })}>
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-muted-foreground hover:text-foreground"
                aria-label={t("kickPeer", { name: displayName(peer) })}
                onClick={onKick}
              >
                <UserX aria-hidden="true" />
              </Button>
            </Hint>
            <Hint label={t("banPeer", { name: displayName(peer) })}>
              <Button
                variant="ghost"
                size="icon-xs"
                className="text-muted-foreground hover:text-red-400"
                aria-label={t("banPeer", { name: displayName(peer) })}
                onClick={onBan}
              >
                <Ban aria-hidden="true" />
              </Button>
            </Hint>
          </span>
        )}
        <TrackNumber track={peer.track} />
      </motion.div>
    </Hint>
  );
}

// Copy the invite link to the clipboard: the async clipboard API where the
// webview grants it, with the hidden-textarea fallback where it doesn't.
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      const ok = document.execCommand("copy");
      ta.remove();
      return ok;
    } catch {
      return false;
    }
  }
}

// Copy an invite link for hosts outside mDNS reach, such as peers on a VPN.
function InviteTicket({ ticket }: { ticket: string }) {
  const [copied, setCopied] = useState(false);
  // Reveal the link for manual selection if clipboard access fails.
  const [reveal, setReveal] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(false), 2500);
    return () => clearTimeout(id);
  }, [copied]);
  if (reveal) {
    return (
      <div className="flex shrink-0 flex-col gap-1 self-stretch px-1">
        <span className="text-[11px] text-muted-foreground">{t("copyInviteFailed")}</span>
        <textarea
          readOnly
          autoFocus
          aria-label={t("copyInvite")}
          value={ticket}
          onFocus={(e) => e.currentTarget.select()}
          className={cn(
            "h-14 w-full resize-none rounded-lg bg-secondary p-2 font-mono text-[10px] leading-tight break-all",
            focusRing,
          )}
        />
      </div>
    );
  }
  return (
    <Hint label={t("inviteTicketHint")} side="top">
      <button
        onClick={async () => {
          if (await copyText(ticket)) setCopied(true);
          else setReveal(true);
        }}
        className={cn(
          "flex shrink-0 items-center gap-1.5 self-start rounded px-1 text-[11px] text-muted-foreground transition-colors hover:text-foreground",
          focusRing,
        )}
      >
        <span className="relative size-3 shrink-0">
          <AnimatePresence initial={false}>
            <motion.span
              key={copied ? "check" : "ticket"}
              className="absolute inset-0 grid place-items-center"
              initial={{ scale: 0.4, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              exit={{ scale: 0.4, opacity: 0 }}
              transition={{ type: "spring", stiffness: 500, damping: 30 }}
            >
              {copied ? (
                <Check className="size-3" aria-hidden="true" />
              ) : (
                <Ticket className="size-3" aria-hidden="true" />
              )}
            </motion.span>
          </AnimatePresence>
        </span>
        <span>{copied ? t("inviteCopied") : t("copyInvite")}</span>
      </button>
    </Hint>
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
      aria-expanded={shown}
      className={cn(
        "flex shrink-0 items-center gap-1.5 self-start rounded px-1 text-[11px] text-muted-foreground transition-colors hover:text-foreground",
        focusRing,
      )}
    >
      {shown ? (
        <EyeOff className="size-3" aria-hidden="true" />
      ) : (
        <Eye className="size-3" aria-hidden="true" />
      )}
      {shown ? (
        <span className="font-mono">{pass}</span>
      ) : (
        <span>{t("showPassword")}</span>
      )}
    </button>
  );
}

// Show background media-transfer progress until the project is fully received.
function LoadingNotice({ state }: { state: ExtState }) {
  const lang = resolveLang();
  const done = state.done_bytes || 0;
  const total = state.total_bytes || 0;
  const pct = total > 0 ? Math.min(100, Math.floor((done / total) * 100)) : 0;
  const { rate } = useTransfer(done, total);
  const filesTotal = state.files_total || 0;
  const detail = [
    filesTotal > 0 ? t("loadingFilesOf", { done: state.files_done || 0, total: filesTotal }) : null,
    total > 0 ? `${formatBytes(done, lang)} / ${formatBytes(total, lang)}` : null,
    rate > 0 ? `${formatBytes(rate, lang)}/s` : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: -4 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -4 }}
      className="flex flex-col gap-1.5 rounded-xl bg-card px-3 py-2.5"
      role="status"
      aria-live="polite"
    >
      <div className="flex items-center gap-2 text-[12px]">
        <Download className="size-3.5 shrink-0 text-found" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate font-medium">{t("loadingMedia")}</span>
        <span className="shrink-0 tabular-nums text-muted-foreground">{pct}%</span>
      </div>
      <div
        className="h-1 overflow-hidden rounded-full bg-secondary"
        role="progressbar"
        aria-label={t("loadingMedia")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
      >
        <motion.div
          className="h-full rounded-full bg-found"
          initial={false}
          animate={{ width: `${pct}%` }}
          transition={{ type: "spring", stiffness: 120, damping: 26 }}
        />
      </div>
      <p className="truncate text-[11px] text-muted-foreground">
        {pct < 100 && detail ? `${detail} · ${t("loadingEditsLocked")}` : t("loadingEditsLocked")}
      </p>
    </motion.div>
  );
}

export function Session({
  state,
  port,
  flashPeers = [],
}: {
  state: ExtState;
  port: number | null;
  /** Peers to highlight once after the user returns to the window. */
  flashPeers?: string[];
}) {
  const hosting = state.mode === "hosting";
  const [confirmStop, setConfirmStop] = useState(false);
  // Guest the host is about to block (the confirm dialog's subject).
  const [banTarget, setBanTarget] = useState<Peer | null>(null);
  const kick = (p: Peer) => send({ cmd: "kick", peer_id: p.id }, port ?? undefined);
  const ban = () => {
    if (banTarget) send({ cmd: "ban", peer_id: banTarget.id }, port ?? undefined);
    setBanTarget(null);
  };
  // Drop the confirm if its subject leaves on their own meanwhile.
  useEffect(() => {
    if (banTarget && !state.peers.some((p) => p.id === banTarget.id)) setBanTarget(null);
  }, [banTarget, state.peers]);
  const others = state.peers.length - 1;
  const hostPass = hosting ? getHostPass(port) : "";
  const leave = () => {
    setConfirmStop(false);
    forgetHostPass(port);
    send({ cmd: "leave" }, port ?? undefined);
  };
  // Confirm only when stopping the session would disconnect other people.
  const requestLeave = () => {
    if (hosting && others > 0) setConfirmStop(true);
    else leave();
  };
  // Close the confirmation if the last guest leaves while it is open.
  useEffect(() => {
    if (confirmStop && others <= 0) setConfirmStop(false);
  }, [confirmStop, others]);

  return (
    <HintProvider>
    <div className="flex min-h-0 flex-1 flex-col gap-4 p-4">
      <div className="flex items-center gap-3">
        <h2 className="min-w-0 flex-1 truncate text-[15px] font-semibold tracking-tight">
          {state.session || t("sessionFallback")}
        </h2>
        {/* Explain why edits are not sent in view-only mode. */}
        {state.nofx && (
          <Hint label={t("nofxBadgeHint")}>
            <Badge
              variant="outline"
              className="shrink-0 border-amber-500/40 text-amber-500"
            >
              {t("nofxBadge")}
            </Badge>
          </Hint>
        )}
        {/* Progressive join: still receiving the project; edits snap back. */}
        {!hosting && state.loading && (
          <Hint label={t("loadingBadgeHint")}>
            <Badge variant="outline" className="shrink-0 border-found/40 text-found">
              {t("loadingMediaShort")}
            </Badge>
          </Hint>
        )}
        <Badge variant={hosting ? "default" : "secondary"} className="shrink-0">
          {hosting ? t("hostingBadge") : t("connectedBadge")}
        </Badge>
      </div>

      <AnimatePresence initial={false}>
        {!hosting && state.loading && <LoadingNotice key="loading" state={state} />}
      </AnimatePresence>

      <LinkNotice peers={state.peers} hosting={hosting} />

      <div className="flex min-h-0 flex-col gap-2">
        <h3 className="px-1 text-[12px] font-medium text-muted-foreground">
          {/* The count rolls slot-machine style when someone joins/leaves. */}
          <Rolling
            text={
              state.peers.length === 1
                ? t("justYou")
                : t("peopleInSession", { count: state.peers.length })
            }
          />
        </h3>
        <ScrollArea
          className="min-h-0 rounded-xl bg-card p-2"
          aria-label={t("peopleList")}
        >
          {/* Animate later roster changes, but not the initial peer list. */}
          <AnimatePresence mode="popLayout" initial={false}>
            {state.peers.map((p) => (
              <PeerRow
                key={p.id}
                peer={p}
                flash={flashPeers.includes(p.id)}
                port={port}
                onKick={hosting ? () => kick(p) : undefined}
                onBan={hosting ? () => setBanTarget(p) : undefined}
              />
            ))}
          </AnimatePresence>
        </ScrollArea>
      </div>

      {/* Absorbs slack so the peer card hugs its content and the Leave
          button stays pinned to the bottom. */}
      <div className="min-h-0 flex-1" />

      {(hostPass || (hosting && (!!state.ticket || !!state.bans?.length))) && (
        <div className="flex items-end gap-2">
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            {hosting && !!state.ticket && <InviteTicket ticket={state.ticket} />}
            {hostPass && <HostPassword pass={hostPass} />}
          </div>
          {hosting && <BanList bans={state.bans ?? []} port={port} variant="icon" />}
        </div>
      )}

      <Button variant="secondary" className="shrink-0" onClick={requestLeave}>
        {hosting ? t("stopHosting") : t("leaveSession")}
      </Button>

      <Dialog open={banTarget !== null} onOpenChange={(o) => !o && setBanTarget(null)}>
        <DialogContent className="max-w-[340px] gap-4 rounded-2xl">
          <DialogHeader>
            <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-red-500/15 sm:mx-0">
              <Ban className="size-5 text-red-400" aria-hidden="true" />
            </div>
            <DialogTitle className="text-[15px]">
              {t("banTitle", { name: banTarget ? displayName(banTarget) : "" })}
            </DialogTitle>
            <DialogDescription className="text-[13px] leading-relaxed">
              {t("banBody")}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2 sm:gap-2">
            <Button variant="outline" className="flex-1" onClick={() => setBanTarget(null)}>
              {t("cancel")}
            </Button>
            <Button variant="destructive" className="flex-1" onClick={ban}>
              {t("banConfirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={confirmStop} onOpenChange={setConfirmStop}>
        <DialogContent className="max-w-[340px] gap-4 rounded-2xl">
          <DialogHeader>
            <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-amber-500/15 sm:mx-0">
              <TriangleAlert className="size-5 text-amber-500" aria-hidden="true" />
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
    </HintProvider>
  );
}
