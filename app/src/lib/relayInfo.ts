// What a relay says about itself (`info` in the `relay` control message) turned
// into what the app shows: calm unless something doesn't work. The texts come
// from the relay's operator and are shown as plain text, nothing more.

import { t, resolveLang, type MsgKey } from "./i18n";
import { compareVersions } from "./version";

export interface RelayNotice {
  text: string;
  /** Unix seconds, 0 = open-ended. */
  from: number;
  until: number;
}

export interface RelayAbout {
  ok?: boolean;
  /** Why a key was refused ("expired"), "" otherwise. */
  reason?: string;
  version?: string;
  /** Below what this app needs, or too old to say anything at all. */
  outdated?: boolean;
  mode?: string;
  budget_percent?: number;
  budget_exhausted?: boolean;
  key_budget_percent?: number;
  key_budget_exhausted?: boolean;
  /** When the budgets start over, Unix seconds. */
  budget_resets?: number;
  hosts?: number;
  max_hosts?: number;
  notice?: RelayNotice | null;
  /** The operator's own details: only shown when something is wrong. */
  name?: string;
  contact?: string;
  key_expires?: number;
  stopping?: boolean;
  /** The relay's code with a changed address, to store instead. */
  new_code?: string;
}

/** From this share of a budget on, the app says so. */
const BUDGET_WARN_PERCENT = 90;
/** A notice shows this long ahead of its start. */
const NOTICE_AHEAD_SECS = 7 * 24 * 60 * 60;
const DAY_SECS = 24 * 60 * 60;
/** Say a key is running out from this many days on. */
const KEY_EXPIRY_SOON_DAYS = 14;

export const nowSecs = () => Math.floor(Date.now() / 1000);

/** Over or too far off: not worth a line. */
export function noticeApplies(n: RelayNotice | null | undefined, now = nowSecs()): n is RelayNotice {
  if (!n?.text) return false;
  if (n.until !== 0 && n.until <= now) return false;
  return n.from === 0 || n.from - now <= NOTICE_AHEAD_SECS;
}

function dayAndTime(secs: number, lang: string): { day: string; time: string } {
  const d = new Date(secs * 1000);
  return {
    day: d.toLocaleDateString(lang, { weekday: "short", day: "numeric", month: "short" }),
    time: d.toLocaleTimeString(lang, { hour: "2-digit", minute: "2-digit" }),
  };
}

/** The notice's time window in this machine's time zone and language; "" when
 *  it has none. */
export function noticeWhen(n: RelayNotice, lang: string = resolveLang()): string {
  const from = n.from ? dayAndTime(n.from, lang) : null;
  const until = n.until ? dayAndTime(n.until, lang) : null;
  if (from && until) {
    return from.day === until.day
      ? `${from.day}, ${from.time}–${until.time}`
      : `${from.day}, ${from.time} – ${until.day}, ${until.time}`;
  }
  if (from) return t("relayNoticeFrom", { when: `${from.day}, ${from.time}` });
  if (until) return t("relayNoticeUntil", { when: `${until.day}, ${until.time}` });
  return "";
}

/** A scheduled window that has begun: whatever it announces is happening now. */
export function noticeActive(n: RelayNotice, now = nowSecs()): boolean {
  return n.from !== 0 && n.from <= now;
}

export function noticeLine(n: RelayNotice, lang: string = resolveLang()): string {
  const when = noticeWhen(n, lang);
  return when ? `${n.text}, ${when}` : n.text;
}

function shortDate(secs: number, lang: string): string {
  return new Date(secs * 1000).toLocaleDateString(lang, { day: "numeric", month: "long" });
}

export interface RelayNote {
  key: MsgKey;
  params?: Record<string, string | number>;
  /** Something doesn't or soon won't work. */
  warn: boolean;
}

/** What is worth saying about a relay besides whether it is online, most
 *  pressing first. The notice isn't among them: it has its own line. */
export function relayNotes(a: RelayAbout | undefined, now = nowSecs(), lang: string = resolveLang()): RelayNote[] {
  if (!a) return [];
  const notes: RelayNote[] = [];
  if (a.outdated) notes.push({ key: "relayOutdated", warn: true });
  const resets = a.budget_resets ? shortDate(a.budget_resets, lang) : "";
  if (a.budget_exhausted) {
    notes.push({ key: "relayBudgetUsedUp", params: { date: resets }, warn: true });
  } else if (a.key_budget_exhausted) {
    notes.push({ key: "relayKeyBudgetUsedUp", params: { date: resets }, warn: true });
  } else if ((a.budget_percent ?? 0) >= BUDGET_WARN_PERCENT) {
    notes.push({ key: "relayBudgetNearly", params: { percent: a.budget_percent ?? 0 }, warn: true });
  } else if ((a.key_budget_percent ?? 0) >= BUDGET_WARN_PERCENT) {
    notes.push({ key: "relayKeyBudgetNearly", params: { percent: a.key_budget_percent ?? 0 }, warn: true });
  } else if (a.mode === "coordination") {
    notes.push({ key: "relayCoordination", warn: false });
  }
  if (a.key_expires && a.key_expires > now && a.key_expires - now <= KEY_EXPIRY_SOON_DAYS * DAY_SECS) {
    // `key_expires` is the first second it no longer works; people think in
    // the last day it does.
    notes.push({ key: "relayKeyExpires", params: { date: shortDate(a.key_expires - 1, lang) }, warn: false });
  }
  return notes;
}

/** "Relay 1.2.0, update available" when it is older than this app. Calm: a
 *  relay catches up by itself, and an older one keeps working. */
export function relayVersionLine(a: RelayAbout | undefined, appVersion: string): string {
  if (!a?.version) return "";
  const older = !!appVersion && compareVersions(a.version, appVersion) < 0;
  return t(older ? "relayVersionOlder" : "relayVersion", { version: a.version });
}
