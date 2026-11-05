import { describe, expect, it } from "vitest";
import { isCopyRefusal, lastSessionAfter, type LastSession } from "./lastSession";
import type { ExtState, LogEntry } from "./types";

const frame = (over: Partial<ExtState>): ExtState =>
  ({
    mode: "browsing",
    session: "",
    project: "",
    error: null,
    log: [],
    sessions: [],
    peers: [],
    ...over,
  }) as ExtState;

const entry = (code: string): LogEntry => ({ code, params: {}, text: code });
const DIR = "C:/REAPER/ReaMesh/Nightfall-0f3a9c27d4e1b856";
const guest = frame({ mode: "client", session: "Nightfall", session_dir: DIR });
const host = frame({ mode: "hosting", session: "Nightfall" });
const block: LastSession = { name: "Nightfall", reason: entry("host_lost"), dir: DIR, linked: false };

describe("lastSessionAfter", () => {
  it("gives a guest's own leave a block, without a reason", () => {
    expect(lastSessionAfter(guest, frame({ mode: "idle" }), null)).toEqual({
      name: "Nightfall",
      reason: null,
      dir: DIR,
      linked: false,
    });
  });

  it("keeps the reason from the first frame out of the session", () => {
    const ended = entry("host_ended");
    const after = lastSessionAfter(guest, frame({ mode: "idle", error: ended }), null);
    expect(after).toEqual({ name: "Nightfall", reason: ended, dir: DIR, linked: false });
    // The error is gone from the next frame; the block isn't.
    expect(lastSessionAfter(frame({ mode: "idle", error: ended }), frame({ mode: "browsing" }), after)).toBe(after);
  });

  it("gives a host a block only when the session ended by itself, and no copy", () => {
    expect(lastSessionAfter(host, frame({ mode: "idle" }), null)).toBeNull();
    const changed = entry("project_changed");
    expect(lastSessionAfter(host, frame({ mode: "idle", error: changed }), null)).toEqual({
      name: "Nightfall",
      reason: changed,
      dir: "",
      linked: false,
    });
  });

  it("remembers that the copy is a linked folder", () => {
    const after = lastSessionAfter({ ...guest, session_linked: true }, frame({ mode: "idle" }), null);
    expect(after?.linked).toBe(true);
    expect(after?.dir).toBe(DIR);
  });

  it("offers no copy while it wasn't whole yet", () => {
    for (const over of [{ loading: true }, { placeholders: 3 }]) {
      const after = lastSessionAfter({ ...guest, ...over }, frame({ mode: "idle" }), null);
      expect(after?.dir).toBe("");
    }
  });

  it("calls a lost host lost while retrying, and when the retry dials straight away", () => {
    const retrying = lastSessionAfter(guest, frame({ reconnecting: true }), null);
    expect(retrying?.reason?.code).toBe("host_lost");
    expect(lastSessionAfter(guest, frame({ mode: "downloading" }), null)?.reason?.code).toBe("host_lost");
  });

  it("shows why the folder stopped a way back, and the dialog is the way on", () => {
    for (const code of ["local_copy_changed", "linked_copy_missing", "linked_copy_foreign"]) {
      const refused = entry(code);
      const after = lastSessionAfter(frame({ mode: "downloading" }), frame({ mode: "idle", error: refused }), block);
      expect(after?.reason).toBe(refused);
      expect(isCopyRefusal(after?.reason ?? null)).toBe(true);
    }
    // These pass once the user has done something: the way back stays.
    for (const code of ["linked_copy_busy", "linked_copy_unsaved", "linked_copy_open"]) {
      const after = lastSessionAfter(frame({ reconnecting: true }), frame({ error: entry(code) }), block);
      expect(after?.reason?.code).toBe(code);
      expect(isCopyRefusal(after?.reason ?? null)).toBe(false);
    }
    expect(isCopyRefusal(null)).toBe(false);
    expect(isCopyRefusal(entry("host_lost"))).toBe(false);
  });

  it("takes a newer reason from the retry", () => {
    const failed = entry("reconnect_failed");
    expect(lastSessionAfter(frame({ reconnecting: true }), frame({ error: failed }), block)?.reason).toBe(failed);
    const cancelled = entry("host_lost");
    expect(lastSessionAfter(frame({ reconnecting: true }), frame({ error: cancelled }), block)?.reason).toBe(
      cancelled,
    );
  });

  it("stays through a rejoin that is still on its way, and through its failure", () => {
    const retry = frame({ reconnecting: true });
    expect(lastSessionAfter(retry, frame({ mode: "downloading" }), block)).toBe(block);
    const failed = frame({ mode: "idle", error: entry("ticket_unreachable") });
    expect(lastSessionAfter(frame({ mode: "downloading" }), failed, block)).toBe(block);
  });

  it("goes when a new session starts", () => {
    expect(lastSessionAfter(frame({}), guest, block)).toBeNull();
    expect(lastSessionAfter(frame({}), host, block)).toBeNull();
    expect(lastSessionAfter(frame({}), frame({ mode: "downloading" }), block)).toBeNull();
  });

  it("leaves other errors to their toasts", () => {
    expect(lastSessionAfter(frame({}), frame({ error: entry("bad_ticket") }), block)).toBe(block);
    expect(lastSessionAfter(frame({ mode: "downloading" }), frame({ mode: "idle", error: entry("banned") }), null)).toBeNull();
  });

  it("compares nothing without a previous frame", () => {
    expect(lastSessionAfter(null, frame({ mode: "idle" }), null)).toBeNull();
    expect(lastSessionAfter(null, frame({ mode: "idle" }), block)).toBe(block);
  });
});
