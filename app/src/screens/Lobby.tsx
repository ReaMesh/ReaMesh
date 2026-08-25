import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useAnimate, useReducedMotion } from "motion/react";
import { ClipboardPaste, LoaderCircle, LockKeyhole, ShieldAlert, Ticket, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { PasswordInput } from "@/components/PasswordInput";
import { ScrollArea } from "@/components/ui/scroll-area";
import { FirewallBlocked } from "@/components/FirewallNotice";
import { TrustDialog } from "@/components/TrustDialog";
import { FOUND_COLOR, inkFor } from "@/lib/colors";
import { send, sendProfile } from "@/lib/connection";
import { rememberHostPass } from "@/lib/hostPass";
import { recordHostJoin } from "@/lib/knownHosts";
import { formatBytes } from "@/lib/format";
import { t } from "@/lib/i18n";
import { getProfile, saveProfile, PALETTE, type Profile } from "@/lib/profile";
import { decodeTicket, normalizeTicket } from "@/lib/ticket";
import { initials, type ExtState, type SessionAd } from "@/lib/types";
import { topRightGutter } from "@/components/TitleBar";
import { cn, focusRing } from "@/lib/utils";

/// Shortest passphrase accepted by the host form. This is only a minimum for
/// avoiding misleadingly weak protected sessions, not a security guarantee.
const MIN_PASSPHRASE = 3;

/// Deadline for a passphrase pre-check.
///
/// The extension has one probe slot, so a later check can replace an earlier
/// one. The deadline also covers stale sessions and rejected invite codes.
const CHECK_TIMEOUT_MS = 12_000;
/// Ticket checks legitimately take longer: the extension is dialing a host that
/// may be across a VPN, and only fast-fails after its own connect timeout.
const TICKET_CHECK_TIMEOUT_MS = 35_000;

type CheckOutcome = "ok" | "bad" | "unknown";

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
    finish(probe.ok ? "ok" : "bad");
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

/** "Hosted by X · " prefix for a session card. The name is self-reported by the
 *  host on a public topic, so it is deliberately styled as an ordinary label
 *  and never as a verified identity. */
function HostedBy({ session }: { session: SessionAd }) {
  if (!session.owner_name) return null;
  return (
    <>
      <span
        aria-hidden="true"
        className="mr-1 inline-block size-1.5 shrink-0 rounded-full align-middle"
        style={{ backgroundColor: session.owner_color || undefined }}
      />
      <span className="text-foreground/80">{session.owner_name}</span>
      {" · "}
    </>
  );
}

/** Warn when different peers claim the same session id. */
function ContestedWarning() {
  return (
    <div
      role="status"
      className="flex items-start gap-2 rounded-lg bg-amber-500/10 px-2.5 py-2 text-amber-500"
    >
      <ShieldAlert className="mt-px size-3.5 shrink-0" aria-hidden="true" />
      <span className="text-[11px] leading-relaxed">{t("sessionContested")}</span>
    </div>
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

function ProfileRow({
  profile,
  onChange,
}: {
  profile: Profile;
  onChange: (p: Profile) => void;
}) {
  return (
    // Extra right padding on Windows: this is the lobby's topmost card, and
    // the Settings gear floats over its top-right corner. Padding rather than
    // a narrower card, so the card's edge still lines up with the ones below.
    <div className={cn("flex items-center gap-3.5 rounded-xl bg-card p-3.5", topRightGutter)}>
      <div
        className="flex size-11 shrink-0 items-center justify-center rounded-full text-sm font-semibold transition-colors duration-300 ease-out"
        style={{ backgroundColor: profile.color, color: inkFor(profile.color) }}
      >
        {initials(profile.name || "?")}
      </div>
      <div className="min-w-0 flex-1 space-y-3">
        <Input
          value={profile.name}
          aria-label={t("yourName")}
          placeholder={t("yourName")}
          maxLength={32}
          className="h-8 border-0 bg-secondary text-[13px]"
          onChange={(e) => onChange({ ...profile, name: e.target.value })}
        />
        {/* Expose color selection as a radio group for assistive technology. */}
        <div
          role="radiogroup"
          aria-label={t("identityColor")}
          className="flex gap-1.5 px-0.5"
        >
          {PALETTE.map((c, i) => (
            <button
              key={c}
              type="button"
              role="radio"
              aria-checked={profile.color === c}
              aria-label={t("identityColorN", { n: i + 1 })}
              onClick={() => onChange({ ...profile, color: c })}
              className={cn(
                // Keep the hover ring allocated so its shadow fades in and out.
                // The selected swatch uses the shared layout ring below.
                "relative size-4 rounded-full ring-2 ring-transparent ring-offset-2 ring-offset-card transition-shadow duration-200",
                profile.color !== c && "hover:ring-muted-foreground/50",
                focusRing,
              )}
              style={{ backgroundColor: c }}
            >
              {profile.color === c && (
                // Animate one selection ring between swatches. Its inset and border
                // match the hover ring's dimensions.
                <motion.span
                  layoutId="profile-color-ring"
                  className="absolute -inset-1 rounded-full border-2 border-foreground/70"
                  transition={{ type: "spring", stiffness: 500, damping: 35 }}
                />
              )}
            </button>
          ))}
        </div>
      </div>
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
  const [error, setError] = useState<"" | "bad" | "unknown">("");
  // Prevent a second join command while the first one starts downloading.
  const [joining, setJoining] = useJoinPending();
  const { scope: shakeScope, shake } = useShake();
  // Highlight sessions discovered after the initial lobby render.
  // Skip the highlight for sessions present on mount or shortly afterward.
  const fresh = useRef(Date.now() - lobbyMountedAt > 1500).current;

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
      // A refused probe for an open session means the host did not answer.
      setError(session.auth ? outcome : "unknown");
      shake();
    },
  );

  // Probe first to verify the passphrase and load the join preview.
  const beginJoin = () => {
    if (checking || joining) return;
    if (session.auth && !pass) return;
    setError("");
    begin();
    send({ cmd: "check", session_id: session.session_id, passphrase: pass }, port ?? undefined);
  };
  const busy = checking || joining;

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 8, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.97 }}
      transition={{ type: "spring", stiffness: 350, damping: 28 }}
      className="relative flex flex-col gap-2 rounded-xl bg-card p-3"
    >
      {fresh && (
        // Briefly tint sessions discovered after the lobby opened.
        <motion.span
          aria-hidden
          className="pointer-events-none absolute inset-0 rounded-xl"
          initial={{ backgroundColor: `${FOUND_COLOR}33` }}
          animate={{ backgroundColor: `${FOUND_COLOR}00` }}
          transition={{ duration: 0.9, ease: "easeOut", delay: 0.15 }}
        />
      )}
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            {session.auth && <LockIcon />}
            <span
              className="truncate text-[13px] font-semibold"
              title={session.session_name}
            >
              {session.session_name}
            </span>
          </div>
          <p
            className="truncate text-[12px] text-muted-foreground"
            title={session.project_file}
          >
            <HostedBy session={session} />
            {session.project_file}
            {(session.bytes ?? 0) > 0 &&
              ` · ${t("filesAndSize", {
                count: session.files ?? 0,
                size: formatBytes(session.bytes ?? 0),
              })}`}
          </p>
        </div>
        <PendingButton
          size="sm"
          className="h-7 w-16 px-3 text-[12px]"
          disabled={(session.auth && !pass) || busy}
          busy={busy}
          label={t("join")}
          onClick={beginJoin}
        />
      </div>
      {session.contested && <ContestedWarning />}
      {session.auth && (
        <>
          <div ref={shakeScope}>
            <PasswordInput
              value={pass}
              aria-label={t("password")}
              placeholder={t("password")}
              className={cn(
                "h-8 border-0 bg-secondary text-[13px]",
                error && "ring-1 ring-red-500/70",
              )}
              onChange={(e) => {
                setPass(e.target.value);
                setError("");
              }}
              onKeyDown={(e) => e.key === "Enter" && pass && beginJoin()}
            />
          </div>
          {error && (
            <ErrorLine>
              {error === "bad" ? t("wrongPassword") : t("checkNoAnswer")}
            </ErrorLine>
          )}
        </>
      )}

      {/* Confirm the files, plugins, and claimed host before joining. */}
      <TrustDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        name={session.session_name}
        hostName={session.owner_name}
        hostPeerId={session.owner}
        bytes={session.bytes}
        files={session.files}
        preview={probe?.session_id === session.session_id ? probe?.preview : undefined}
        onJoin={join}
      />
    </motion.div>
  );
}

// Ticket parsing lives in lib/ticket.ts (unit-tested; mirrors the extension's
// parse_ticket so the app never accepts a code the extension would refuse).

// Join hosts that are not discoverable through mDNS by pasting an invite code.
function TicketJoin({ probe, port }: { probe?: ExtState["probe"]; port: number | null }) {
  const [open, setOpen] = useState(false);
  const [raw, setRaw] = useState("");
  const [pass, setPass] = useState("");
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [error, setError] = useState<"" | "bad" | "unknown">("");
  const [joining, setJoining] = useJoinPending();
  const { scope: shakeScope, shake } = useShake();
  const ticket = decodeTicket(raw);
  const code = normalizeTicket(raw);
  const invalid = code.length > 0 && !ticket;
  const name = ticket?.name || t("sessionFallback");

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
      setError(ticket?.auth ? outcome : "unknown");
      shake();
    },
  );

  // Probe first to verify the passphrase and load the join preview.
  const beginJoin = () => {
    if (!ticket || checking || joining) return;
    if (ticket.auth && !pass) return;
    setError("");
    begin();
    send({ cmd: "check", ticket: code, passphrase: pass }, port ?? undefined);
  };
  const busy = checking || joining;

  // Collapse back to the one-line link, clearing whatever was typed so it
  // reopens fresh.
  const close = () => {
    setOpen(false);
    setRaw("");
    setPass("");
    setError("");
    setJoining(false);
    reset();
  };

  return (
    // Keep the invite header mounted while its form expands below it.
    <div
      className={cn(
        "shrink-0 rounded-xl transition-colors duration-300",
        open && "bg-card",
      )}
    >
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
            transition={{ type: "spring", stiffness: 400, damping: 38 }}
            className="overflow-hidden"
          >
            <div className="space-y-2 p-3 pt-1">
              <div className="flex items-center gap-2">
                <div className="relative min-w-0 flex-1">
                  <Input
                    value={raw}
                    aria-label={t("haveInviteCode")}
                    placeholder={t("ticketPlaceholder")}
                    autoFocus
                    className="h-8 border-0 bg-secondary pr-8 text-[13px]"
                    onChange={(e) => {
                      setRaw(e.target.value);
                      setError("");
                    }}
                    onKeyDown={(e) =>
                      e.key === "Enter" && ticket && (!ticket.auth || pass) && beginJoin()
                    }
                  />
                  {/* Paste from clipboard: the common gesture for a copied code. */}
                  <button
                    type="button"
                    title={t("pasteInvite")}
                    aria-label={t("pasteInvite")}
                    className={cn(
                      "absolute right-1 top-1/2 grid size-6 -translate-y-1/2 place-items-center rounded text-muted-foreground transition-colors hover:bg-background/60 hover:text-foreground",
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
                <PendingButton
                  size="sm"
                  className="h-8 w-16 shrink-0 px-3 text-[12px]"
                  disabled={!ticket || (ticket.auth && !pass) || busy}
                  busy={busy}
                  label={t("join")}
                  onClick={beginJoin}
                />
              </div>
              {invalid && <ErrorLine>{t("invalidTicket")}</ErrorLine>}
              {ticket && (
                <div className="flex items-center gap-1.5 px-0.5">
                  {ticket.auth && <LockIcon />}
                  <span className="truncate text-[12px] text-muted-foreground" title={name}>
                    {name}
                  </span>
                </div>
              )}
              {ticket?.auth && (
                <>
                  <div ref={shakeScope}>
                    <PasswordInput
                      value={pass}
                      aria-label={t("password")}
                      placeholder={t("password")}
                      className={cn(
                        "h-8 border-0 bg-secondary text-[13px]",
                        error && "ring-1 ring-red-500/70",
                      )}
                      onChange={(e) => {
                        setPass(e.target.value);
                        setError("");
                      }}
                      onKeyDown={(e) => e.key === "Enter" && pass && beginJoin()}
                    />
                  </div>
                  {error && (
                    <ErrorLine>
                      {error === "bad" ? t("wrongPassword") : t("checkNoAnswer")}
                    </ErrorLine>
                  )}
                </>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* Use the same confirmation for discovered and invite-code sessions. */}
      <TrustDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        name={name}
        hostPeerId={ticket?.peer}
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
  // Keep the host button pending until hosting starts or an error arrives.
  // Saving a large project may take time; block duplicate commands meanwhile.
  const [pending, setPending] = useState(false);
  // If a passphrase is supplied, enforce the minimum length.
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
    setPending(true);
    // Keep our own passphrase so the Session screen can re-reveal it if the
    // host forgets it; the extension never echoes it back.
    rememberHostPass(port, pass);
    send(confirm ? { cmd: "host", passphrase: pass, confirm: true } : { cmd: "host", passphrase: pass }, port);
  };
  const dismissWarning = () => send({ cmd: "host_dismiss" }, port ?? undefined);
  const w = warning ?? null;
  return (
    <div className="space-y-2 rounded-xl bg-card p-3">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <span
            className="block truncate text-[13px] font-semibold"
            title={project || undefined}
          >
            {project || t("untitledProject")}
          </span>
          <p className="text-[12px] text-muted-foreground">{t("shareProject")}</p>
        </div>
        <PendingButton
          size="sm"
          className="h-7 px-4 text-[12px]"
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
        className={cn(
          "h-8 border-0 bg-secondary text-[13px]",
          tooShort && "ring-1 ring-red-500/70",
        )}
        onChange={(e) => setPass(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && host(false)}
      />
      {tooShort && <ErrorLine>{t("passwordTooShort", { min: MIN_PASSPHRASE })}</ErrorLine>}

      {/* Confirm media size, free space, and cloud-folder warnings. */}
      <Dialog open={!!w} onOpenChange={(open) => !open && dismissWarning()}>
        <DialogContent className="max-w-[340px] gap-4 rounded-2xl">
          <DialogHeader>
            <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-amber-500/15 sm:mx-0">
              <ShieldAlert className="size-5 text-amber-500" aria-hidden="true" />
            </div>
            <DialogTitle className="text-[15px]">{t("hostCopyTitle")}</DialogTitle>
            <DialogDescription asChild>
              <div className="space-y-2 text-[13px] leading-relaxed">
                {w && w.files > 0 && (
                  <p>
                    {t("hostCopyBody", { count: w.files, size: formatBytes(w.bytes) })}{" "}
                    {w.free_known && (
                      <span className={w.enough ? undefined : "font-medium text-red-400"}>
                        {w.enough
                          ? t("hostFreeOk", { free: formatBytes(w.free) })
                          : t("hostFreeNotEnough", { free: formatBytes(w.free) })}
                      </span>
                    )}
                  </p>
                )}
                {w?.cloud && <p className="text-amber-500">{t("hostCloudWarning")}</p>}
              </div>
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2 sm:gap-2">
            <Button variant="outline" className="flex-1" onClick={dismissWarning}>
              {t("cancel")}
            </Button>
            <Button
              className="flex-1"
              onClick={() => {
                dismissWarning();
                host(true);
              }}
            >
              {t("hostAnyway")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export function Lobby({ state, port }: { state: ExtState; port: number | null }) {
  const [profile, setProfile] = useState(getProfile);
  // Sessions discovered after this timestamp get a one-time highlight.
  const mountedAt = useRef(Date.now()).current;
  const updateProfile = (p: Profile) => {
    setProfile(p);
    saveProfile(p);
    sendProfile();
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 p-4">
      {/* The extension lost the host without a farewell and is watching the
          lobby to rejoin the same session automatically. */}
      {state.reconnecting && (
        <div
          role="status"
          className="flex items-center gap-2.5 rounded-xl bg-amber-500/10 px-3 py-2.5 text-amber-500"
        >
          <LoaderCircle
            className="size-4 shrink-0 motion-safe:animate-spin"
            aria-hidden="true"
          />
          <span className="text-[12px] font-medium">{t("reconnectingBanner")}</span>
        </div>
      )}

      {/* Show a repair path for an active Windows Firewall block rule. */}
      <FirewallBlocked />

      <ProfileRow profile={profile} onChange={updateProfile} />

      <div className="flex min-h-0 flex-1 flex-col gap-2">
        <h3 className="px-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          {t("sessionsOnNetwork")}
        </h3>
        {state.sessions.length > 0 ? (
          <ScrollArea className="min-h-0 flex-1 -mx-1 px-1">
            <div className="space-y-2">
              <AnimatePresence mode="popLayout">
                {state.sessions.map((s) => (
                  <SessionCard
                    key={s.session_id}
                    session={s}
                    probe={state.probe}
                    lobbyMountedAt={mountedAt}
                    port={port}
                  />
                ))}
              </AnimatePresence>
            </div>
          </ScrollArea>
        ) : (
          /* Fill the empty lobby without a permanent discovery spinner. */
          <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 pb-4 text-center">
            <div className="space-y-1">
              <p className="text-[13px] font-medium">{t("noSessionsYet")}</p>
              <p className="mx-auto max-w-[250px] text-[12px] leading-relaxed text-muted-foreground">
                {t("noSessionsHint")}
              </p>
            </div>
          </div>
        )}
      </div>

      <TicketJoin probe={state.probe} port={port} />

      <HostCard
        project={state.project}
        error={state.error}
        warning={state.host_warning}
        port={port}
      />
    </div>
  );
}
