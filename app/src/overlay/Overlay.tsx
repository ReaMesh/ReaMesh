// Frameless presence HUD for peer names, colors, and focused tracks. Session
// controls remain in the main companion window.
//
// Expanded shows the full roster. Collapsed shows the avatar stack.
//
// The OS window is sized to hug the card exactly (see the ResizeObserver
// below) so its transparent area never swallows clicks meant for REAPER.

import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useAnimate } from "motion/react";
import { ChevronUp, Crown, Eye, Flag, Pin, PinOff, Snowflake } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { currentMonitor, getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { Hint, HintProvider } from "@/components/Hint";
import { PeerAvatar } from "@/components/PeerAvatar";
import { TrackNumber } from "@/components/Rolling";
import { t, useLang } from "@/lib/i18n";
import { displayName, type Peer } from "@/lib/types";
import { useOverlaySettings } from "@/lib/overlaySettings";
import { useOverlayState } from "./useOverlayState";

const WIDTH = 232;
/// Maximum HUD height relative to its current monitor. The roster scrolls once
/// it reaches this limit.
const MAX_HEIGHT_FRACTION = 0.5;
/// Floor for the clamp, so a tiny/odd monitor report can't collapse the HUD.
const MIN_MAX_HEIGHT = 160;

function PeerRow({
  peer,
  onReveal,
  pickMode = false,
  onPick,
  followerCount = 0,
}: {
  peer: Peer;
  onReveal?: (peer: Peer) => void;
  /** Follow-mode picking: clicking any other person starts following them. */
  pickMode?: boolean;
  onPick?: (peer: Peer) => void;
  /** On the self row: how many peers are following this user. */
  followerCount?: number;
}) {
  // Other peers can be followed in pick mode or revealed when they are on a track.
  const pickable = pickMode && !peer.is_self && !!onPick;
  const clickable =
    pickable || (!!onReveal && !peer.is_self && peer.track != null && peer.track > 0);
  // Flash the row to acknowledge the change happening in REAPER.
  const [flashScope, animateFlash] = useAnimate();
  const activate = () => {
    if (pickable) {
      onPick!(peer);
    } else {
      onReveal!(peer);
    }
    if (!flashScope.current) return;
    const tint = /^#[0-9a-fA-F]{6}$/.test(peer.color)
      ? [`${peer.color}4d`, `${peer.color}00`]
      : ["rgba(255,255,255,0.14)", "rgba(255,255,255,0)"];
    animateFlash(
      flashScope.current,
      { backgroundColor: tint },
      { duration: 0.5, ease: "easeOut" },
    );
  };
  const followingYou =
    followerCount === 1
      ? t("followingYouOne")
      : t("followingYouMany", { count: followerCount });
  return (
    <Hint
      label={
        pickable
          ? t("followingName", { name: displayName(peer) })
          : clickable
            ? t("revealPeerTrack", { name: displayName(peer) })
            : undefined
      }
    >
      <motion.div
        layout
        initial={{ opacity: 0, x: -8 }}
        animate={{ opacity: 1, x: 0 }}
        exit={{ opacity: 0, x: 8 }}
        transition={{ type: "spring", stiffness: 350, damping: 28 }}
        onClick={clickable ? activate : undefined}
        onKeyDown={
          clickable
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  activate();
                }
              }
            : undefined
        }
        role={clickable ? "button" : undefined}
        tabIndex={clickable ? 0 : undefined}
        className={`relative flex items-center gap-2 rounded-md px-1.5 py-1 ${
          clickable ? "cursor-pointer transition-colors hover:bg-white/5" : ""
        } ${pickable ? "ring-1 ring-inset ring-white/15" : ""}`}
      >
        {clickable && (
          <span
            ref={flashScope}
            aria-hidden
            className="pointer-events-none absolute inset-0 rounded-md"
          />
        )}
        <PeerAvatar peer={peer} size="sm" />
        <span className="min-w-0 flex-1 truncate text-[12px] font-medium">
          {displayName(peer)}
        </span>
        {peer.frozen && !peer.is_self && (
          // The peer is online, but their REAPER has stopped responding.
          <Hint label={t("reaperNotRespondingPeer")}>
            <span className="flex shrink-0 items-center">
              <Snowflake className="size-3 text-sky-300" aria-hidden="true" />
              <span className="sr-only">{t("reaperNotRespondingPeer")}</span>
            </span>
          </Hint>
        )}
        {peer.is_self && followerCount > 0 && (
          // The being-followed indicator: someone's transport now mirrors yours.
          <Hint label={followingYou}>
            <span className="flex shrink-0 items-center gap-0.5 text-found">
              <Eye className="size-3" aria-hidden="true" />
              {followerCount > 1 && <span className="text-[9px]">{followerCount}</span>}
              <span className="sr-only">{followingYou}</span>
            </span>
          </Hint>
        )}
        {peer.is_owner && (
          <Hint label={t("sessionHost")}>
            <span className="flex shrink-0 items-center">
              <Crown className="size-3 text-amber-400" aria-hidden="true" />
              <span className="sr-only">{t("sessionHost")}</span>
            </span>
          </Hint>
        )}
        {peer.is_self && (
          <span className="shrink-0 text-[9px] text-muted-foreground">
            {t("youBadge")}
          </span>
        )}
        <TrackNumber track={peer.track} />
      </motion.div>
    </Hint>
  );
}

const STACK_MAX = 4;

/** Overlapping identity avatars, shown only when collapsed. */
function AvatarStack({ peers }: { peers: Peer[] }) {
  const extra = peers.length - STACK_MAX;
  return (
    <div className="flex items-center">
      {peers.slice(0, STACK_MAX).map((p, i) => (
        <motion.div
          key={p.id}
          layout
          style={{ marginLeft: i === 0 ? 0 : -8, zIndex: peers.length - i }}
        >
          <PeerAvatar peer={p} size="sm" className="ring-2 ring-background" />
        </motion.div>
      ))}
      {/* Otherwise the collapsed HUD silently under-reports who is present. */}
      {extra > 0 && (
        <span className="ml-1 shrink-0 text-[10px] font-medium text-muted-foreground">
          +{extra}
        </span>
      )}
    </div>
  );
}

export function Overlay({ port }: { port: number | null }) {
  const { state, send } = useOverlayState(port);
  const [collapsed, setCollapsed] = useState(false);
  const settings = useOverlaySettings();
  const cardRef = useRef<HTMLDivElement>(null);
  useLang(); // re-render on language change

  // Fit the native window to the card throughout size animations.
  // Extra transparent space would intercept clicks intended for REAPER.
  const [maxBodyHeight, setMaxBodyHeight] = useState(MIN_MAX_HEIGHT);
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const win = getCurrentWindow();
    let raf = 0;
    // Recalculate the height limit when the HUD moves to another monitor.
    let cap = Number.POSITIVE_INFINITY;
    const measure = async () => {
      try {
        const mon = await currentMonitor();
        if (!mon) return;
        const scale = mon.scaleFactor || 1;
        const logical = mon.size.height / scale;
        cap = Math.max(MIN_MAX_HEIGHT, Math.floor(logical * MAX_HEIGHT_FRACTION));
        // The header is fixed; the roster body gets whatever is left.
        setMaxBodyHeight(Math.max(MIN_MAX_HEIGHT, cap - 36));
      } catch {
        // Plain-browser development has no Tauri window API.
      }
    };
    const fit = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const h = Math.max(1, Math.ceil(el.getBoundingClientRect().height));
        win.setSize(new LogicalSize(WIDTH, Math.min(h, cap))).catch(() => {});
      });
    };
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    void measure().then(fit);
    const unlisten = win.onMoved(() => void measure().then(fit)).catch(() => null);
    return () => {
      ro.disconnect();
      cancelAnimationFrame(raf);
      void unlisten.then((f) => f?.());
    };
  }, []);

  const peers = state?.peers ?? [];
  const reveal = (peer: Peer) => send({ cmd: "reveal_peer", peer_id: peer.id });

  // ---- roster bar: shared markers + follow mode ----
  const self = peers.find((p) => p.is_self);
  const followingId = self?.follow || "";
  const followTarget = followingId ? peers.find((p) => p.id === followingId) : undefined;
  const followerCount = self
    ? peers.filter((p) => !p.is_self && p.follow && p.follow === self.id).length
    : 0;
  const [markerOpen, setMarkerOpen] = useState(false);
  const [markerName, setMarkerName] = useState("");
  const [pickMode, setPickMode] = useState(false);
  // Follow target gone (left / went stale): the extension already stopped
  // following; make sure pick mode isn't stuck either.
  useEffect(() => {
    if (followingId) setPickMode(false);
  }, [followingId]);

  // Temporarily let the HUD take focus while the marker field is open, then
  // return focus to REAPER. Focus the field only after the native call finishes.
  const markerInput = useRef<HTMLInputElement>(null);
  const markerWasOpen = useRef(false);
  useEffect(() => {
    // Do not hand focus to REAPER just because the HUD mounted.
    if (!markerOpen && !markerWasOpen.current) return;
    markerWasOpen.current = markerOpen;
    let alive = true;
    invoke("set_overlay_interactive", { on: markerOpen })
      .then(() => {
        if (alive && markerOpen) markerInput.current?.focus();
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [markerOpen]);
  useEffect(
    () => () => {
      invoke("set_overlay_interactive", { on: false }).catch(() => {});
    },
    [],
  );

  const closeMarker = () => {
    setMarkerOpen(false);
    setMarkerName("");
  };

  // Close the marker field on blur so later HUD clicks cannot steal focus.
  useEffect(() => {
    if (!markerOpen) return;
    const onBlur = () => closeMarker();
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, [markerOpen]); // eslint-disable-line react-hooks/exhaustive-deps
  const shareMarker = () => {
    const name = markerName.trim();
    if (!name) return;
    send({ cmd: "share_marker", name });
    closeMarker();
  };
  const pickFollow = (peer: Peer) => {
    setPickMode(false);
    send({ cmd: "follow", peer_id: peer.id });
  };
  const stopFollowing = () => send({ cmd: "follow", peer_id: "" });
  // The two tools share one fold below the bar, so only one is ever open.
  const openMarker = () => {
    setPickMode(false);
    setMarkerOpen((v) => !v);
  };
  const openPick = () => {
    closeMarker();
    setPickMode((v) => !v);
  };

  return (
    // The card is the tooltip boundary: the OS window hugs it exactly, so a
    // bubble outside the card would be clipped (see Hint.tsx).
    <HintProvider boundary={cardRef}>
    <div
      ref={cardRef}
      style={{ width: WIDTH }}
      className="overflow-hidden rounded-xl border border-white/10 bg-background/80 text-foreground opacity-90 backdrop-blur-xl transition-opacity duration-200 hover:opacity-100"
    >
      <div
        data-tauri-drag-region
        className="flex h-9 items-center gap-2 px-2.5"
      >
        <div
          data-tauri-drag-region
          className="flex min-w-0 flex-1 items-center"
        >
          <AnimatePresence initial={false}>
            {collapsed && settings.roster && peers.length > 0 && (
              <motion.div
                key="stack"
                initial={{ opacity: 0, scale: 0.85, x: -4 }}
                animate={{ opacity: 1, scale: 1, x: 0 }}
                exit={{ opacity: 0, scale: 0.85, x: -4 }}
                transition={{ type: "spring", stiffness: 420, damping: 30 }}
              >
                <AvatarStack peers={peers} />
              </motion.div>
            )}
          </AnimatePresence>
        </div>
        <button
          type="button"
          onClick={() => setCollapsed((c) => !c)}
          aria-label={collapsed ? t("expand") : t("collapse")}
          className="grid size-6 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors outline-none hover:bg-white/5 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
        >
          <motion.span
            animate={{ rotate: collapsed ? 180 : 0 }}
            transition={{ type: "spring", stiffness: 400, damping: 30 }}
            className="grid place-items-center"
          >
            <ChevronUp className="size-4" aria-hidden="true" />
          </motion.span>
        </button>
      </div>

      <AnimatePresence initial={false}>
        {!collapsed && (
          <motion.div
            key="body"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ type: "spring", stiffness: 400, damping: 38 }}
            style={{ overflow: "hidden" }}
          >
            {settings.roster && (
              <div
                className="scroll-slim overflow-y-auto border-t border-white/10 px-1.5 py-1.5"
                style={{ maxHeight: maxBodyHeight }}
              >
                {peers.length === 0 ? (
                  <p className="px-1.5 py-1 text-[12px] text-muted-foreground">
                    {t("connecting")}
                  </p>
                ) : (
                  <AnimatePresence initial={false} mode="popLayout">
                    {peers.map((p) => (
                      <PeerRow
                        key={p.id}
                        peer={p}
                        onReveal={reveal}
                        pickMode={pickMode}
                        onPick={pickFollow}
                        followerCount={followerCount}
                      />
                    ))}
                  </AnimatePresence>
                )}
              </div>
            )}

            {/* Markers and follow mode share this expandable area. */}
            <div className="border-t border-white/10 px-1.5 py-1">
              <div className="flex h-6 items-center gap-1">
                <Hint label={t("shareMarkerTitle")}>
                  <button
                    type="button"
                    onClick={openMarker}
                    aria-expanded={markerOpen}
                    aria-label={t("shareMarkerTitle")}
                    className={`grid size-6 shrink-0 place-items-center rounded-md transition-colors outline-none hover:bg-white/5 hover:text-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring ${
                      markerOpen ? "bg-white/10 text-foreground" : "text-muted-foreground"
                    }`}
                  >
                    <Flag className="size-3.5" aria-hidden="true" />
                  </button>
                </Hint>
                {followTarget ? (
                  // Pinned to someone: the whole label is the unpin button.
                  <Hint label={t("stopFollowing")}>
                    <button
                      type="button"
                      onClick={stopFollowing}
                      aria-label={t("stopFollowing")}
                      className="flex h-6 min-w-0 items-center gap-1 rounded-md px-1.5 text-found transition-colors outline-none hover:bg-white/5 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                    >
                      <PinOff className="size-3.5 shrink-0" aria-hidden="true" />
                      <span className="min-w-0 truncate text-[11px] font-medium">
                        {t("followingName", { name: displayName(followTarget) })}
                      </span>
                    </button>
                  </Hint>
                ) : (
                  <Hint label={t("followMode")}>
                    <button
                      type="button"
                      onClick={openPick}
                      aria-expanded={pickMode}
                      aria-label={t("followMode")}
                      className={`grid size-6 shrink-0 place-items-center rounded-md transition-colors outline-none hover:bg-white/5 hover:text-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring ${
                        pickMode ? "bg-white/10 text-foreground" : "text-muted-foreground"
                      }`}
                    >
                      <Pin className="size-3.5" aria-hidden="true" />
                    </button>
                  </Hint>
                )}
              </div>
              <AnimatePresence initial={false} mode="wait">
                {markerOpen ? (
                  <motion.div
                    key="marker"
                    initial={{ height: 0, opacity: 0 }}
                    animate={{ height: "auto", opacity: 1 }}
                    exit={{ height: 0, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 400, damping: 38 }}
                    className="overflow-hidden"
                  >
                    {/* px-px so the focus ring (drawn inside, see ring-inset)
                        still has a hair of room and never clips against the
                        card's rounded edge. */}
                    <div className="flex items-center gap-1 px-px pt-1 pb-0.5">
                      <input
                        ref={markerInput}
                        value={markerName}
                        onChange={(e) => setMarkerName(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") shareMarker();
                          if (e.key === "Escape") closeMarker();
                        }}
                        maxLength={80}
                        aria-label={t("markerNamePlaceholder")}
                        placeholder={t("markerNamePlaceholder")}
                        className="h-6 min-w-0 flex-1 rounded-md bg-white/5 px-1.5 text-[11px] outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                      />
                      <button
                        type="button"
                        onClick={shareMarker}
                        disabled={!markerName.trim()}
                        className="h-6 shrink-0 rounded-md px-2 text-[11px] font-medium text-foreground transition-colors outline-none hover:bg-white/10 disabled:opacity-40 focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                      >
                        {t("share")}
                      </button>
                    </div>
                  </motion.div>
                ) : pickMode && !followTarget ? (
                  <motion.div
                    key="pick"
                    initial={{ height: 0, opacity: 0 }}
                    animate={{ height: "auto", opacity: 1 }}
                    exit={{ height: 0, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 400, damping: 38 }}
                    className="overflow-hidden"
                  >
                    <p className="px-1.5 pt-1 pb-0.5 text-[10px] leading-relaxed text-muted-foreground">
                      {t("pickPersonToFollow")}
                    </p>
                  </motion.div>
                ) : null}
              </AnimatePresence>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
    </HintProvider>
  );
}
