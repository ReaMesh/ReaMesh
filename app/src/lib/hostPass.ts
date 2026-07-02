// The passphrase the local user chose when they started hosting, kept in memory
// only: never persisted, and never echoed back by the extension (which
// deliberately doesn't return passphrases). It lets a host who forgets their
// own password re-reveal it from the Session screen. Keyed by control-server
// port, since each REAPER instance hosts independently. Lost on app restart:
// if we didn't start the host in this run, we simply can't show it.
const byPort = new Map<number, string>();

export function rememberHostPass(port: number | null, pass: string) {
  if (port == null) return;
  if (pass) byPort.set(port, pass);
  else byPort.delete(port);
}

export function getHostPass(port: number | null): string {
  return port == null ? "" : byPort.get(port) ?? "";
}

export function forgetHostPass(port: number | null) {
  if (port != null) byPort.delete(port);
}
