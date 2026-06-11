// Local profile: a display name and identity color, persisted on this
// machine only (no accounts). Sent to every REAPER instance on connect and
// whenever it changes; the extension gossips it to session peers as a
// presence beacon.

export interface Profile {
  name: string;
  color: string;
}

/** Identity palette for the dark theme. */
export const PALETTE = [
  "#f06464", // coral
  "#f0a05a", // amber
  "#e8d35c", // gold
  "#7ed27e", // green
  "#5cc8c8", // teal
  "#6aa6f0", // blue
  "#9d8cf0", // violet
  "#e87ec8", // pink
];

const KEY = "reamesh.profile";

export function getProfile(): Profile {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const p = JSON.parse(raw);
      if (typeof p.name === "string" && typeof p.color === "string") return p;
    }
  } catch {
    // fall through to default
  }
  return { name: "", color: PALETTE[Math.floor(Math.random() * PALETTE.length)] };
}

export function saveProfile(p: Profile) {
  localStorage.setItem(KEY, JSON.stringify(p));
}
