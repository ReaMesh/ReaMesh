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
  // Without a token (an extension from before the token, or a browser
  // preview against a stand-in server) connect plainly: an extension that
  // wants a token then only sends its `hello`, marked `auth: false`, which is
  // still enough to show a version mismatch (see connection.ts).
  return token ? new WebSocket(url, [`reamesh.${token}`]) : new WebSocket(url);
}
