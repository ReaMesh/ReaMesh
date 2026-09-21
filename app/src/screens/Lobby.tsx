import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion, useAnimate, useReducedMotion } from "motion/react";
import {
  AudioLines,
  ChevronDown,
  ClipboardPaste,
  LoaderCircle,
  LockKeyhole,
  Radio,
  ShieldAlert,
  Ticket,
  Users,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFact,
  DialogFacts,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { PasswordInput } from "@/components/PasswordInput";
import { BanList } from "@/components/BanList";
import { Divider, Group, IconTile, SectionLabel } from "@/components/Group";
import { SURFACE_EASE } from "@/components/ProfileChip";
import { ProfileEditor } from "@/components/ProfileEditor";
import { FirewallBlocked } from "@/components/FirewallNotice";
import { TrustDialog } from "@/components/TrustDialog";
import { FOUND_COLOR, inkFor } from "@/lib/colors";
import { send } from "@/lib/connection";
import { stageHostPass } from "@/lib/hostPass";
import { takeInvite, useInviteSeq } from "@/lib/invite";
import { recordHostJoin } from "@/lib/knownHosts";
import { formatBytes } from "@/lib/format";
import { t } from "@/lib/i18n";
import {
  getProfile,
  setProfileEditorOpen,
  setProfileInlineHost,
  useProfileEditor,
} from "@/lib/profile";
import { decodeTicket, normalizeTicket } from "@/lib/ticket";
import { initials, type ExtState, type SessionAd } from "@/lib/types";
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

type CheckOutcome = "ok" | "bad" | "banned" | "unknown";
type CheckError = "" | Exclude<CheckOutcome, "ok">;

/** Inline text under the passphrase field for a failed check. */
function checkErrorText(error: CheckError): string {
  if (error === "bad") return t("wrongPassword");
  if (error === "banned") return t("bannedFromSession");
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

/** Warn when different peers claim the same session id. */
function ContestedWarning() {
  return (
    <div
      role="status"
      className="mt-2 flex items-start gap-2 rounded-[10px] bg-amber-500/10 px-2.5 py-2 text-amber-500"
    >
      <ShieldAlert className="mt-px size-3.5 shrink-0" aria-hidden="true" />
      <span className="text-[11.5px] leading-snug">{t("sessionContested")}</span>
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

/** The identity editor as a card at the top of the Lobby, unfolding out of the
 *  title-bar chip. On a first run it opens by itself and asks once who this
 *  is, instead of silently joining as "Guest". */
function ProfileCard({ firstRun }: { firstRun: boolean }) {
  return (
    // Height, not a transform: the list below is pushed down frame by frame
    // and nothing inside is ever scaled. The negative margin eats the parent's
    // flex gap while the card is folded away.
    <motion.div
      initial={{ height: 0, opacity: 0, marginBottom: -18 }}
      animate={{ height: "auto", opacity: 1, marginBottom: 0 }}
      exit={{ height: 0, opacity: 0, marginBottom: -18 }}
      transition={SURFACE_EASE}
      className="shrink-0 overflow-hidden rounded-xl"
    >
      <div className="flex flex-col gap-3 bg-card p-3.5">
        <div className="flex items-center justify-between gap-2">
          <h2 className="text-[13px] font-semibold">
            {firstRun ? t("firstRunTitle") : t("editProfile")}
          </h2>
          <button
            type="button"
            aria-label={t("collapse")}
            onClick={() => setProfileEditorOpen(false)}
            className={cn(
              "flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-white/10 hover:text-foreground",
              focusRing,
            )}
          >
            <motion.span
              className="flex"
              initial={{ rotate: 0 }}
              animate={{ rotate: 180 }}
              exit={{ rotate: 0 }}
              transition={SURFACE_EASE}
            >
              <ChevronDown className="size-4" aria-hidden="true" />
            </motion.span>
          </button>
        </div>
        <ProfileEditor autoFocus ringOffset="ring-offset-card" />
      </div>
    </motion.div>
  );
}

/** The host's self-reported face. A contested session gets a neutral one: two
 *  devices claim it, so neither name deserves the trust a face implies. */
function HostAvatar({ session }: { session: SessionAd }) {
  if (session.contested || !session.owner_name) {
    return (
      <div
        aria-hidden="true"
        className="flex size-8 shrink-0 items-center justify-center rounded-full bg-[#2a2a31] text-muted-foreground"
      >
        <Users className="size-[15px]" />
      </div>
    );
  }
  return (
    <div
      aria-hidden="true"
      className="flex size-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold"
      style={{ backgroundColor: session.owner_color, color: inkFor(session.owner_color) }}
    >
      {initials(session.owner_name)}
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
      setError(session.auth || outcome === "banned" ? outcome : "unknown");
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

  const meta = [
    !session.contested && session.owner_name ? session.owner_name : session.project_file,
    (session.bytes ?? 0) > 0
      ? t("previewFiles", { count: session.files ?? 0, size: formatBytes(session.bytes ?? 0) })
      : null,
  ]
    .filter(Boolean)
    .join(" · ");
  const joinButton = (primary: boolean) => (
    <PendingButton
      size="sm"
      variant={primary ? "default" : "secondary"}
      className={cn("shrink-0 px-3", primary ? "h-8 min-w-[84px] text-[13px]" : "h-7 min-w-[76px] text-[12px]")}
      disabled={(session.auth && !pass) || busy}
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
            <p className="truncate text-[12px] text-muted-foreground" title={session.project_file}>
              {meta}
            </p>
          </div>
          {!session.auth && joinButton(false)}
        </div>
        {session.contested && <ContestedWarning />}
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

      {/* Confirm the files, plugins, and claimed host before joining. */}
      <TrustDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        name={session.session_name}
        hostId={session.owner}
        hostName={session.contested ? undefined : session.owner_name}
        hostColor={session.owner_color}
        bytes={session.bytes}
        files={session.files}
        preview={probe?.session_id === session.session_id ? probe?.preview : undefined}
        onJoin={join}
      />
    </motion.div>
  );
}

// Ticket parsing lives in lib/ticket.ts (unit-tested; mirrors the extension's
// parse_ticket so the app never accepts a link the extension would refuse).

// Join hosts that are not discoverable through mDNS through an invite link:
// pasted here, or opened from a chat/browser (the OS hands it to the app, see
// lib/invite).
function TicketJoin({ probe, port }: { probe?: ExtState["probe"]; port: number | null }) {
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
      setError(ticket?.auth || outcome === "banned" ? outcome : "unknown");
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
    startCheck(code, pass);
  };
  const busy = checking || joining;

  // A link the OS opened the app with: fill it in and, when the session takes
  // no passphrase, go straight on to the host check (and so the trust dialog);
  // a protected one waits here for the passphrase. Also runs on mount, for a
  // link that arrived while another screen was up.
  const inviteSeq = useInviteSeq();
  useEffect(() => {
    const link = takeInvite();
    if (!link) return;
    const info = decodeTicket(link);
    setRaw(link);
    setPass("");
    setError("");
    if (info && !info.auth && !joining) startCheck(normalizeTicket(link), "");
  }, [inviteSeq]); // eslint-disable-line react-hooks/exhaustive-deps

  // Emptying the field drops whatever check was running for the old link.
  useEffect(() => {
    if (code) return;
    setPass("");
    setError("");
    setJoining(false);
    reset();
  }, [code]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    // The list's last row: a session you can't see here is still a session.
    <div className="flex shrink-0 items-start gap-3 py-2 pl-3 pr-2">
      <IconTile>
        <Ticket />
      </IconTile>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex h-8 items-center gap-1">
          <input
            value={raw}
            aria-label={t("haveInviteCode")}
            placeholder={t("ticketPlaceholder")}
            title={t("ticketJoinHint")}
            spellCheck={false}
            className="h-8 min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-muted-foreground/80"
            onChange={(e) => {
              setRaw(e.target.value);
              setError("");
            }}
            onKeyDown={(e) => e.key === "Enter" && ticket && (!ticket.auth || pass) && beginJoin()}
          />
          {ticket && !ticket.auth ? (
            <PendingButton
              size="sm"
              variant="secondary"
              className="h-7 min-w-[76px] shrink-0 px-3 text-[12px]"
              disabled={busy}
              busy={busy}
              label={t("join")}
              onClick={beginJoin}
            />
          ) : (
            !ticket && (
              <Button
                variant="ghost"
                size="sm"
                className="h-7 shrink-0 gap-1.5 px-2.5 text-[12px] text-foreground/65"
                onClick={async () => {
                  const text = await readClipboard();
                  if (text) {
                    setRaw(text);
                    setError("");
                  }
                }}
              >
                <ClipboardPaste className="size-3.5" aria-hidden="true" />
                {t("pasteInvite")}
              </Button>
            )
          )}
        </div>
        {invalid && <ErrorLine>{t("invalidTicket")}</ErrorLine>}
        {ticket && (
          <div className="flex items-center gap-1.5 pb-1 pr-1">
            <span className="truncate text-[12px] text-muted-foreground" title={name}>
              {name}
            </span>
            {ticket.auth && <LockIcon />}
          </div>
        )}
        <div ref={shakeScope} className="space-y-1.5 pr-1 empty:hidden">
          {ticket?.auth && (
            <div className="flex gap-2 pt-1 pb-1">
              <PasswordInput
                value={pass}
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

      {/* Use the same confirmation for discovered and invite-link sessions. */}
      <TrustDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        name={name}
        hostId={ticket?.peer}
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
    setPending(true);
    // Stage the passphrase locally; commit it only after hosting starts.
    stageHostPass(port, pass);
    send(confirm ? { cmd: "host", passphrase: pass, confirm: true } : { cmd: "host", passphrase: pass }, port);
  };
  const dismissWarning = () => send({ cmd: "host_dismiss" }, port ?? undefined);
  const w = warning ?? null;
  return (
    <div className="flex shrink-0 flex-col gap-2.5 rounded-xl bg-card p-3">
      <div className="flex items-center gap-3">
        <IconTile className="text-foreground/65 [&_svg]:size-4">
          <AudioLines />
        </IconTile>
        <div className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-semibold" title={project || undefined}>
            {project || t("untitledProject")}
          </span>
          <p className="truncate text-[12px] text-muted-foreground">{t("yourOpenProject")}</p>
        </div>
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

      {/* Confirm media size, free space, and cloud-folder warnings. */}
      <Dialog open={!!w} onOpenChange={(open) => !open && dismissWarning()}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("hostCopyTitle")}</DialogTitle>
            {w && w.files > 0 && <DialogDescription>{t("hostCopyWhy")}</DialogDescription>}
          </DialogHeader>
          {w && w.files > 0 && (
            <DialogFacts>
              <DialogFact label={t("hostCopyLabel")}>
                <span className="tabular-nums">
                  {t("previewFiles", { count: w.files, size: formatBytes(w.bytes) })}
                </span>
              </DialogFact>
              {w.free_known && (
                <DialogFact label={t("hostFreeLabel")}>
                  <span className="tabular-nums">{formatBytes(w.free)}</span>
                  <span className={cn("font-normal", w.enough ? "text-emerald-400" : "text-red-400")}>
                    {w.enough ? t("hostFreeEnough") : t("hostFreeShort")}
                  </span>
                </DialogFact>
              )}
            </DialogFacts>
          )}
          {w?.cloud && (
            <p className="rounded-[10px] bg-amber-500/10 px-2.5 py-2 text-[12px] leading-snug text-amber-500">
              {t("hostCloudWarning")}
            </p>
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
              {t("hostAnyway")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export function Lobby({ state, port }: { state: ExtState; port: number | null }) {
  // Sessions discovered after this timestamp get a one-time highlight.
  const mountedAt = useRef(Date.now()).current;
  // Decided once per visit, so typing the first letter does not retitle the card.
  const firstRun = useRef(!getProfile().name.trim()).current;
  // While the Lobby is up the identity editor unfolds here, as part of the
  // page; a nameless first run opens it unasked.
  const editor = useProfileEditor();
  useEffect(() => {
    setProfileInlineHost(true);
    if (firstRun) setProfileEditorOpen(true);
    // Leaving the Lobby (a join, hosting) folds the card back into the chip
    // rather than letting it reappear as a panel over the next screen.
    return () => {
      setProfileEditorOpen(false);
      setProfileInlineHost(false);
    };
  }, [firstRun]);
  const count = state.sessions.length;

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-[18px] px-4 pt-1.5 pb-4">
      {/* The extension lost the host without a farewell and is watching the
          lobby to rejoin the same session automatically. */}
      {state.reconnecting && (
        <div
          role="status"
          className="flex shrink-0 items-center gap-2.5 rounded-xl bg-amber-500/10 px-3 py-2.5 text-amber-500"
        >
          <LoaderCircle className="size-4 shrink-0 motion-safe:animate-spin" aria-hidden="true" />
          <span className="text-[12px] font-medium">{t("reconnectingBanner")}</span>
        </div>
      )}

      {/* Show a repair path for an active Windows Firewall block rule. */}
      <FirewallBlocked />

      <AnimatePresence initial={false}>
        {editor.open && editor.inline && <ProfileCard key="profile" firstRun={firstRun} />}
      </AnimatePresence>

      <section className="flex min-h-0 flex-1 flex-col gap-2">
        <SectionLabel aside={count > 0 ? t("sessionsFound", { count }) : undefined}>
          {t("sessionsOnNetwork")}
        </SectionLabel>
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
            <div className="flex flex-col items-center gap-2 px-6 py-6 text-center">
              <Radio className="size-5 text-muted-foreground/70" aria-hidden="true" />
              <p className="text-[13px] font-semibold">{t("noSessionsYet")}</p>
            </div>
          )}
          <Divider inset={false} />
          <TicketJoin probe={state.probe} port={port} />
        </Group>
      </section>

      {/* Blocks outlive the session they were made in: keep them revocable. */}
      <BanList bans={state.bans ?? []} port={port} />

      <section className="flex shrink-0 flex-col gap-2">
        <SectionLabel>{t("hostSection")}</SectionLabel>
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
