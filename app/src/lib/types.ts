// Shapes of the JSON the extension's control server sends. One `state`
// message mirrors the whole extension state; the app holds no other truth.

export type Mode = "idle" | "browsing" | "downloading" | "hosting" | "client";

export interface Peer {
  id: string;
  /** Display name from the peer's presence beacon; "" if they never sent one. */
  name: string;
  /** Identity color (hex). Always present: derived from the peer id if unset. */
  color: string;
  is_self: boolean;
  is_owner: boolean;
}

export interface SessionAd {
  session_id: string;
  session_name: string;
  project_file: string;
  owner: string;
  auth: boolean;
}

export interface ExtState {
  mode: Mode;
  /** Rust-side session state: hosting|browsing|connecting|downloading|ready|failed */
  state: string;
  progress: number;
  session: string;
  /** Name (stem) of the project currently open in this REAPER instance. */
  project: string;
  error: string;
  log: string[];
  sessions: SessionAd[];
  peers: Peer[];
}

/** One running REAPER instance (= one control server port). */
export interface Instance {
  port: number;
  pid?: number;
  state?: ExtState;
}

export function displayName(p: Peer): string {
  if (p.name) return p.name;
  return "Guest";
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}
