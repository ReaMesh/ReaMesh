import { describe, expect, it } from "vitest";
import { STAR_ASK_AFTER, STAR_SESSION_MS, starAskAfter, starAskDue, type StarAsk } from "./starAsk";
import type { ExtState, Peer } from "./types";

const me = { id: "a", name: "Ben", color: "#0f0", is_self: true, is_owner: true } as Peer;
const them = { id: "b", name: "Alex", color: "#00f", is_self: false, is_owner: false } as Peer;

const frame = (over: Partial<ExtState>): ExtState =>
  ({ mode: "browsing", session: "", project: "", error: null, log: [], sessions: [], peers: [], ...over }) as ExtState;

const hosting = (peers: Peer[]) => frame({ mode: "hosting", session: "Nightfall", peers });
const fresh: StarAsk = { sessions: 0, done: false, current: null };
const MIN = 60_000;

/** Walk frames through the record, one every 20s from `start` to `end`. */
function run(rec: StarAsk, state: ExtState, start: number, end: number): StarAsk {
  for (let t = start; t <= end; t += 20_000) rec = starAskAfter(rec, state, 47415, t);
  return rec;
}

describe("starAskAfter", () => {
  it("counts a session once others were in it long enough", () => {
    const rec = run(fresh, hosting([me, them]), 0, STAR_SESSION_MS + MIN);
    expect(rec.sessions).toBe(1);
    // Still running: it doesn't count twice.
    expect(run(rec, hosting([me, them]), STAR_SESSION_MS + 2 * MIN, 3 * STAR_SESSION_MS).sessions).toBe(1);
  });

  it("doesn't count a session alone or a short one", () => {
    expect(run(fresh, hosting([me]), 0, 2 * STAR_SESSION_MS).sessions).toBe(0);
    let rec = run(fresh, hosting([me, them]), 0, 5 * MIN);
    rec = starAskAfter(rec, frame({ mode: "browsing" }), 47415, 5 * MIN + 1);
    expect(rec.sessions).toBe(0);
    expect(rec.current).toBeNull();
  });

  it("starts over when the session went unseen for a while", () => {
    let rec = run(fresh, hosting([me, them]), 0, 8 * MIN);
    // The app was closed; the next frame comes much later.
    rec = run(rec, hosting([me, them]), 60 * MIN, 61 * MIN);
    expect(rec.sessions).toBe(0);
  });

  it("asks after enough sessions and never once done", () => {
    expect(starAskDue({ ...fresh, sessions: STAR_ASK_AFTER - 1 })).toBe(false);
    expect(starAskDue({ ...fresh, sessions: STAR_ASK_AFTER })).toBe(true);
    const done = { ...fresh, sessions: STAR_ASK_AFTER, done: true };
    expect(starAskDue(done)).toBe(false);
    expect(run(done, hosting([me, them]), 0, 2 * STAR_SESSION_MS)).toBe(done);
  });
});
