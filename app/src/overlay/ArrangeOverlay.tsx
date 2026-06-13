// The full-window, click-through overlay that paints, directly over REAPER's
// arrange view, where everyone else is: a thin colored playhead per peer and a
// tinted "ghost" around the items they have selected ("Alice is holding these
// three items"). It maps each peer's project-time / track GUID onto YOUR local
// view using geometry the extension measures every tick (see computeArrangeView
// in reamesh.cpp), so it stays correct whatever your layout or scroll position.
//
// Coordinate model: the extension reports geometry in REAPER SCREEN coordinates
// (logical points, top-left origin; the extension already flips REAPER's
// native bottom-left rects). The native layer sizes THIS window to cover
// REAPER's whole window, so a screen point becomes window-relative by
// subtracting the window's own screen origin, the single conversion below
// (`relX`/`relY`). If alignment is ever off on a given machine, that origin (or
// a constant) is the one knob.
//
// Two deliberate restraints keep it from being distracting: only the *items*
// peers hold are outlined (never whole tracks), and everything is drawn muted
// and semi-transparent. Peer playheads are interpolated between the ~1 Hz
// presence beacons so a rolling playhead sweeps smoothly instead of stepping.

import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ArrangeView, Peer } from "@/lib/types";
import { displayName } from "@/lib/types";
import { useOverlaySettings } from "@/lib/overlaySettings";
import { useOverlayState } from "./useOverlayState";

/** This window's top-left in logical screen points (it covers REAPER's window,
 *  so this is REAPER's window origin). Polled from Tauri as REAPER moves. */
function useWindowOrigin() {
  const [origin, setOrigin] = useState({ x: 0, y: 0 });
  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const poll = async () => {
      try {
        const [pos, scale] = await Promise.all([win.outerPosition(), win.scaleFactor()]);
        if (alive) setOrigin({ x: pos.x / scale, y: pos.y / scale });
      } catch {
        /* plain-browser dev: no Tauri window APIs */
      }
    };
    poll();
    const id = window.setInterval(poll, 90);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);
  return origin;
}

/** Per-peer baseline for playhead interpolation: the last play position we
 *  received and when it landed. While a peer is rolling, beacons arrive only
 *  ~1 Hz, so we extrapolate forward at 1x between them (REAPER's edit/play rate
 *  isn't beaconed; the next beacon corrects any drift). When the peer isn't
 *  playing, `play` is a static edit cursor and we show it as-is. */
interface PlayBase {
  play: number;
  at: number;
  playing: boolean;
}

function useInterpolatedPlay(peers: Peer[]): (peer: Peer) => number | undefined {
  const bases = useRef<Map<string, PlayBase>>(new Map());
  const anyPlaying = peers.some((p) => !p.is_self && p.playing && typeof p.play === "number");

  // Re-baseline whenever a beacon changes a peer's reported position.
  useEffect(() => {
    const t = performance.now();
    const seen = new Set<string>();
    for (const p of peers) {
      if (p.is_self || typeof p.play !== "number") continue;
      seen.add(p.id);
      const b = bases.current.get(p.id);
      if (!b || b.play !== p.play || b.playing !== !!p.playing) {
        bases.current.set(p.id, { play: p.play, at: t, playing: !!p.playing });
      }
    }
    for (const id of [...bases.current.keys()]) if (!seen.has(id)) bases.current.delete(id);
  }, [peers]);

  // Drive smooth re-renders only while something is actually rolling.
  const [, force] = useState(0);
  useEffect(() => {
    if (!anyPlaying) return;
    let raf = 0;
    const tick = () => {
      force((n) => (n + 1) & 0xffff);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [anyPlaying]);

  return (peer: Peer) => {
    if (typeof peer.play !== "number") return undefined;
    const b = bases.current.get(peer.id);
    if (!b || !b.playing) return peer.play;
    return b.play + (performance.now() - b.at) / 1000;
  };
}

interface PlayheadDraw {
  id: string;
  x: number;
  top: number;
  bottom: number;
  color: string;
  name: string;
}
interface GhostDraw {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  color: string;
}

export function ArrangeOverlay({ port }: { port: number | null }) {
  const { state } = useOverlayState(port);
  const settings = useOverlaySettings();
  const origin = useWindowOrigin();

  const view = state?.view as ArrangeView | undefined;
  const peers = state?.peers ?? [];
  const tracks = view?.tracks;
  const playAt = useInterpolatedPlay(peers);

  // Nothing measurable (not in a session, or geometry unresolved) -> paint air.
  if (!view || !tracks || view.spp <= 0) return null;

  const relX = (screenX: number) => screenX - origin.x;
  const relY = (screenY: number) => screenY - origin.y;
  const timeToX = (t: number) => relX(view.ax + (t - view.t0) / view.spp);
  const arrangeLeft = relX(view.ax);
  const winW = window.innerWidth;

  const playheads: PlayheadDraw[] = [];
  const ghosts: GhostDraw[] = [];

  for (const peer of peers) {
    if (peer.is_self) continue;
    const color = peer.color || "#888";

    if (settings.playheads) {
      const play = playAt(peer);
      if (typeof play === "number") {
        const x = timeToX(play);
        if (x >= arrangeLeft - 1 && x <= winW + 1) {
          playheads.push({
            id: peer.id,
            x,
            top: relY(view.top),
            bottom: relY(view.bottom),
            color,
            name: displayName(peer),
          });
        }
      }
    }

    // Selection ghosts: only the *items* a peer is holding (never whole tracks
    // because that lit up the entire row and was far too loud).
    if (settings.ghosts) {
      for (const guid of peer.seli ?? []) {
        const it = view.items?.[guid];
        const tr = it && tracks[it.tr];
        if (!it || !tr) continue;
        ghosts.push({
          id: `${peer.id}:it:${guid}`,
          x: timeToX(it.pos),
          y: relY(tr.y),
          w: Math.max(2, it.len / view.spp),
          h: tr.h,
          color,
        });
      }
    }
  }

  return (
    <svg
      style={{ position: "fixed", inset: 0, width: "100%", height: "100%", pointerEvents: "none" }}
    >
      {ghosts.map((g) => (
        <rect
          key={g.id}
          x={g.x}
          y={g.y}
          width={g.w}
          height={g.h}
          rx={2}
          fill={g.color}
          fillOpacity={0.06}
          stroke={g.color}
          strokeOpacity={0.4}
          strokeWidth={1}
        />
      ))}
      {playheads.map((p) => (
        <g key={p.id}>
          <line
            x1={p.x}
            y1={p.top}
            x2={p.x}
            y2={p.bottom}
            stroke={p.color}
            strokeWidth={1.5}
            strokeOpacity={0.5}
          />
          {/* Small, low-key name tab at the top of the line. */}
          <rect
            x={p.x}
            y={p.top}
            width={Math.min(88, 7 + p.name.length * 5.6)}
            height={13}
            rx={2}
            fill={p.color}
            fillOpacity={0.75}
          />
          <text x={p.x + 4} y={p.top + 9.5} fontSize={9} fontWeight={600} fill="#fff" fillOpacity={0.95}>
            {p.name}
          </text>
        </g>
      ))}
    </svg>
  );
}
