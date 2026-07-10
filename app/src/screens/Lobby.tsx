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
import { FOUND_COLOR } from "@/lib/colors";
import { getActivePort, send, sendProfile } from "@/lib/connection";
import { rememberHostPass } from "@/lib/hostPass";
import { formatBytes } from "@/lib/format";
import { t } from "@/lib/i18n";
import { getProfile, saveProfile, PALETTE, type Profile } from "@/lib/profile";
import { initials, type ExtState, type SessionAd } from "@/lib/types";
import { cn } from "@/lib/utils";

// The "only join people you trust" warning is shown once per app run: the
// first join after launch gets it, later joins in the same run don't. Module
// state (not localStorage) is exactly that lifetime: a restart shows it again.
let trustWarningAccepted = false;

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
      initial={{ opacity: 0, y: -3 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, ease: "easeOut" }}
      className="px-0.5 text-[12px] text-red-400"
    >
      {children}
    </motion.p>
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
    <div className="flex items-center gap-3.5 rounded-xl bg-card p-3.5">
      <div
        className="flex size-11 shrink-0 items-center justify-center rounded-full text-sm font-semibold text-black/80 transition-colors duration-300 ease-out"
        style={{ backgroundColor: profile.color }}
      >
        {initials(profile.name || "?")}
      </div>
      <div className="min-w-0 flex-1 space-y-3">
        <Input
          value={profile.name}
          placeholder={t("yourName")}
          maxLength={32}
          className="h-8 border-0 bg-secondary text-[13px]"
          onChange={(e) => onChange({ ...profile, name: e.target.value })}
        />
        <div className="flex gap-1.5 px-0.5">
          {PALETTE.map((c) => (
            <button
              key={c}
              aria-label={`color ${c}`}
              onClick={() => onChange({ ...profile, color: c })}
              className={cn(
                // Keep the hover ring allocated so its shadow fades in and out.
                // The selected swatch uses the shared layout ring below.
                "relative size-4 rounded-full ring-2 ring-transparent ring-offset-2 ring-offset-card transition-shadow duration-200",
                profile.color !== c && "hover:ring-muted-foreground/50",
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
}: {
  session: SessionAd;
  probe?: ExtState["probe"];
  lobbyMountedAt: number;
}) {
  const [pass, setPass] = useState("");
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [checking, setChecking] = useState(false);
  const [pwError, setPwError] = useState(false);
  // Guards against an earlier check's stale result resolving the new one: we
  // only act once we've seen the fresh "in flight" (ok === null) for our check.
  const sawPending = useRef(false);
  const { scope: shakeScope, shake } = useShake();
  // A session discovered while the lobby was already up gets a one-shot
  // found-blue glow so the eye catches it. Cards present when the lobby
  // mounts (or arriving in its first moments) just enter normally.
  const fresh = useRef(Date.now() - lobbyMountedAt > 1500).current;

  const join = () => {
    trustWarningAccepted = true;
    setConfirmOpen(false);
    send({
      cmd: "join",
      session_id: session.session_id,
      session_name: session.session_name,
      passphrase: pass,
    });
  };
  // Passphrase already verified (or none needed): the trust gate shows once per
  // run, otherwise join straight away.
  const proceedJoin = () => {
    if (trustWarningAccepted) join();
    else setConfirmOpen(true);
  };
  // The Join button: for a locked session, verify the passphrase with the owner
  // FIRST (no dialog, no animation, no transition) so a wrong one shows an
  // inline error instead of flashing the whole join. Open sessions join directly.
  const beginJoin = () => {
    if (checking) return;
    if (!session.auth) return proceedJoin();
    if (!pass) return;
    setPwError(false);
    setChecking(true);
    sawPending.current = false;
    send({ cmd: "check", session_id: session.session_id, passphrase: pass });
  };

  // Resolve our in-flight passphrase check from the extension's `probe` field.
  useEffect(() => {
    if (!checking || probe?.session_id !== session.session_id) return;
    if (probe.ok === null || probe.ok === undefined) {
      sawPending.current = true; // fresh check is in flight
      return;
    }
    if (!sawPending.current) return; // ignore a stale prior result
    setChecking(false);
    if (probe.ok) proceedJoin();
    else {
      setPwError(true);
      shake();
    }
  }, [probe, checking]); // eslint-disable-line react-hooks/exhaustive-deps

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
        <motion.span
          aria-hidden
          className="pointer-events-none absolute inset-0 rounded-xl"
          style={{
            boxShadow: `0 0 0 1px ${FOUND_COLOR}80, 0 0 14px ${FOUND_COLOR}40`,
          }}
          initial={{ opacity: 1 }}
          animate={{ opacity: 0 }}
          transition={{ duration: 2.2, ease: "easeOut", delay: 0.2 }}
        />
      )}
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            {session.auth && (
              <LockKeyhole className="size-3.5 shrink-0 text-muted-foreground" />
            )}
            <span className="truncate text-[13px] font-semibold">
              {session.session_name}
            </span>
          </div>
          <p className="truncate text-[12px] text-muted-foreground">
            {session.project_file}
            {(session.bytes ?? 0) > 0 &&
              ` · ${t("filesAndSize", {
                count: session.files ?? 0,
                size: formatBytes(session.bytes ?? 0),
              })}`}
          </p>
        </div>
        <Button
          size="sm"
          className="h-7 w-16 px-3 text-[12px]"
          disabled={(session.auth && !pass) || checking}
          onClick={beginJoin}
        >
          {checking ? <LoaderCircle className="size-3.5 animate-spin" /> : t("join")}
        </Button>
      </div>
      {session.auth && (
        <>
          <div ref={shakeScope}>
            <PasswordInput
              value={pass}
              placeholder={t("password")}
              className={cn(
                "h-8 border-0 bg-secondary text-[13px]",
                pwError && "ring-1 ring-red-500/70",
              )}
              onChange={(e) => {
                setPass(e.target.value);
                setPwError(false);
              }}
              onKeyDown={(e) => e.key === "Enter" && pass && beginJoin()}
            />
          </div>
          {pwError && <ErrorLine>{t("wrongPassword")}</ErrorLine>}
        </>
      )}

      {/* Joining downloads and opens the host's project, and the host can load
          track effects/plugins live during the session; both run code on this
          machine. Make that trust decision explicit before the first join of
          this app run. */}
      <Dialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <DialogContent className="max-w-[340px] gap-4 rounded-2xl">
          <DialogHeader>
            <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-amber-500/15 sm:mx-0">
              <ShieldAlert className="size-5 text-amber-500" />
            </div>
            <DialogTitle className="text-[15px]">{t("trustTitle")}</DialogTitle>
            <DialogDescription className="text-[13px] leading-relaxed">
              {t("trustBody", { name: session.session_name })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2 sm:gap-2">
            <Button
              variant="outline"
              className="flex-1"
              onClick={() => setConfirmOpen(false)}
            >
              {t("cancel")}
            </Button>
            <Button className="flex-1" onClick={join}>
              {t("joinAnyway")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </motion.div>
  );
}

// What the app itself reads out of a pasted invite code: enough to show the
// session name, ask for a passphrase when one is needed, and match the
// extension's `probe` result. The addresses inside are the extension's
// business; the whole raw code is sent along with the join/check commands.
function decodeTicket(raw: string): { sid: string; name: string; auth: boolean } | null {
  const s = raw.trim();
  if (!s.startsWith("RM1:") || s.length > 8192) return null;
  try {
    const b64 = s.slice(4).replace(/-/g, "+").replace(/_/g, "/");
    const pad = b64.length % 4 === 2 ? "==" : b64.length % 4 === 3 ? "=" : "";
    const json = JSON.parse(
      new TextDecoder().decode(Uint8Array.from(atob(b64 + pad), (c) => c.charCodeAt(0))),
    );
    if (json?.v !== 1 || typeof json.sid !== "string" || !json.sid) return null;
    if (!Array.isArray(json.addrs) || json.addrs.length === 0) return null;
    return {
      sid: json.sid,
      name: typeof json.name === "string" ? json.name : "",
      auth: !!json.auth,
    };
  } catch {
    return null;
  }
}

// Join through a pasted invite code: the path to a host outside mDNS reach
// (VPN like Tailscale, multicast-blocking network). Collapsed to one quiet
// line until asked for; once a valid code is pasted it behaves exactly like a
// SessionCard: passphrase pre-check first, the once-per-run trust gate, then
// the join (the extension dials the code's addresses instead of an announced
// session).
function TicketJoin({ probe }: { probe?: ExtState["probe"] }) {
  const [open, setOpen] = useState(false);
  const [raw, setRaw] = useState("");
  const [pass, setPass] = useState("");
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [checking, setChecking] = useState(false);
  const [pwError, setPwError] = useState(false);
  const sawPending = useRef(false);
  const { scope: shakeScope, shake } = useShake();
  const ticket = decodeTicket(raw);
  const invalid = raw.trim().length > 0 && !ticket;
  const name = ticket?.name || t("sessionFallback");

  const join = () => {
    if (!ticket) return;
    trustWarningAccepted = true;
    setConfirmOpen(false);
    send({ cmd: "join", ticket: raw.trim(), session_name: name, passphrase: pass });
  };
  const proceedJoin = () => {
    if (trustWarningAccepted) join();
    else setConfirmOpen(true);
  };
  const beginJoin = () => {
    if (!ticket || checking) return;
    if (!ticket.auth) return proceedJoin();
    if (!pass) return;
    setPwError(false);
    setChecking(true);
    sawPending.current = false;
    send({ cmd: "check", ticket: raw.trim(), passphrase: pass });
  };

  // Same probe reconciliation as SessionCard, keyed on the code's session id.
  // A code check can take a while (the extension is dialing the host), so the
  // spinner may spin for many seconds; that is the honest state.
  useEffect(() => {
    if (!checking || !ticket || probe?.session_id !== ticket.sid) return;
    if (probe.ok === null || probe.ok === undefined) {
      sawPending.current = true;
      return;
    }
    if (!sawPending.current) return;
    setChecking(false);
    if (probe.ok) proceedJoin();
    else {
      setPwError(true);
      shake();
    }
  }, [probe, checking]); // eslint-disable-line react-hooks/exhaustive-deps

  // Collapse back to the one-line link, clearing whatever was typed so it
  // reopens fresh.
  const close = () => {
    setOpen(false);
    setRaw("");
    setPass("");
    setPwError(false);
    setChecking(false);
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
          onClick={() => (open ? close() : setOpen(true))}
          title={open ? undefined : t("ticketJoinHint")}
          className="flex min-w-0 items-center gap-1.5 px-1 py-0.5 text-[11px] font-medium text-muted-foreground transition-colors hover:text-foreground"
        >
          <Ticket className="size-3 shrink-0" />
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
              className="grid size-5 shrink-0 place-items-center rounded text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            >
              <X className="size-3.5" />
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
                    placeholder={t("ticketPlaceholder")}
                    autoFocus
                    className="h-8 border-0 bg-secondary pr-8 text-[13px]"
                    onChange={(e) => {
                      setRaw(e.target.value);
                      setPwError(false);
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
                    className="absolute right-1 top-1/2 grid size-6 -translate-y-1/2 place-items-center rounded text-muted-foreground transition-colors hover:bg-background/60 hover:text-foreground"
                    onClick={async () => {
                      const text = await readClipboard();
                      if (text) {
                        setRaw(text.trim());
                        setPwError(false);
                      }
                    }}
                  >
                    <ClipboardPaste className="size-3.5" />
                  </button>
                </div>
                <Button
                  size="sm"
                  className="h-8 w-16 shrink-0 px-3 text-[12px]"
                  disabled={!ticket || (ticket.auth && !pass) || checking}
                  onClick={beginJoin}
                >
                  {checking ? <LoaderCircle className="size-3.5 animate-spin" /> : t("join")}
                </Button>
              </div>
              {invalid && <ErrorLine>{t("invalidTicket")}</ErrorLine>}
              {ticket && (
                <div className="flex items-center gap-1.5 px-0.5">
                  {ticket.auth && (
                    <LockKeyhole className="size-3.5 shrink-0 text-muted-foreground" />
                  )}
                  <span className="truncate text-[12px] text-muted-foreground">{name}</span>
                </div>
              )}
              {ticket?.auth && (
                <>
                  <div ref={shakeScope}>
                    <PasswordInput
                      value={pass}
                      placeholder={t("password")}
                      className={cn(
                        "h-8 border-0 bg-secondary text-[13px]",
                        pwError && "ring-1 ring-red-500/70",
                      )}
                      onChange={(e) => {
                        setPass(e.target.value);
                        setPwError(false);
                      }}
                      onKeyDown={(e) => e.key === "Enter" && pass && beginJoin()}
                    />
                  </div>
                  {pwError && <ErrorLine>{t("wrongPassword")}</ErrorLine>}
                </>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {/* The same explicit trust decision as joining an announced session:
          a pasted code downloads and runs the host's project all the same. */}
      <Dialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <DialogContent className="max-w-[340px] gap-4 rounded-2xl">
          <DialogHeader>
            <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-amber-500/15 sm:mx-0">
              <ShieldAlert className="size-5 text-amber-500" />
            </div>
            <DialogTitle className="text-[15px]">{t("trustTitle")}</DialogTitle>
            <DialogDescription className="text-[13px] leading-relaxed">
              {t("trustBody", { name })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2 sm:gap-2">
            <Button
              variant="outline"
              className="flex-1"
              onClick={() => setConfirmOpen(false)}
            >
              {t("cancel")}
            </Button>
            <Button className="flex-1" onClick={join}>
              {t("joinAnyway")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function HostCard({ project, error }: { project: string; error: ExtState["error"] }) {
  const [pass, setPass] = useState("");
  // Keep the host button pending until hosting starts or an error arrives.
  // Saving a large project may take time; block duplicate commands meanwhile.
  const [pending, setPending] = useState(false);
  const errKey = error ? JSON.stringify(error) : "";
  const lastErr = useRef(errKey);
  useEffect(() => {
    if (pending && errKey !== lastErr.current) setPending(false);
    lastErr.current = errKey;
  }, [errKey, pending]);
  useEffect(() => {
    if (!pending) return;
    const id = setTimeout(() => setPending(false), 10000); // safety net
    return () => clearTimeout(id);
  }, [pending]);
  const host = () => {
    if (pending) return;
    setPending(true);
    // Keep our own passphrase so the Session screen can re-reveal it if the
    // host forgets it; the extension never echoes it back.
    rememberHostPass(getActivePort(), pass);
    send({ cmd: "host", passphrase: pass });
  };
  return (
    <div className="space-y-2 rounded-xl bg-card p-3">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-semibold">
            {project || t("untitledProject")}
          </span>
          <p className="text-[12px] text-muted-foreground">{t("shareProject")}</p>
        </div>
        <Button
          size="sm"
          className="h-7 px-4 text-[12px]"
          disabled={pending}
          onClick={host}
        >
          {pending ? <LoaderCircle className="size-3.5 animate-spin" /> : t("hostAction")}
        </Button>
      </div>
      <PasswordInput
        value={pass}
        placeholder={t("passwordOptional")}
        className="h-8 border-0 bg-secondary text-[13px]"
        onChange={(e) => setPass(e.target.value)}
      />
    </div>
  );
}

export function Lobby({ state }: { state: ExtState }) {
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
        <div className="flex items-center gap-2.5 rounded-xl bg-amber-500/10 px-3 py-2.5 text-amber-500">
          <LoaderCircle className="size-4 shrink-0 animate-spin" />
          <span className="text-[12px] font-medium">{t("reconnectingBanner")}</span>
        </div>
      )}

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

      <TicketJoin probe={state.probe} />

      <HostCard project={state.project} error={state.error} />
    </div>
  );
}
