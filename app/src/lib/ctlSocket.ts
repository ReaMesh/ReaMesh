// Opens a WebSocket to the extension's control server on `port`.
//
// The server only accepts a connection that presents its per-run token as the
// WebSocket subprotocol `reamesh.<token>`. The token sits in a file only this
// user can read; the app backend hands it over (`ctl_token`), so a web page
// that merely knows the port has no way in.

import { invoke } from "@tauri-apps/api/core";

export async function openCtlSocket(port: number): Promise<WebSocket> {
  let token: string | null = null;
  try {
    token = (await invoke<string | null>("ctl_token", { port })) ?? null;
  } catch {
    token = null;
  }
  const url = `ws://127.0.0.1:${port}`;
  // Without a token (no server on that port yet, or a browser preview against
  // a stand-in server) the real extension refuses the handshake, which reads
  // as "nobody there" and is retried on the usual cadence.
  return token ? new WebSocket(url, [`reamesh.${token}`]) : new WebSocket(url);
}
