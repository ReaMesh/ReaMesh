// What a linked copy (a folder "Save as" made, see src-tauri/src/links.rs) has to
// do with a session on offer. The core checks all of this again when the join
// runs: this only decides what the trust dialog asks.

export interface LinkedCopy {
  /** The folder. */
  path: string;
  /** The session's name when the copy was made. */
  name: string;
  project: string;
  /** Peer id of the host: it can't be copied, unlike a project id. */
  owner: string;
  project_file: string;
  /** The project file's hash as of the last agreement with a session. */
  hash: string;
  /** Kept where it is among the joined copies, not saved to a folder of the user's. */
  kept?: boolean;
}

/** How long the core's sweep lets a joined copy sit unused (`STAGING_MAX_AGE`). */
const COPY_MAX_AGE_DAYS = 7;

/** Days until the sweep may take a copy last changed at `modified` (unix ms),
 *  counting a started day as a whole one. 0 = its time is up, the next sweep takes
 *  it. null when the age isn't known. */
export function copyDaysLeft(modified: number, now: number): number | null {
  if (modified <= 0) return null;
  const left = Math.ceil((modified + COPY_MAX_AGE_DAYS * 864e5 - now) / 864e5);
  return Math.min(COPY_MAX_AGE_DAYS, Math.max(0, left));
}

/** What to call it. A kept copy's folder name carries a session hash. */
export function linkLabel(copy: LinkedCopy): string {
  return copy.kept ? copy.name : (copy.path.split(/[/\\]/).filter(Boolean).pop() ?? copy.path);
}

/** Nothing to do, a plain update, or a local change that a join would replace. */
export type LinkState = "same" | "update" | "conflict";

/** The copy of this project of this host, if any. Newest link first in the list. */
export function findLink(
  copies: LinkedCopy[],
  project: string | undefined,
  owner: string | undefined,
): LinkedCopy | null {
  if (!project || !owner) return null;
  return copies.find((c) => c.project === project && c.owner === owner) ?? null;
}

/** `local` is the hash of the copy's project file, null when it isn't there. */
export function linkStateOf(local: string | null, base: string, hosted: string): LinkState {
  if (local && local === hosted) return "same";
  if (local && local === base) return "update";
  return "conflict";
}

/** What a join sends along for the folder to use; empty = a temporary copy. */
export function joinInto(
  copy: LinkedCopy | null,
  state: LinkState | null,
  { nofx, overwrite }: { nofx: boolean; overwrite: boolean },
): { dest?: string; overwrite?: boolean } {
  // Without effects it is a view-only copy, whatever else there is.
  if (!copy || !state || nofx) return {};
  if (state !== "conflict") return { dest: copy.path };
  return overwrite ? { dest: copy.path, overwrite: true } : {};
}
