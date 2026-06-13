// The full-window, click-through overlay that paints, directly over REAPER's
// arrange view, where everyone else is: a live presence layer in the spirit of
// Word/Figma cursors:
//   • a labeled caret at each peer's edit cursor ("Alice is here"),
//   • a separate sweeping playhead while they're rolling,
//   • a translucent band over their time selection.
// Per-object selection (which items / tracks a peer has selected) is
// deliberately NOT drawn here: a peer's editing *lock* on a media item shows up
// instead as a local dark-grey tint on the item itself (the extension recolors
// the item via reamesh_locked_entities), which reads better than a floating
// ghost. It maps each peer's project-time / track GUID onto YOUR local view
// using geometry the extension measures every tick (computeArrangeView in
// reamesh.cpp), so it stays correct whatever your layout or scroll position.
//
// Coordinate model: the extension reports geometry in REAPER SCREEN coordinates
// (logical points, top-left origin; it already flips REAPER's native
// bottom-left rects). The native layer sizes THIS window to cover REAPER's whole
// window, so a screen point becomes window-relative by subtracting the window's
// own screen origin (`relX`/`relY`). That origin is tracked EVENT-DRIVEN (the
// window moves/resizes exactly when REAPER does), not on a lazy poll; a stale
// origin was the old "playhead in the wrong place / flickers when REAPER moves"
// bug. If alignment is ever off on a given machine, that origin is the one knob.

import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { ArrangeView, Peer } from "@/lib/types";
import { displayName } from "@/lib/types";
import { useOverlaySettings } from "@/lib/overlaySettings";
import { useOverlayState } from "./useOverlayState";

/** This window's top-left in logical screen points. The native layer pins this
 *  window onto REAPER's frame, so this IS REAPER's window origin, and because
 *  the window moves/resizes precisely when REAPER does, we learn the new origin
 *  from Tauri's move/resize events (carrying the new position) instead of
 *  polling, which used to lag ~90ms behind and misplace everything mid-move.
 *  `null` until the first read, so we never paint at a bogus {0,0}.
 *
 *  NOTE: `outerPosition`/`scaleFactor`/move-resize events are silently caught,
 *  so they fail invisibly if the "arrange" window lacks the matching Tauri
 *  capability: origin then stays `null` forever and the whole layer paints
 *  nothing. The grant lives in `src-tauri/capabilities/arrange.json`
 *  (`core:window:allow-outer-position` + `allow-scale-factor`); keep that window
 *  label listed there. */
function useWindowOrigin() {
  const [origin, setOrigin] = useState<{ x: number; y: number } | null>(null);
  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    let scale = 1;
    let lastX = Number.NaN;
    let lastY = Number.NaN;
    const cleanups: Array<() => void> = [];
    const apply = (px: number, py: number) => {
      const x = px / scale;
      const y = py / scale;
      if (!alive || (x === lastX && y === lastY)) return; // skip redundant renders
      lastX = x;
      lastY = y;
      setOrigin({ x, y });
    };
    const readPos = async () => {
      try {
        const pos = await win.outerPosition();
        apply(pos.x, pos.y);
      } catch {
        /* plain-browser dev: no Tauri window APIs */
      }
    };
    (async () => {
      try {
        scale = await win.scaleFactor();
        await readPos();
        cleanups.push(await win.onMoved((e) => apply(e.payload.x, e.payload.y)));
        cleanups.push(await win.onResized(() => void readPos()));
        cleanups.push(
          await win.onScaleChanged((e) => {
            scale = e.payload.scaleFactor;
            void readPos();
          }),
        );
      } catch {
        /* plain-browser dev */
      }
    })();
    // Safety net for the rare change that emits no event (e.g. a display
    // reconfiguration the window doesn't move through).
    const id = window.setInterval(() => void readPos(), 500);
    return () => {
      alive = false;
      window.clearInterval(id);
      for (const c of cleanups) c();
    };
  }, []);
  return origin;
}

/** Per-peer baseline for playhead interpolation: the last play position we
 *  received and when it landed. While a peer is rolling, beacons arrive only
 *  ~1 Hz, so we extrapolate forward at 1x between them (REAPER's play rate
 *  isn't beaconed; the next beacon corrects any drift). */
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

interface CaretDraw {
  id: string;
  x: number;
  top: number;
  bottom: number;
  color: string;
  name: string;
}
interface HeadDraw {
  id: string;
  x: number;
  top: number;
  bottom: number;
  color: string;
}
interface BandDraw {
  id: string;
  x: number;
  w: number;
  top: number;
  bottom: number;
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

  // Nothing measurable (origin not read yet, not in a session, or geometry
  // unresolved) -> paint air.
  if (!origin || !view || !tracks || view.spp <= 0) return null;

  const relX = (screenX: number) => screenX - origin.x;
  const relY = (screenY: number) => screenY - origin.y;
  const timeToX = (t: number) => relX(view.ax + (t - view.t0) / view.spp);
  const arrangeLeft = relX(view.ax);
  const top = relY(view.top);
  const bottom = relY(view.bottom);
  const winW = window.innerWidth;
  const visibleX = (x: number) => x >= arrangeLeft - 1 && x <= winW + 1;
  const clampX = (x: number) => Math.max(arrangeLeft, Math.min(x, winW));

  const carets: CaretDraw[] = [];
  const heads: HeadDraw[] = [];
  const bands: BandDraw[] = [];

  for (const peer of peers) {
    if (peer.is_self) continue;
    const color = peer.color || "#888";

    if (settings.playheads) {
      // Edit-cursor caret: the "Alice is here" marker, always shown. Falls
      // back to the play field for an older extension that doesn't beacon a
      // separate edit cursor.
      const caretPos = typeof peer.cursor === "number" ? peer.cursor : peer.play;
      if (typeof caretPos === "number") {
        const x = timeToX(caretPos);
        if (visibleX(x)) {
          carets.push({ id: peer.id, x, top, bottom, color, name: displayName(peer) });
        }
      }
      // Sweeping playhead: only while they're actually rolling, so a stopped
      // peer is just a calm caret, not a redundant second line.
      if (peer.playing) {
        const play = playAt(peer);
        if (typeof play === "number") {
          const x = timeToX(play);
          if (visibleX(x)) heads.push({ id: peer.id, x, top, bottom, color });
        }
      }
    }

    if (settings.ghosts) {
      // Time selection: a translucent band across the tracks (their loop /
      // time range, colored as them).
      if (typeof peer.tsa === "number" && typeof peer.tsb === "number" && peer.tsb > peer.tsa) {
        const x1 = clampX(timeToX(peer.tsa));
        const x2 = clampX(timeToX(peer.tsb));
        if (x2 - x1 >= 1) bands.push({ id: peer.id, x: x1, w: x2 - x1, top, bottom, color });
      }
    }
  }

  return (
    <svg
      style={{ position: "fixed", inset: 0, width: "100%", height: "100%", pointerEvents: "none" }}
    >
      {/* Time-selection bands sit behind everything else. */}
      {bands.map((b) => (
        <g key={b.id}>
          <rect x={b.x} y={b.top} width={b.w} height={b.bottom - b.top} fill={b.color} fillOpacity={0.07} />
          <line x1={b.x} y1={b.top} x2={b.x} y2={b.bottom} stroke={b.color} strokeOpacity={0.35} strokeWidth={1} />
          <line
            x1={b.x + b.w}
            y1={b.top}
            x2={b.x + b.w}
            y2={b.bottom}
            stroke={b.color}
            strokeOpacity={0.35}
            strokeWidth={1}
          />
        </g>
      ))}
      {/* Sweeping playheads: brighter line + a small ▶ at the top. */}
      {heads.map((p) => (
        <g key={`h:${p.id}`}>
          <line x1={p.x} y1={p.top} x2={p.x} y2={p.bottom} stroke={p.color} strokeWidth={1.5} strokeOpacity={0.6} />
          <path d={`M ${p.x} ${p.top} l 7 0 l -3.5 6 z`} fill={p.color} fillOpacity={0.85} />
        </g>
      ))}
      {/* Edit-cursor carets: thin line + a low-key name tab. Drawn last so the
          labels sit on top of the bands. */}
      {carets.map((p) => (
        <g key={`c:${p.id}`}>
          <line x1={p.x} y1={p.top} x2={p.x} y2={p.bottom} stroke={p.color} strokeWidth={1} strokeOpacity={0.7} />
          <rect
            x={p.x}
            y={p.top}
            width={Math.min(88, 7 + p.name.length * 5.6)}
            height={13}
            rx={2}
            fill={p.color}
            fillOpacity={0.8}
          />
          <text x={p.x + 4} y={p.top + 9.5} fontSize={9} fontWeight={600} fill="#fff" fillOpacity={0.95}>
            {p.name}
          </text>
        </g>
      ))}
    </svg>
  );
}
