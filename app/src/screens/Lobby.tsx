import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useAnimate, useReducedMotion } from "motion/react";
import {
  Bluetooth,
  ChevronDown,
  ClipboardPaste,
  LoaderCircle,
  LockKeyhole,
  Ticket,
  X,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFact,
  DialogFacts,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { PasswordInput } from "@/components/PasswordInput";
import { BanList } from "@/components/BanList";
import { Divider, Group, SectionLabel } from "@/components/Group";
import { ProfileFields } from "@/components/ProfileEditor";
import { FirewallBlocked } from "@/components/FirewallNotice";
import { TrustDialog } from "@/components/TrustDialog";
import { FOUND_COLOR, inkFor } from "@/lib/colors";
import { send } from "@/lib/connection";
import { stageHostPass } from "@/lib/hostPass";
import { takeInvite, useInviteSeq } from "@/lib/invite";
import { recordHostJoin } from "@/lib/knownHosts";
import { formatBytes } from "@/lib/format";
import { t } from "@/lib/i18n";
import { getProfile, useProfile } from "@/lib/profile";
import { nearbyBlocked, setNearby, useNearby } from "@/lib/nearby";
import { decodeTicket, normalizeTicket } from "@/lib/ticket";
import { initials, type ExtState, type SessionAd } from "@/lib/types";
import { protoSkew } from "@/lib/version";
import { cn, focusRing } from "@/lib/utils";

/// Shortest passphrase accepted by the host form. This is only a minimum for
/// avoiding misleadingly weak protected sessions, not a security guarantee.
const MIN_PASSPHRASE = 3;

/// Deadline for a passphrase pre-check.
///
/// The extension has one probe slot, so a later check can replace an earlier
/// one. The deadline also covers stale sessions and rejected invite links.
const CHECK_TIMEOUT_MS = 12_000;
/// Ticket checks legitimately take longer: the extension is dialing a host that
/// may be across a VPN, and only fast-fails after its own connect timeout.
const TICKET_CHECK_TIMEOUT_MS = 35_000;

/** The spring the Lobby's cards unfold on, the one the invite card always had:
 *  no hard start, no overshoot, settled after about 0.3 s. */
const FOLD = { type: "spring", stiffness: 400, damping: 38 } as const;

type CheckOutcome = "ok" | "bad" | "banned" | "version" | "unverified" | "unknown";
type CheckError = "" | Exclude<CheckOutcome, "ok">;

/** Inline text under the passphrase field for a failed check. */
function checkErrorText(error: CheckError): string {
  if (error === "bad") return t("wrongPassword");
  if (error === "banned") return t("bannedFromSession");
  if (error === "version") return t("hostOtherVersion");
  if (error === "unverified") return t("hostUnverified");
  return t("checkNoAnswer");
}

/** Run a passphrase pre-check until it answers, is replaced, or times out. */
function usePassphraseCheck(
  sid: string | undefined,
  probe: ExtState["probe"],
  timeoutMs: number,
  onResult: (outcome: CheckOutcome) => void,
) {
  const [checking, setChecking] = useState(false);
  // Ignore old results until this check reports that it is in flight.
  const sawPending = useRef(false);
  const cb = useRef(onResult);
  cb.current = onResult;

  const finish = (outcome: CheckOutcome) => {
    setChecking(false);
    cb.current(outcome);
  };

  useEffect(() => {
    if (!checking) return;
    if (!sid) return;
    if (probe?.session_id !== sid) {
      // Another check replaced this probe, so its answer will never arrive.
      if (sawPending.current) finish("unknown");
      return;
    }
    if (probe.ok === null || probe.ok === undefined) {
      sawPending.current = true; // fresh check is in flight
      return;
    }
    if (!sawPending.current) return; // ignore a stale prior result
    // Only a refused token is a wrong passphrase; a host that never answered
    // (timeout, unreachable, gone) must not be reported as one. A blocked
    // device gets its own answer, whatever it typed.
    finish(
      probe.ok
        ? "ok"
        : probe.reason === "rejected"
          ? "bad"
          : probe.reason === "banned"
            ? "banned"
            : probe.reason === "version"
              ? "version"
              : probe.reason === "unverified"
                ? "unverified"
                : "unknown",
    );
  }, [probe, checking, sid]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!checking) return;
    const id = setTimeout(() => finish("unknown"), timeoutMs);
    return () => clearTimeout(id);
  }, [checking, timeoutMs]); // eslint-disable-line react-hooks/exhaustive-deps

  return {
    checking,
    begin: () => {
      sawPending.current = false;
      setChecking(true);
    },
    reset: () => setChecking(false),
  };
}

/// Stop a pending join if the fire-and-forget command is refused without a
/// response. Successful joins leave the lobby before this deadline.
const JOIN_TIMEOUT_MS = 10_000;

function useJoinPending(): [boolean, React.Dispatch<React.SetStateAction<boolean>>] {
  const [joining, setJoining] = useState(false);
  useEffect(() => {
    if (!joining) return;
    const id = setTimeout(() => setJoining(false), JOIN_TIMEOUT_MS);
    return () => clearTimeout(id);
  }, [joining]);
  return [joining, setJoining];
}

// Read the clipboard for the Paste button. Returns "" if the webview denies
// clipboard read (the user can still paste with the keyboard).
async function readClipboard(): Promise<string> {
  try {
    return (await navigator.clipboard.readText()) ?? "";
  } catch {
    return "";
  }
}

// Shake rejected passphrases without remounting or moving focus.
function useShake() {
  const [scope, animate] = useAnimate();
  const reduce = useReducedMotion();
  const shake = () => {
    if (reduce || !scope.current) return;
    animate(scope.current, { x: [0, -7, 6, -4, 3, 0] }, { duration: 0.4, ease: "easeOut" });
  };
  return { scope, shake };
}

/** Fading slide-in for the small red "wrong password" line. */
function ErrorLine({ children }: { children: React.ReactNode }) {
  return (
    <motion.p
      role="status"
      initial={{ opacity: 0, y: -3 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: "easeOut" }}
      className="px-0.5 text-[12px] text-red-400"
    >
      {children}
    </motion.p>
  );
}

/** Add accessible text to the otherwise decorative lock icon. */
function LockIcon() {
  return (
    <span className="flex shrink-0 items-center" title={t("passwordProtected")}>
      <LockKeyhole className="size-3.5 text-muted-foreground" aria-hidden="true" />
      <span className="sr-only">{t("passwordProtected")}</span>
    </span>
  );
}

/** Keep the button's accessible name while its visible label becomes a spinner. */
function PendingButton({
  busy,
  label,
  className,
  ...rest
}: React.ComponentProps<typeof Button> & { busy: boolean; label: string }) {
  return (
    <Button {...rest} aria-busy={busy || undefined} className={className}>
      {busy ? (
        <>
          <LoaderCircle className="size-3.5 motion-safe:animate-spin" aria-hidden="true" />
          <span className="sr-only">{label}</span>
        </>
      ) : (
        label
      )}
    </Button>
  );
}

/** Animates its own height when what is inside changes size (a pasted code
 *  turning into its session, an error line), on the same spring. */
function AutoHeight({ children }: { children: React.ReactNode }) {
  const inner = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | "auto">("auto");
  useLayoutEffect(() => {
    const el = inner.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.offsetHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return (
    <motion.div initial={false} animate={{ height }} transition={FOLD} className="overflow-hidden">
      <div ref={inner}>{children}</div>
    </motion.div>
  );
}

// Hosting or joining without a name would put a nameless face in everyone
// else's roster. The Host / Join buttons ask the name card to open instead.
const nameAsks = new Set<() => void>();
/** True (and the name card asked to open) when there is no name yet. */
function needName(): boolean {
  if (getProfile().name.trim()) return false;
  nameAsks.forEach((ask) => ask());
  return true;
}

/** Who you are, as the Lobby's first row. The row itself becomes the head of
 *  the name + colour card: the card's background fades in around it, the head
 *  eases into its padding, and the fields grow in below on the spring. A
 *  nameless first run opens it unasked, instead of silently joining as
 *  "Guest". */
function NameCard() {
  const profile = useProfile();
  const named = profile.name.trim().length > 0;
  // Decided once per visit, so typing the first letter doesn't fold it away.
  const [open, setOpen] = useState(() => !named);
  // Bumped by a Host / Join click without a name: remounts the fields so the
  // name input takes focus again, and shows why the click did nothing else.
  const [asked, setAsked] = useState(0);
  useEffect(() => {
    const ask = () => {
      setOpen(true);
      setAsked((n) => n + 1);
    };
    nameAsks.add(ask);
    return () => {
      nameAsks.delete(ask);
    };
  }, []);
  return (
    <div className={cn("shrink-0 rounded-xl transition-colors duration-300", open && "bg-card")}>
      <button
        type="button"
        aria-expanded={open}
        title={open ? undefined : t("editProfile")}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          "flex w-full items-center gap-2.5 rounded-xl px-1 py-1.5 text-left transition-[padding] duration-300",
          open && "px-3 pt-2.5",
          focusRing,
        )}
      >
        <span
          aria-hidden="true"
          className="flex size-6 shrink-0 items-center justify-center rounded-full text-[11px] font-semibold transition-colors duration-300"
          style={{ backgroundColor: profile.color, color: inkFor(profile.color) }}
        >
          {initials(profile.name)}
        </span>
        <span
          className={cn(
            "min-w-0 truncate text-[13px]",
            named ? "font-semibold" : "text-muted-foreground",
          )}
        >
          {named ? profile.name : t("yourName")}
        </span>
        <motion.span
          className="ml-auto flex shrink-0 text-muted-foreground"
          initial={false}
          animate={{ rotate: open ? 180 : 0 }}
          transition={FOLD}
        >
          <ChevronDown className="size-4" aria-hidden="true" />
        </motion.span>
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            key="fields"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={FOLD}
            className="overflow-hidden"
          >
            {/* Indented to the name above, past the avatar. */}
            <div className="pt-1 pr-3 pb-3 pl-[46px]">
              <ProfileFields
                key={asked}
                autoFocus
                onDone={() => named && setOpen(false)}
              />
              {asked > 0 && !named && (
                <p role="status" className="mt-2 text-[12px] text-amber-500">
                  {t("nameFirst")}
                </p>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** The host's self-reported face. A contested session gets a neutral one: two
 *  devices claim it, so neither name deserves the trust a face implies. So
 *  does a host that never said its name. */
function HostAvatar({ session }: { session: SessionAd }) {
  const known = !session.contested && !!session.owner_name;
  return (
    <div
      aria-hidden="true"
      className={cn(
        "flex size-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold",
        !known && "bg-[#2a2a31] text-muted-foreground",
      )}
      style={
        known
          ? { backgroundColor: session.owner_color, color: inkFor(session.owner_color) }
          : undefined
      }
    >
      {known ? initials(session.owner_name ?? "") : "?"}
    </div>
  );
}

function SessionCard({
  session,
  probe,
  lobbyMountedAt,
  port,
}: {
  session: SessionAd;
  probe?: ExtState["probe"];
  lobbyMountedAt: number;
  /** The instance this card was rendered for. Every command names it
   *  explicitly, so a command can never land on a different REAPER because the
   *  active instance changed between render and click. */
  port: number | null;
}) {
  const [pass, setPass] = useState("");
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [error, setError] = useState<CheckError>("");
  // Prevent a second join command while the first one starts downloading.
  const [joining, setJoining] = useJoinPending();
  const { scope: shakeScope, shake } = useShake();
  // A session discovered while the lobby was already up gets a one-shot
  // found-blue wash so the eye catches it. Cards present when the lobby
  // mounts (or arriving in its first moments) just enter normally.
  const fresh = useRef(Date.now() - lobbyMountedAt > 1500).current;
  // Another build can't be joined: say which side needs the update instead.
  const skew = protoSkew(session.proto);

  const join = (nofx: boolean) => {
    if (joining) return;
    setJoining(true);
    setConfirmOpen(false);
    // "You've joined this host N times" is a memory of trusting this identity.
    recordHostJoin(session.owner);
    send(
      {
        cmd: "join",
        session_id: session.session_id,
        session_name: session.session_name,
        passphrase: pass,
        nofx,
      },
      port ?? undefined,
    );
  };

  const { checking, begin } = usePassphraseCheck(
    session.session_id,
    probe,
    CHECK_TIMEOUT_MS,
    (outcome) => {
      if (outcome === "ok") return setConfirmOpen(true);
      // A refused probe for an open session means the host did not answer,
      // unless the host said outright that this device is blocked.
      setError(
        session.auth || outcome === "banned" || outcome === "version" || outcome === "unverified"
          ? outcome
          : "unknown",
      );
      shake();
    },
  );

  // Probe first to verify the passphrase and load the join preview.
  const beginJoin = () => {
    if (checking || joining) return;
    if (session.auth && !pass) return;
    if (needName()) return;
    setError("");
    begin();
    send({ cmd: "check", session_id: session.session_id, passphrase: pass }, port ?? undefined);
  };
  const busy = checking || joining;

  // Both join buttons look the same; the one beside a password field is as
  // tall as the field.
  const joinButton = (besideField: boolean) => (
    <PendingButton
      size="sm"
      variant="secondary"
      className={cn(
        "shrink-0 px-3",
        besideField ? "h-8 min-w-[84px] text-[13px]" : "h-7 min-w-[76px] text-[12px]",
      )}
      disabled={(session.auth && !pass) || busy || skew !== "same"}
      busy={busy}
      label={t("join")}
      onClick={beginJoin}
    />
  );

  return (
    <motion.div
      // Position only: a full layout animation scale-tweens the row's
      // contents when the error line below appears, which reads as the row
      // stretching for a moment before the refusal shows.
      layout="position"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.2, ease: "easeOut" }}
      className="relative flex items-start gap-3 px-3 py-2.5"
    >
      {fresh && (
        // Briefly tint sessions discovered after the lobby opened.
        <motion.span
          aria-hidden
          className="pointer-events-none absolute inset-0"
          initial={{ backgroundColor: `${FOUND_COLOR}33` }}
          animate={{ backgroundColor: `${FOUND_COLOR}00` }}
          transition={{ duration: 0.9, ease: "easeOut", delay: 0.15 }}
        />
      )}
      <HostAvatar session={session} />
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex min-h-8 items-center gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              <span className="truncate text-[13px] font-semibold" title={session.session_name}>
                {session.session_name}
              </span>
              {session.auth && <LockIcon />}
            </div>
            {/* One line: who hosts it. A second device claiming the same
                session takes that line over. */}
            {session.contested ? (
              <p role="status" className="truncate text-[12px] text-amber-500">
                {t("sessionContested")}
              </p>
            ) : skew !== "same" ? (
              <p className="truncate text-[12px] text-amber-500">
                {t(skew === "newer" ? "hostNewerVersion" : "hostOlderVersion")}
              </p>
            ) : (
              <p className="truncate text-[12px] text-muted-foreground" title={session.project_file}>
                {session.owner_name || session.project_file}
              </p>
            )}
          </div>
          {!session.auth && joinButton(false)}
        </div>
        {/* Always mounted (empty:hidden when idle) so a failed probe on an open
            session still gets its "no answer" line and the shake. */}
        <div ref={shakeScope} className="mt-2 space-y-1.5 empty:hidden">
          {session.auth && (
            <div className="flex gap-2">
              <PasswordInput
                value={pass}
                aria-label={t("passwordFor", { name: session.session_name })}
                placeholder={t("password")}
                className={cn(
                  "h-8 border-0 bg-muted text-[13px]",
                  error && "ring-1 ring-red-500/70",
                )}
                onChange={(e) => {
                  setPass(e.target.value);
                  setError("");
                }}
                onKeyDown={(e) => e.key === "Enter" && pass && beginJoin()}
              />
              {joinButton(true)}
            </div>
          )}
          {error && <ErrorLine>{checkErrorText(error)}</ErrorLine>}
        </div>
      </div>

      {/* Confirm the size, plugins, and claimed host before joining. */}
      <TrustDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        name={session.session_name}
        hostId={session.owner}
        hostName={session.contested ? undefined : session.owner_name}
        bytes={session.bytes}
        preview={probe?.session_id === session.session_id ? probe?.preview : undefined}
        onJoin={join}
      />
    </motion.div>
  );
}

// Ticket parsing lives in lib/ticket.ts (unit-tested; mirrors the extension's
// parse_ticket so the app never accepts a link the extension would refuse).

// Join hosts that are not discoverable through mDNS through an invite code:
// pasted here, or opened from a chat/browser (the OS hands it to the app, see
// lib/invite). A quiet link under the list that opens into a card in place:
// the link stays put and becomes the card's head.
function InviteCard({
  sessions,
  probe,
  port,
}: {
  sessions: SessionAd[];
  probe?: ExtState["probe"];
  port: number | null;
}) {
  const [open, setOpen] = useState(false);
  const [raw, setRaw] = useState("");
  const [pass, setPass] = useState("");
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [error, setError] = useState<CheckError>("");
  const [joining, setJoining] = useJoinPending();
  const { scope: shakeScope, shake } = useShake();
  const ticket = decodeTicket(raw);
  const code = normalizeTicket(raw);
  const invalid = code.length > 0 && !ticket;
  const name = ticket?.name || t("sessionFallback");
  // The code names no host, but the check waits for the pinned host's own
  // signed announce, and that carries the name it goes by. Matched on the
  // pinned peer id, so another device announcing the same id can't lend it one.
  const hostName = ticket
    ? sessions.find((s) => s.session_id === ticket.sid && s.owner === ticket.peer)?.owner_name
    : undefined;

  const join = (nofx: boolean) => {
    if (!ticket || joining) return;
    setJoining(true);
    setConfirmOpen(false);
    recordHostJoin(ticket.peer);
    send(
      { cmd: "join", ticket: code, session_name: name, passphrase: pass, nofx },
      port ?? undefined,
    );
  };

  // Invite checks get a longer deadline because the host may be across a VPN.
  const { checking, begin, reset } = usePassphraseCheck(
    ticket?.sid,
    probe,
    TICKET_CHECK_TIMEOUT_MS,
    (outcome) => {
      if (outcome === "ok") return setConfirmOpen(true);
      setError(
        ticket?.auth || outcome === "banned" || outcome === "version" || outcome === "unverified"
          ? outcome
          : "unknown",
      );
      shake();
    },
  );

  // Probe first to verify the passphrase and load the join preview.
  const startCheck = (code: string, pass: string) => {
    setError("");
    begin();
    send({ cmd: "check", ticket: code, passphrase: pass }, port ?? undefined);
  };
  const beginJoin = () => {
    if (!ticket || checking || joining) return;
    if (ticket.auth && !pass) return;
    if (needName()) return;
    startCheck(code, pass);
  };
  const busy = checking || joining;

  // A link the OS opened the app with: fill it in and wait. Nothing is dialed
  // until the user presses Join (or enters the passphrase), so a link opened
  // by a web page or another app can't make ReaMesh contact a host on its own.
  // Also runs on mount, for a link that arrived while another screen was up.
  const inviteSeq = useInviteSeq();
  useEffect(() => {
    const link = takeInvite();
    if (!link) return;
    setOpen(true);
    setRaw(link);
    setPass("");
    setError("");
  }, [inviteSeq]); // eslint-disable-line react-hooks/exhaustive-deps

  // The X in the head: fold back to the one-line link and forget the code, so
  // it reopens fresh.
  const close = () => {
    setOpen(false);
    setRaw("");
    setPass("");
    setError("");
    setJoining(false);
    reset();
  };

  return (
    // Keep the head mounted while the card's body unfolds below it.
    <div className={cn("shrink-0 rounded-xl transition-colors duration-300", open && "bg-card")}>
      <div
        className={cn(
          "flex items-center justify-between transition-[padding] duration-300",
          open && "p-2 pb-1",
        )}
      >
        <button
          type="button"
          aria-expanded={open}
          onClick={() => (open ? close() : setOpen(true))}
          title={open ? undefined : t("ticketJoinHint")}
          className={cn(
            "flex min-w-0 items-center gap-1.5 rounded px-1 py-0.5 text-[11px] font-medium text-muted-foreground transition-colors hover:text-foreground",
            focusRing,
          )}
        >
          <Ticket className="size-3 shrink-0" aria-hidden="true" />
          <span className="truncate">{t("haveInviteCode")}</span>
        </button>
        <AnimatePresence initial={false}>
          {open && (
            <motion.button
              key="close"
              type="button"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.15 }}
              onClick={close}
              aria-label={t("close")}
              className={cn(
                "grid size-5 shrink-0 place-items-center rounded text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground",
                focusRing,
              )}
            >
              <X className="size-3.5" aria-hidden="true" />
            </motion.button>
          )}
        </AnimatePresence>
      </div>

      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            key="body"
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={FOLD}
            className="overflow-hidden"
          >
            <AutoHeight>
              <div className="space-y-2 px-3 pt-1 pb-3">
                {ticket ? (
                  // The code has done its job: what is left is the session,
                  // as the list above would show it.
                  <div className="flex min-h-7 items-center gap-3 px-0.5">
                    <div className="flex min-w-0 flex-1 items-center gap-1.5">
                      <span className="truncate text-[13px] font-semibold" title={name}>
                        {name}
                      </span>
                      {ticket.auth && <LockIcon />}
                    </div>
                    {!ticket.auth && (
                      <PendingButton
                        size="sm"
                        variant="secondary"
                        autoFocus
                        className="h-7 min-w-[76px] shrink-0 px-3 text-[12px]"
                        disabled={busy}
                        busy={busy}
                        label={t("join")}
                        onClick={beginJoin}
                      />
                    )}
                  </div>
                ) : (
                  <div className="relative">
                    <Input
                      value={raw}
                      aria-label={t("haveInviteCode")}
                      placeholder={t("ticketPlaceholder")}
                      spellCheck={false}
                      autoFocus
                      className="h-8 border-0 bg-muted pr-8 text-[13px]"
                      onChange={(e) => {
                        setRaw(e.target.value);
                        setError("");
                      }}
                    />
                    <button
                      type="button"
                      title={t("pasteInvite")}
                      aria-label={t("pasteInvite")}
                      className={cn(
                        "absolute top-1/2 right-1 grid size-6 -translate-y-1/2 place-items-center rounded text-muted-foreground transition-colors hover:bg-background/60 hover:text-foreground",
                        focusRing,
                      )}
                      onClick={async () => {
                        const text = await readClipboard();
                        if (text) {
                          setRaw(text);
                          setError("");
                        }
                      }}
                    >
                      <ClipboardPaste className="size-3.5" aria-hidden="true" />
                    </button>
                  </div>
                )}
                {invalid && <ErrorLine>{t("invalidTicket")}</ErrorLine>}
                <div ref={shakeScope} className="space-y-2 empty:hidden">
                  {ticket?.auth && (
                    <div className="flex gap-2">
                      <PasswordInput
                        value={pass}
                        autoFocus
                        aria-label={t("passwordFor", { name })}
                        placeholder={t("password")}
                        className={cn(
                          "h-8 border-0 bg-muted text-[13px]",
                          error && "ring-1 ring-red-500/70",
                        )}
                        onChange={(e) => {
                          setPass(e.target.value);
                          setError("");
                        }}
                        onKeyDown={(e) => e.key === "Enter" && pass && beginJoin()}
                      />
                      <PendingButton
                        size="sm"
                        variant="secondary"
                        className="h-8 min-w-[84px] shrink-0 px-3 text-[13px]"
                        disabled={!pass || busy}
                        busy={busy}
                        label={t("join")}
                        onClick={beginJoin}
                      />
                    </div>
                  )}
                  {error && <ErrorLine>{checkErrorText(error)}</ErrorLine>}
                </div>
              </div>
            </AutoHeight>
          </motion.div>
        )}
      </AnimatePresence>

      {/* Use the same confirmation for discovered and invite-link sessions. */}
      <TrustDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        name={name}
        hostId={ticket?.peer}
        hostName={hostName}
        preview={ticket && probe?.session_id === ticket.sid ? probe?.preview : undefined}
        onJoin={join}
      />
    </div>
  );
}

function HostCard({
  project,
  error,
  warning,
  port,
}: {
  project: string;
  error: ExtState["error"];
  /** Warning that pauses hosting until the user confirms or cancels. */
  warning: ExtState["host_warning"];
  port: number | null;
}) {
  const [pass, setPass] = useState("");
  // Pending until the mode flips to hosting (which unmounts the Lobby) or the
  // host attempt fails (a new error arrives, e.g. "save the project first").
  // Saving a large project before hosting takes a moment; without this the
  // button looks dead and invites double-clicks.
  const [pending, setPending] = useState(false);
  // A passphrase is optional, but if there is one it has to be worth the
  // padlock the session card will wear.
  const tooShort = pass.length > 0 && pass.length < MIN_PASSPHRASE;

  // Log events have no unique id, so clear pending on any error received while
  // this host attempt is active.
  const errKey = error ? JSON.stringify(error) : "";
  useEffect(() => {
    if (pending && errKey) setPending(false);
  }, [errKey, pending]);
  // Stop the spinner while the user decides about a host warning.
  useEffect(() => {
    if (pending && warning) setPending(false);
  }, [warning, pending]);
  useEffect(() => {
    if (!pending) return;
    const id = setTimeout(() => setPending(false), 10000); // safety net
    return () => clearTimeout(id);
  }, [pending]);

  const host = (confirm: boolean) => {
    if (pending || tooShort) return;
    // Do not enter a pending state if there is no socket to send through.
    if (port === null) return;
    if (needName()) return;
    setPending(true);
    // Stage the passphrase locally; commit it only after hosting starts.
    stageHostPass(port, pass);
    send(confirm ? { cmd: "host", passphrase: pass, confirm: true } : { cmd: "host", passphrase: pass }, port);
  };
  const dismissWarning = () => send({ cmd: "host_dismiss" }, port ?? undefined);
  const w = warning ?? null;
  return (
    <div className="flex shrink-0 flex-col gap-2.5 rounded-xl bg-card p-3">
      <div className="flex min-h-7 items-center gap-3">
        <span
          className="min-w-0 flex-1 truncate text-[13px] font-semibold"
          title={project || undefined}
        >
          {project || t("untitledProject")}
        </span>
        <PendingButton
          size="sm"
          className="h-7 min-w-[68px] px-3 text-[12px]"
          disabled={pending || tooShort}
          busy={pending}
          label={t("hostAction")}
          onClick={() => host(false)}
        />
      </div>
      <PasswordInput
        value={pass}
        aria-label={t("passwordOptional")}
        placeholder={t("passwordOptional")}
        className={cn("h-8 border-0 bg-muted text-[13px]", tooShort && "ring-1 ring-red-500/70")}
        onChange={(e) => setPass(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && host(false)}
      />
      {tooShort && <ErrorLine>{t("passwordTooShort", { min: MIN_PASSPHRASE })}</ErrorLine>}

      {/* Confirm media size, free space, and cloud-folder warnings. The row
          labels say what the numbers mean. */}
      <Dialog open={!!w} onOpenChange={(open) => !open && dismissWarning()}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("hostCopyTitle")}</DialogTitle>
          </DialogHeader>
          {w && w.files > 0 && (
            <DialogFacts>
              <DialogFact label={t("hostCopyLabel")}>
                <span className="tabular-nums">{formatBytes(w.bytes)}</span>
              </DialogFact>
              {w.free_known && (
                <DialogFact label={t("hostFreeLabel")}>
                  <span className="tabular-nums">{formatBytes(w.free)}</span>
                  {/* Worth a word only when it doesn't fit. */}
                  {!w.enough && (
                    <span className="font-normal text-red-400">{t("hostFreeShort")}</span>
                  )}
                </DialogFact>
              )}
            </DialogFacts>
          )}
          {w?.cloud && (
            <p className="text-[12px] leading-normal text-amber-500">{t("hostCloudWarning")}</p>
          )}
          <DialogFooter>
            <Button variant="secondary" onClick={dismissWarning}>
              {t("cancel")}
            </Button>
            <Button
              onClick={() => {
                dismissWarning();
                host(true);
              }}
            >
              {/* Copying media is just what hosting this project takes; only a
                  real problem (no room, a syncing cloud folder) gets "anyway". */}
              {w && (w.cloud || (w.files > 0 && w.free_known && !w.enough))
                ? t("hostAnyway")
                : t("hostCopyAndHost")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** A VPN can hide the sessions on this network (LAN discovery doesn't cross
 *  it, and a full tunnel may cut the local network off). Said once, quietly,
 *  where it explains something: under an empty session list. Re-checked while
 *  that list stays empty, so switching the VPN off makes it go away. */
function VpnHint() {
  const [on, setOn] = useState(false);
  useEffect(() => {
    let alive = true;
    const check = () =>
      invoke<boolean>("vpn_active")
        .then((v) => alive && setOn(v))
        .catch(() => {}); // plain-browser development: no backend
    check();
    const id = setInterval(check, 10_000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, []);
  if (!on) return null;
  return (
    <p className="mt-1.5 text-[11.5px] leading-normal text-muted-foreground/80">{t("vpnHint")}</p>
  );
}

/** How long the Lobby stays empty before it offers Bluetooth. */
const NEARBY_OFFER_DELAY_MS = 10_000;

/** Bluetooth under an empty Lobby. Some networks (guest Wi-Fi) block the usual
 *  search; once nothing has shown up for a while, and only until the user
 *  decided either way, one quiet button turns the Bluetooth search on (which
 *  is what asks macOS for the permission). Once on, the search runs silently;
 *  only a refused permission is worth a line. */
function NearbyLine() {
  const n = useNearby();
  const [waited, setWaited] = useState(false);
  useEffect(() => {
    const id = setTimeout(() => setWaited(true), NEARBY_OFFER_DELAY_MS);
    return () => clearTimeout(id);
  }, []);
  if (!n.supported) return null;
  if (n.enabled === true) {
    if (!nearbyBlocked(n)) return null;
    return (
      <p className="mt-1.5 text-[11.5px] leading-normal text-muted-foreground/80">
        {t("nearbyBlocked")}
      </p>
    );
  }
  if (n.enabled === false || !waited || nearbyBlocked(n)) return null;
  return (
    <button
      type="button"
      onClick={() => setNearby(true)}
      title={t("nearbyHostHint")}
      className={cn(
        "mx-auto mt-2 flex items-center gap-1.5 rounded px-1 py-0.5 text-[11px] font-medium text-muted-foreground transition-colors hover:text-foreground",
        focusRing,
      )}
    >
      <Bluetooth className="size-3 shrink-0" aria-hidden="true" />
      {t("nearbyOffer")}
    </button>
  );
}

/** The extension lost the host without a farewell and is watching the lobby
 *  to rejoin the same session on its own: one quiet row, nothing to do. */
function ReconnectRow({ log }: { log: ExtState["log"] }) {
  const name = [...log].reverse().find((e) => e.code === "reconnecting")?.params?.name;
  return (
    <div role="status" className="flex shrink-0 items-center gap-2.5 rounded-xl bg-card px-3 py-2.5">
      <LoaderCircle
        className="size-4 shrink-0 text-muted-foreground motion-safe:animate-spin"
        aria-hidden="true"
      />
      <span className="min-w-0 truncate text-[12px] text-foreground/65">
        {name ? t("reconnectingTo", { name: String(name) }) : t("reconnectingBanner")}
      </span>
    </div>
  );
}

export function Lobby({ state, port }: { state: ExtState; port: number | null }) {
  // Sessions discovered after this timestamp get a one-time highlight.
  const mountedAt = useRef(Date.now()).current;
  const count = state.sessions.length;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-[18px] px-4 pt-1.5 pb-4">
      {state.reconnecting && <ReconnectRow log={state.log} />}

      {/* Show a repair path for an active Windows Firewall block rule. */}
      <FirewallBlocked />

      <NameCard />

      <section className="flex min-h-0 flex-1 flex-col gap-2">
        <SectionLabel>{t("sessionsOnNetwork")}</SectionLabel>
        <Group className="min-h-0 shrink">
          {count > 0 ? (
            <div className="scroll-slim min-h-0 overflow-y-auto">
              <AnimatePresence mode="popLayout" initial={false}>
                {state.sessions.map((s, i) => (
                  <motion.div key={s.session_id} layout="position" exit={{ opacity: 0 }}>
                    {i > 0 && <Divider />}
                    <SessionCard
                      session={s}
                      probe={state.probe}
                      lobbyMountedAt={mountedAt}
                      port={port}
                    />
                  </motion.div>
                ))}
              </AnimatePresence>
            </div>
          ) : (
            /* Quiet on purpose: no permanent discovery spinner. */
            <div className="px-6 py-6 text-center">
              <p className="text-[13px] text-muted-foreground">{t("noSessionsYet")}</p>
              <VpnHint />
              <NearbyLine />
            </div>
          )}
        </Group>
        {/* A session you can't see here is still a session. */}
        <InviteCard sessions={state.sessions} probe={state.probe} port={port} />
      </section>

      {/* Blocks outlive the session they were made in: keep them revocable. */}
      <BanList bans={state.bans ?? []} port={port} />

      {/* No section label: the card's own Host button already says it. */}
      <section className="flex shrink-0 flex-col gap-2">
        <HostCard
          project={state.project}
          error={state.error}
          warning={state.host_warning}
          port={port}
        />
      </section>
    </div>
  );
}
