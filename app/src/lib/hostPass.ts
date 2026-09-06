// The passphrase the local user chose when they started hosting, kept in memory
// only: never persisted, and never echoed back by the extension (which
// deliberately doesn't return passphrases). It lets a host who forgets their
// own password re-reveal it from the Session screen. Keyed by control-server
// port, since each REAPER instance hosts independently. Lost on app restart:
// if we didn't start the host in this run, we simply can't show it.
//
// A passphrase is *staged* when the host command is sent and only *committed*
// when hosting is actually observed shortly after: a refused attempt (unsaved
// project, pre-host warning left open) must not leave a password behind that a
// later, open session would then "reveal".
const byPort = new Map<number, string>();
const staged = new Map<number, { pass: string; at: number }>();
const STAGE_TTL_MS = 15000;

export function stageHostPass(port: number | null, pass: string) {
  if (port == null) return;
  byPort.delete(port);
  staged.set(port, { pass, at: Date.now() });
}

/** Called while hosting: promotes a fresh staged passphrase, drops a stale one. */
export function getHostPass(port: number | null): string {
  if (port == null) return "";
  const s = staged.get(port);
  if (s) {
    staged.delete(port);
    if (s.pass && Date.now() - s.at < STAGE_TTL_MS) byPort.set(port, s.pass);
  }
  return byPort.get(port) ?? "";
}

export function forgetHostPass(port: number | null) {
  if (port == null) return;
  byPort.delete(port);
  staged.delete(port);
}
