// Frameless presence HUD for peer names, colors, and focused tracks. Session
// controls remain in the main companion window.
//
// Expanded shows the full roster. Collapsed shows the avatar stack.
//
// The OS window is sized to hug the card exactly (see the ResizeObserver
// below) so its transparent area never swallows clicks meant for REAPER.

import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { ChevronUp, Crown } from "lucide-react";
import { SlotText } from "slot-text/react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { PeerAvatar } from "@/components/PeerAvatar";
import { t, useLang } from "@/lib/i18n";
import { displayName, type Peer } from "@/lib/types";
import { useOverlaySettings } from "@/lib/overlaySettings";
import { useOverlayState } from "./useOverlayState";

const WIDTH = 232;

/** The peer's current track number, right-aligned and rolling on change. */
function TrackNumber({ track }: { track?: number | null }) {
  if (track == null || track <= 0) return <span className="w-4" />;
  return (
    <SlotText
      text={String(track)}
      options={{ direction: "up", duration: 260 }}
      className="w-4 shrink-0 text-right text-[12px] tabular-nums text-muted-foreground"
    />
  );
}

function PeerRow({
  peer,
  onReveal,
}: {
  peer: Peer;
  onReveal?: (peer: Peer) => void;
}) {
  // Click-to-follow: only meaningful for someone else who's actually on a
  // track. Tapping your own row, or a peer not on any track, does nothing.
  const clickable =
    !!onReveal && !peer.is_self && peer.track != null && peer.track > 0;
  return (
    <motion.div
      layout
      initial={{ opacity: 0, x: -8 }}
      animate={{ opacity: 1, x: 0 }}
      exit={{ opacity: 0, x: 8 }}
      transition={{ type: "spring", stiffness: 350, damping: 28 }}
      onClick={clickable ? () => onReveal!(peer) : undefined}
      onKeyDown={
        clickable
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onReveal!(peer);
              }
            }
          : undefined
      }
      role={clickable ? "button" : undefined}
      tabIndex={clickable ? 0 : undefined}
      title={clickable ? t("revealPeerTrack", { name: displayName(peer) }) : undefined}
      className={`flex items-center gap-2 rounded-md px-1.5 py-1 ${
        clickable
          ? "cursor-pointer transition-colors hover:bg-white/5"
          : ""
      }`}
    >
      <PeerAvatar peer={peer} size="sm" />
      <span className="min-w-0 flex-1 truncate text-[12px] font-medium">
        {displayName(peer)}
      </span>
      {peer.is_owner && (
        <span title={t("sessionHost")} className="shrink-0">
          <Crown className="size-3 text-amber-400" />
        </span>
      )}
      {peer.is_self && (
        <span className="shrink-0 text-[9px] text-muted-foreground">
          {t("youBadge")}
        </span>
      )}
      <TrackNumber track={peer.track} />
    </motion.div>
  );
}

/** Overlapping identity avatars, shown only when collapsed. */
function AvatarStack({ peers }: { peers: Peer[] }) {
  return (
    <div className="flex">
      {peers.slice(0, 4).map((p, i) => (
        <motion.div
          key={p.id}
          layout
          style={{ marginLeft: i === 0 ? 0 : -8, zIndex: peers.length - i }}
        >
          <PeerAvatar peer={p} size="sm" className="ring-2 ring-background" />
        </motion.div>
      ))}
    </div>
  );
}

export function Overlay({ port }: { port: number | null }) {
  const { state, send } = useOverlayState(port);
  const [collapsed, setCollapsed] = useState(false);
  const settings = useOverlaySettings();
  const cardRef = useRef<HTMLDivElement>(null);
  useLang(); // re-render on language change

  // Keep the OS window glued to the card's actual height. The window is
  // transparent, so any slack would block REAPER underneath; instead it grows
  // and shrinks (smoothly, as the collapse animation runs) to fit.
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const win = getCurrentWindow();
    let raf = 0;
    const fit = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const h = Math.max(1, Math.ceil(el.getBoundingClientRect().height));
        win.setSize(new LogicalSize(WIDTH, h)).catch(() => {});
      });
    };
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    fit();
    return () => {
      ro.disconnect();
      cancelAnimationFrame(raf);
    };
  }, []);

  const peers = state?.peers ?? [];
  const reveal = (peer: Peer) => send({ cmd: "reveal_peer", peer_id: peer.id });

  return (
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
          className="grid size-6 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground"
        >
          <motion.span
            animate={{ rotate: collapsed ? 180 : 0 }}
            transition={{ type: "spring", stiffness: 400, damping: 30 }}
            className="grid place-items-center"
          >
            <ChevronUp className="size-4" />
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
              <div className="border-t border-white/10 px-1.5 py-1.5">
                {peers.length === 0 ? (
                  <p className="px-1.5 py-1 text-[12px] text-muted-foreground">
                    {t("connecting")}
                  </p>
                ) : (
                  <AnimatePresence initial={false} mode="popLayout">
                    {peers.map((p) => (
                      <PeerRow key={p.id} peer={p} onReveal={reveal} />
                    ))}
                  </AnimatePresence>
                )}
              </div>
            )}

          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
