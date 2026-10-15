import { useEffect, useState } from "react";
import { AnimatePresence, motion, useAnimate } from "motion/react";
import { Ban, Check, Copy, Ellipsis, Eye, EyeOff, LocateFixed, UserX } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger } from "@/components/ui/menu";
import { Switch } from "@/components/ui/switch";
import { BanList } from "@/components/BanList";
import { Divider, Group, SectionLabel } from "@/components/Group";
import { HintProvider } from "@/components/Hint";
import { HostCrown } from "@/components/HostCrown";
import { useSlowLink } from "@/components/LinkNotice";
import { PeerAvatar } from "@/components/PeerAvatar";
import { TrackNumber } from "@/components/TrackNumber";
import { send } from "@/lib/connection";
import { forgetHostPass, getHostPass } from "@/lib/hostPass";
import { nearbyBlocked, setNearby, useNearby } from "@/lib/nearby";
import { t } from "@/lib/i18n";
import { displayName, type ExtState, type Peer } from "@/lib/types";
import { cn, focusRing, noContextMenu } from "@/lib/utils";

function editingLabel(e: NonNullable<Peer["editing"]>): string {
  if (e.kind === "track") return e.name ? t("quoted", { text: e.name }) : t("trackN", { n: e.n ?? 0 });
  if (e.kind !== "project") return ""; // unknown shape from a newer extension
  if (e.field === "tempo") return t("editingTempo");
  if (e.field === "markers") return t("editingMarkers");
  return t("editingProject");
}

/** What someone else is doing right now, as one line of text, most urgent
 *  first. Your own row has none: who follows you is said at their row. */
function PeerMeta({
  peer,
  peers,
  hosting,
  following,
}: {
  peer: Peer;
  peers: Peer[];
  hosting: boolean;
  following: boolean;
}) {
  // The link that matters: a guest's to the host, the host's to each guest.
  const rtt = !peer.is_self && (hosting || peer.is_owner) ? (peer.rtt ?? 0) : 0;
  const slow = useSlowLink(rtt);
  if (peer.is_self) return null;

  let text = "";
  let tone = "text-muted-foreground";
  let title: string | undefined;
  if (peer.frozen) {
    text = t("reaperNotResponding");
    tone = "text-amber-500";
    title = t("reaperNotRespondingPeer");
  } else if (slow) {
    text = t("slowLink");
    tone = "text-amber-500";
    title = t("slowLinkRtt", { ms: Math.round(rtt) });
  } else if (following) {
    text = t("followingShort");
    tone = "text-found";
  } else if (peer.editing && editingLabel(peer.editing)) {
    text = t("peerEditing", { what: editingLabel(peer.editing) });
  } else if (peer.follow) {
    const target = peers.find((p) => p.id === peer.follow);
    if (target) {
      text = target.is_self ? t("followsYou") : t("followsName", { name: displayName(target) });
      if (target.is_self) tone = "text-found";
    }
  }
  if (!text) return null;
  return (
    <p className={cn("truncate text-[12px]", tone)} title={title}>
      {text}
    </p>
  );
}

// Forward the ref so popLayout can measure and remove an exiting row from flow.
function PeerRow({
  ref,
  peer,
  peers,
  hosting,
  flash,
  port,
  following,
  onKick,
  onBan,
}: {
  ref?: React.Ref<HTMLDivElement>;
  peer: Peer;
  peers: Peer[];
  hosting: boolean;
  flash?: boolean;
  port: number | null;
  /** This user currently follows this peer. */
  following: boolean;
  /** Host-only moderation; both absent on a guest's roster and on the self row. */
  onKick?: () => void;
  onBan?: () => void;
}) {
  const name = displayName(peer);
  const onTrack = peer.track != null && peer.track > 0;
  // Click-to-jump, same as the overlay HUD: only meaningful for someone else
  // who's actually on a track.
  const clickable = !peer.is_self && onTrack;
  const reveal = () => send({ cmd: "reveal_peer", peer_id: peer.id }, port ?? undefined);
  const follow = (on: boolean) =>
    send({ cmd: "follow", peer_id: on ? peer.id : "" }, port ?? undefined);
  // Briefly tint peers who joined while the window was unfocused.
  const [flashScope, animateFlash] = useAnimate();
  useEffect(() => {
    if (!flash || !flashScope.current) return;
    const tint = /^#[0-9a-fA-F]{6}$/.test(peer.color)
      ? [`${peer.color}40`, `${peer.color}00`]
      : ["rgba(255,255,255,0.12)", "rgba(255,255,255,0)"];
    animateFlash(flashScope.current, { backgroundColor: tint }, { duration: 2, ease: "easeOut" });
  }, [flash, peer.color, flashScope, animateFlash]);
  return (
    <motion.div
      ref={ref}
      layout="position"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.2, ease: "easeOut" }}
      onClick={clickable ? reveal : undefined}
      title={clickable ? t("revealPeerTrack", { name }) : undefined}
      className={cn(
        "relative flex min-h-[52px] items-center gap-3 px-3 py-2",
        clickable && "cursor-pointer transition-colors hover:bg-white/[0.03]",
      )}
    >
      <span ref={flashScope} aria-hidden className="pointer-events-none absolute inset-0" />
      <PeerAvatar peer={peer} />
      <div className="flex min-w-0 flex-1 flex-col gap-px">
        <div className="flex min-w-0 items-baseline gap-1.5">
          <span className="truncate text-[13px] font-semibold">{name}</span>
          {peer.is_self && (
            <span className="shrink-0 text-[12px] text-muted-foreground">{t("youBadge")}</span>
          )}
          {peer.is_owner && <HostCrown />}
        </div>
        <PeerMeta peer={peer} peers={peers} hosting={hosting} following={following} />
      </div>
      {/* The track as plain text, right-aligned in one column; your own row
          keeps the menu's width empty so it lines up too. */}
      <div
        className="flex shrink-0 items-center gap-3"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <TrackNumber track={peer.track} />
        {peer.is_self ? (
          <span aria-hidden="true" className="w-7 shrink-0" />
        ) : (
          <Menu>
            <MenuTrigger asChild>
              <button
                type="button"
                aria-label={t("actionsFor", { name })}
                className={cn(
                  "grid size-7 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-white/[0.07] hover:text-foreground data-[state=open]:bg-white/[0.07]",
                  focusRing,
                )}
              >
                <Ellipsis className="size-4" aria-hidden="true" />
              </button>
            </MenuTrigger>
            <MenuContent onContextMenu={noContextMenu}>
              <MenuItem onSelect={() => follow(!following)}>
                {following ? <EyeOff /> : <Eye />}
                {following ? t("stopFollowing") : t("followAction")}
              </MenuItem>
              <MenuItem disabled={!onTrack} onSelect={reveal}>
                <LocateFixed />
                {t("jumpToTrack")}
              </MenuItem>
              {onKick && onBan && (
                <>
                  <MenuSeparator />
                  <MenuItem onSelect={onKick}>
                    <UserX />
                    {t("kickAction")}
                  </MenuItem>
                  <MenuItem destructive onSelect={onBan}>
                    <Ban />
                    {t("banAction")}
                  </MenuItem>
                </>
              )}
            </MenuContent>
          </Menu>
        )}
      </div>
    </motion.div>
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

/** A row of the "Invite" group: content on the left, one action on the right. */
// Bluetooth for guests nearby whose network blocks the usual search (guest
// Wi-Fi). Only this host's network addresses are advertised; the switch is the
// shared preference, and turning it on is what asks macOS for the permission.
function NearbyRow() {
  const n = useNearby();
  const on = n.enabled === true;
  return (
    <InviteRow action={<Switch checked={on} onChange={setNearby} label={t("nearbyHostLabel")} />}>
      <p className="truncate text-[13px]" title={t("nearbyHostHint")}>
        {t("nearbyHostLabel")}
      </p>
      {on && nearbyBlocked(n) && (
        <p className="truncate text-[12px] text-amber-500">{t("nearbyBlocked")}</p>
      )}
    </InviteRow>
  );
}

function InviteRow({ children, action }: { children: React.ReactNode; action: React.ReactNode }) {
  return (
    <div className="flex min-h-[52px] items-center gap-3 py-2 pr-2 pl-3">
      <div className="min-w-0 flex-1">{children}</div>
      {action}
    </div>
  );
}

// The session's invite code. It lets someone outside mDNS reach (VPN, strict
// LAN) join: opening it takes them straight into their app's join flow,
// pasting it in their Lobby works too. The row names it; the link itself only
// shows when the clipboard refuses it, so it can be copied by hand.
function InviteCode({ ticket }: { ticket: string }) {
  const [copied, setCopied] = useState(false);
  const [reveal, setReveal] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(false), 2500);
    return () => clearTimeout(id);
  }, [copied]);
  return (
    <InviteRow
      action={
        <Button
          variant="ghost"
          size="sm"
          className="h-7 shrink-0 gap-1.5 px-2.5 text-[12px] text-foreground/80"
          onClick={async () => {
            if (await copyText(ticket)) setCopied(true);
            else setReveal(true);
          }}
        >
          {copied ? <Check className="size-3.5" aria-hidden="true" /> : <Copy className="size-3.5" aria-hidden="true" />}
          {copied ? t("copied") : t("copy")}
        </Button>
      }
    >
      {reveal ? (
        <input
          readOnly
          autoFocus
          aria-label={t("inviteLink")}
          value={ticket}
          onFocus={(e) => e.currentTarget.select()}
          className="w-full bg-transparent text-[12px] text-foreground/80 outline-none"
        />
      ) : (
        <p className="truncate text-[13px]" title={t("inviteTicketHint")}>
          {t("inviteCode")}
        </p>
      )}
    </InviteRow>
  );
}

// The password this host set, in case they forget it while a guest is trying
// to join. Re-masks itself so it doesn't sit exposed.
function HostPassword({ pass }: { pass: string }) {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    if (!shown) return;
    const id = setTimeout(() => setShown(false), 6000);
    return () => clearTimeout(id);
  }, [shown]);
  return (
    <InviteRow
      action={
        <Button
          variant="ghost"
          size="sm"
          aria-expanded={shown}
          className="h-7 shrink-0 gap-1.5 px-2.5 text-[12px] text-foreground/80"
          onClick={() => setShown((v) => !v)}
        >
          {shown ? <EyeOff className="size-3.5" aria-hidden="true" /> : <Eye className="size-3.5" aria-hidden="true" />}
          {shown ? t("hide") : t("show")}
        </Button>
      }
    >
      <p className="truncate text-[13px]">{t("password")}</p>
      <p className={cn("truncate text-[12px] text-muted-foreground", shown && "font-mono text-foreground/80")}>
        {shown ? pass : "••••••••"}
      </p>
    </InviteRow>
  );
}

// Progressive join: the project is open, media still streaming in behind it.
// One line and a thin bar under the session name; the edits that snap back
// meanwhile explain themselves in a toast.
function LoadingStrip({ state }: { state: ExtState }) {
  const done = state.done_bytes || 0;
  const total = state.total_bytes || 0;
  const pct = total > 0 ? Math.min(100, Math.floor((done / total) * 100)) : 0;
  return (
    <motion.div
      initial={{ height: 0, opacity: 0 }}
      animate={{ height: "auto", opacity: 1 }}
      exit={{ height: 0, opacity: 0 }}
      transition={{ type: "spring", stiffness: 400, damping: 38 }}
      className="shrink-0 overflow-hidden"
    >
      <div className="flex flex-col gap-1.5 px-5 pt-3">
        <div className="flex justify-between gap-2 text-[12px]">
          <span className="truncate text-foreground/65">{t("loadingMedia")}</span>
          <span className="shrink-0 tabular-nums text-muted-foreground">{pct} %</span>
        </div>
        <div
          className="h-[3px] overflow-hidden rounded-full bg-secondary"
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
      </div>
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
  const nearby = useNearby();
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

  const self = state.peers.find((p) => p.is_self);
  const loading = !hosting && !!state.loading;
  const banName = banTarget ? displayName(banTarget) : "";

  return (
    <HintProvider>
      <div className="flex min-h-0 flex-1 flex-col">
        {/* The name, and nothing that the list or the button below already
            say: the role is in the button, the host is in the list. */}
        <header className="flex shrink-0 flex-col gap-1 px-5 pt-1">
          <h2 className="truncate text-[21px] leading-tight font-semibold tracking-[-0.015em]">
            {state.session || t("sessionFallback")}
          </h2>
          {state.nofx && <p className="text-[12px] text-muted-foreground">{t("nofxSub")}</p>}
        </header>

        <AnimatePresence initial={false}>
          {loading && <LoadingStrip key="loading" state={state} />}
        </AnimatePresence>

        <div className="scroll-slim flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto px-4 pt-3.5 pb-1">
          <section className="flex shrink-0 flex-col gap-2">
            <SectionLabel>{t("people")}</SectionLabel>
            <Group aria-label={t("peopleList")} onContextMenu={noContextMenu}>
              {/* Animate later roster changes, but not the initial peer list. */}
              <AnimatePresence mode="popLayout" initial={false}>
                {state.peers.map((p, i) => (
                  <motion.div key={p.id} layout="position" exit={{ opacity: 0 }}>
                    {i > 0 && <Divider />}
                    <PeerRow
                      peer={p}
                      peers={state.peers}
                      hosting={hosting}
                      flash={flashPeers.includes(p.id)}
                      port={port}
                      following={!!self?.follow && self.follow === p.id}
                      onKick={hosting ? () => kick(p) : undefined}
                      onBan={hosting ? () => setBanTarget(p) : undefined}
                    />
                  </motion.div>
                ))}
              </AnimatePresence>
            </Group>
          </section>

          {hosting && (!!state.ticket || !!hostPass) && (
            <section className="flex shrink-0 flex-col gap-2">
              <SectionLabel>{t("invite")}</SectionLabel>
              <Group>
                {!!state.ticket && <InviteCode ticket={state.ticket} />}
                {!!state.ticket && !!hostPass && <Divider inset={false} />}
                {!!hostPass && <HostPassword pass={hostPass} />}
                {!!state.ticket && nearby.supported && (
                  <>
                    <Divider inset={false} />
                    <NearbyRow />
                  </>
                )}
              </Group>
            </section>
          )}

          {hosting && <BanList bans={state.bans ?? []} port={port} />}
        </div>

        {/* One anchored action; toasts appear at the top and never cover it. */}
        <footer className="flex shrink-0 px-4 pt-3 pb-4">
          <Button variant="secondary" className="h-9 flex-1" onClick={requestLeave}>
            {hosting ? t("stopHosting") : t("leaveSession")}
          </Button>
        </footer>

        <Dialog open={banTarget !== null} onOpenChange={(o) => !o && setBanTarget(null)}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t("banTitle", { name: banName })}</DialogTitle>
              <DialogDescription>{t("banBody", { name: banName })}</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="secondary" onClick={() => setBanTarget(null)}>
                {t("cancel")}
              </Button>
              <Button variant="destructive" onClick={ban}>
                {t("banConfirm")}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>

        <Dialog open={confirmStop} onOpenChange={setConfirmStop}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>{t("stopHostingTitle")}</DialogTitle>
              <DialogDescription>
                {others === 1 ? t("stopHostingBodyOne") : t("stopHostingBodyMany", { count: others })}
              </DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="secondary" onClick={() => setConfirmStop(false)}>
                {t("cancel")}
              </Button>
              <Button onClick={leave}>{t("stopHosting")}</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>
    </HintProvider>
  );
}
