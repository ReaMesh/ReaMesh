import { useEffect, useState } from "react";
import { AnimatePresence, motion, useAnimate } from "motion/react";
import {
  Ban,
  Check,
  ChevronDown,
  Copy,
  Crown,
  Download,
  Ellipsis,
  Eye,
  EyeOff,
  History,
  KeyRound,
  Link2,
  LocateFixed,
  LogOut,
  Pencil,
  Snowflake,
  UserX,
  Wifi,
} from "lucide-react";
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
import { BanList } from "@/components/BanList";
import { Divider, Group, IconTile, SectionLabel } from "@/components/Group";
import { Hint, HintProvider } from "@/components/Hint";
import { PeerAvatar } from "@/components/PeerAvatar";
import { TrackNumber } from "@/components/TrackNumber";
import { send } from "@/lib/connection";
import { formatBytes, useTransfer } from "@/lib/format";
import { forgetHostPass, getHostPass } from "@/lib/hostPass";
import { resolveLang, t } from "@/lib/i18n";
import { localizeLogEntry } from "@/lib/log";
import { displayName, type ExtState, type Peer } from "@/lib/types";
import { LinkNotice } from "@/components/LinkNotice";
import { cn, focusRing, noContextMenu } from "@/lib/utils";

function editingLabel(e: NonNullable<Peer["editing"]>): string {
  if (e.kind === "track") return e.name ? t("quoted", { text: e.name }) : t("trackN", { n: e.n ?? 0 });
  if (e.field === "tempo") return t("editingTempo");
  if (e.field === "markers") return t("editingMarkers");
  return t("editingProject");
}

/** What a person is doing right now, in words. One line, most urgent first. */
function PeerMeta({ peer, peers }: { peer: Peer; peers: Peer[] }) {
  const line = (icon: React.ReactNode, text: string, tone?: string) => (
    <p className={cn("flex min-w-0 items-center gap-[5px] text-[12px] text-muted-foreground", tone)}>
      {icon}
      <span className="truncate">{text}</span>
    </p>
  );
  if (peer.frozen && !peer.is_self)
    return line(
      <Snowflake className="size-3 shrink-0" aria-hidden="true" />,
      t("reaperNotRespondingPeer"),
      "text-sky-300",
    );
  if (peer.editing)
    return line(
      <Pencil className="size-[11px] shrink-0" aria-hidden="true" />,
      t("peerEditing", { what: editingLabel(peer.editing) }),
    );
  if (peer.is_self) {
    const followers = peers.filter((p) => !p.is_self && p.follow === peer.id);
    if (followers.length === 0) return null;
    return line(
      <Eye className="size-3 shrink-0" aria-hidden="true" />,
      followers.length === 1
        ? t("nameFollowsYou", { name: displayName(followers[0]) })
        : t("followingYouMany", { count: followers.length }),
      "text-found",
    );
  }
  if (peer.follow) {
    const target = peers.find((p) => p.id === peer.follow);
    if (target)
      return line(
        <Eye className="size-3 shrink-0" aria-hidden="true" />,
        target.is_self ? t("followsYou") : t("followsName", { name: displayName(target) }),
        target.is_self ? "text-found" : undefined,
      );
  }
  if ((peer.rtt ?? 0) > 0)
    return line(
      <Wifi className="size-3 shrink-0" aria-hidden="true" />,
      `${Math.round(peer.rtt ?? 0)} ms`,
    );
  return null;
}

// Forward the ref so popLayout can measure and remove an exiting row from flow.
function PeerRow({
  ref,
  peer,
  peers,
  flash,
  port,
  following,
  onKick,
  onBan,
}: {
  ref?: React.Ref<HTMLDivElement>;
  peer: Peer;
  peers: Peer[];
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
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="truncate text-[13px] font-semibold">{name}</span>
          {peer.is_self && (
            <span className="shrink-0 text-[12px] text-muted-foreground">{t("youBadge")}</span>
          )}
          {peer.is_owner && (
            // The crown needs real text for the host to be identifiable non-visually.
            <Hint label={t("sessionHost")}>
              <span className="flex shrink-0 items-center">
                <Crown className="size-3 text-amber-400" aria-hidden="true" />
                <span className="sr-only">{t("sessionHost")}</span>
              </span>
            </Hint>
          )}
        </div>
        <PeerMeta peer={peer} peers={peers} />
      </div>
      <div
        className="flex shrink-0 items-center gap-1"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
      >
        {following && (
          <button
            type="button"
            onClick={() => follow(false)}
            title={t("stopFollowing")}
            className={cn(
              "flex h-6 items-center gap-1 rounded-md bg-found/15 px-2 text-[11.5px] font-medium text-found transition-colors hover:bg-found/25",
              focusRing,
            )}
          >
            <Eye className="size-3" aria-hidden="true" />
            {t("followingShort")}
          </button>
        )}
        <TrackNumber track={peer.track} />
        {!peer.is_self && (
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


/** A row of the "Invite" group: icon tile, content, one action on the right. */
function InviteRowShell({
  icon,
  children,
  action,
}: {
  icon: React.ReactNode;
  children: React.ReactNode;
  action: React.ReactNode;
}) {
  return (
    <div className="flex min-h-[52px] items-center gap-3 py-2 pl-3 pr-2">
      <IconTile>{icon}</IconTile>
      <div className="min-w-0 flex-1">{children}</div>
      {action}
    </div>
  );
}

// The session's invite link. It lets someone outside mDNS reach (VPN, strict
// LAN) join: opening it takes them straight into their app's join flow,
// pasting it in their Lobby works too.
function InviteLink({ ticket }: { ticket: string }) {
  const [copied, setCopied] = useState(false);
  // Make the link selectable by hand if clipboard access fails.
  const [reveal, setReveal] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(false), 2500);
    return () => clearTimeout(id);
  }, [copied]);
  return (
    <InviteRowShell
      icon={<Link2 />}
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
          {copied ? t("inviteCopied") : t("copy")}
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
          className="w-full bg-transparent font-mono text-[11.5px] outline-none"
        />
      ) : (
        <p className="truncate font-mono text-[11.5px] text-foreground/80" title={t("inviteTicketHint")}>
          {ticket}
        </p>
      )}
    </InviteRowShell>
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
    <InviteRowShell
      icon={<KeyRound />}
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
      <p className="truncate text-[13px] font-semibold">{t("password")}</p>
      <p className={cn("truncate text-[12px] text-muted-foreground", shown && "font-mono text-foreground/80")}>
        {shown ? pass : "••••••••"}
      </p>
    </InviteRowShell>
  );
}

// Progressive join: the project is open, media still streaming in behind it.
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
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: -4 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -4 }}
      className="flex shrink-0 flex-col gap-2 rounded-xl bg-card p-3"
      role="status"
      aria-live="polite"
    >
      <div className="flex items-center gap-2">
        <Download className="size-[15px] shrink-0 text-found" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate text-[13px] font-semibold">{t("loadingMedia")}</span>
        <span className="shrink-0 text-[12px] tabular-nums text-foreground/65">{pct} %</span>
      </div>
      <div
        className="h-1 overflow-hidden rounded-full bg-white/[0.07]"
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
      {detail && (
        <div className="flex justify-between gap-2 text-[11.5px] tabular-nums text-muted-foreground">
          <span className="truncate">{detail}</span>
          {rate > 0 && <span className="shrink-0">{formatBytes(rate, lang)}/s</span>}
        </div>
      )}
      {/* The one sentence that matters here; it wraps instead of truncating. */}
      <p className="text-[12px] leading-snug text-foreground/65">{t("loadingEditsLocked")}</p>
    </motion.div>
  );
}

/** Activity as one quiet row: the latest event, expandable to the recent few. */
function HistoryRow({ log }: { log: ExtState["log"] }) {
  const [open, setOpen] = useState(false);
  if (log.length === 0) return null;
  const recent = log.slice(-8).reverse();
  return (
    <Group>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className={cn("flex h-10 items-center gap-2 px-3 text-left transition-colors hover:bg-white/[0.03]", focusRing)}
      >
        <History className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
        <span className="shrink-0 text-[13px] font-semibold">{t("history")}</span>
        {!open && (
          <span className="min-w-0 flex-1 truncate text-[12px] text-muted-foreground">
            {localizeLogEntry(recent[0])}
          </span>
        )}
        <ChevronDown
          className={cn("ml-auto size-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-180")}
          aria-hidden="true"
        />
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.ul
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ type: "spring", stiffness: 400, damping: 38 }}
            className="overflow-hidden"
          >
            {recent.map((e, i) => (
              <li key={i} className="px-3 py-1.5 text-[12px] leading-snug text-foreground/65 last:pb-2.5">
                {localizeLogEntry(e)}
              </li>
            ))}
          </motion.ul>
        )}
      </AnimatePresence>
    </Group>
  );
}

// When each session was first seen, so "for 38 min" survives a trip to Settings.
const startedAt = new Map<string, number>();

function useMinutesSince(key: string): number {
  if (!startedAt.has(key)) startedAt.set(key, Date.now());
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);
  return Math.floor((now - (startedAt.get(key) ?? now)) / 60_000);
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

  const minutes = useMinutesSince(`${port}:${state.mode}:${state.session}`);
  const self = state.peers.find((p) => p.is_self);
  const owner = state.peers.find((p) => p.is_owner);
  const loading = !hosting && !!state.loading;
  // The status line says the state in words instead of stacking badges.
  const status = [
    loading ? t("loadingMediaShort") : t("live"),
    hosting
      ? t("youAreHosting")
      : owner
        ? t("connectedTo", { name: displayName(owner) })
        : t("connectedBadge"),
    !hosting && (owner?.rtt ?? 0) > 0 ? `${Math.round(owner?.rtt ?? 0)} ms` : null,
    hosting && minutes >= 1 ? t("sinceMinutes", { m: minutes }) : null,
  ].filter(Boolean);

  return (
    <HintProvider>
      <div className="flex min-h-0 flex-1 flex-col">
        <header className="flex shrink-0 flex-col gap-1.5 px-5 pt-1">
          <h2 className="truncate text-[21px] leading-tight font-semibold tracking-[-0.015em]">
            {state.session || t("sessionFallback")}
          </h2>
          <p className="flex items-center gap-2 text-[12px] text-foreground/65">
            <span
              aria-hidden="true"
              className="size-2 shrink-0 rounded-full bg-found shadow-[0_0_0_3px_color-mix(in_srgb,var(--found)_15%,transparent)]"
            />
            <span className="truncate">{status.join(" · ")}</span>
            {/* Explain why edits are not sent in view-only mode. */}
            {state.nofx && (
              <Hint label={t("nofxBadgeHint")}>
                <span className="shrink-0 text-amber-500">· {t("nofxBadge")}</span>
              </Hint>
            )}
          </p>
        </header>

        <div className="scroll-slim flex min-h-0 flex-1 flex-col gap-3.5 overflow-y-auto px-4 pt-3.5 pb-1">
          <AnimatePresence initial={false}>
            {loading && <LoadingNotice key="loading" state={state} />}
          </AnimatePresence>

          {/* Explain lag before someone blames the app for it. */}
          <LinkNotice peers={state.peers} hosting={hosting} />

          <section className="flex shrink-0 flex-col gap-2">
            <SectionLabel aside={state.peers.length}>{t("people")}</SectionLabel>
            <Group aria-label={t("peopleList")} onContextMenu={noContextMenu}>
              {/* Animate later roster changes, but not the initial peer list. */}
              <AnimatePresence mode="popLayout" initial={false}>
                {state.peers.map((p, i) => (
                  <motion.div key={p.id} layout="position" exit={{ opacity: 0 }}>
                    {i > 0 && <Divider />}
                    <PeerRow
                      peer={p}
                      peers={state.peers}
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
                {!!state.ticket && <InviteLink ticket={state.ticket} />}
                {!!state.ticket && !!hostPass && <Divider />}
                {!!hostPass && <HostPassword pass={hostPass} />}
              </Group>
            </section>
          )}

          <HistoryRow log={state.log} />
        </div>

        {/* One anchored action; toasts appear at the top and never cover it. */}
        <footer className="flex shrink-0 items-center gap-2 px-4 pt-3 pb-4">
          {hosting && <BanList bans={state.bans ?? []} port={port} variant="icon" />}
          <Button variant="secondary" className="h-9 flex-1 gap-1.5" onClick={requestLeave}>
            <LogOut className="size-[15px]" aria-hidden="true" />
            {hosting ? t("stopHosting") : t("leaveSession")}
          </Button>
        </footer>

        <Dialog open={banTarget !== null} onOpenChange={(o) => !o && setBanTarget(null)}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>
                {t("banTitle", { name: banTarget ? displayName(banTarget) : "" })}
              </DialogTitle>
              <DialogDescription>{t("banBody")}</DialogDescription>
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
