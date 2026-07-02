import { useEffect, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { LoaderCircle, LockKeyhole, ShieldAlert } from "lucide-react";
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
import { ScrollArea } from "@/components/ui/scroll-area";
import { send, sendProfile } from "@/lib/connection";
import { formatBytes } from "@/lib/format";
import { t } from "@/lib/i18n";
import { getProfile, saveProfile, PALETTE, type Profile } from "@/lib/profile";
import { initials, type ExtState, type SessionAd } from "@/lib/types";
import { cn } from "@/lib/utils";

// The "only join people you trust" warning is shown once per app run: the
// first join after launch gets it, later joins in the same run don't. Module
// state (not localStorage) is exactly that lifetime: a restart shows it again.
let trustWarningAccepted = false;

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
                // Always carry a 2px ring (transparent by default) so the
                // grey hover ring fades both in and out via transition-shadow,
                // and never shows on the already-selected swatch.
                "size-4 rounded-full ring-2 ring-transparent ring-offset-2 ring-offset-card transition-shadow duration-200",
                profile.color === c
                  ? "ring-foreground/70"
                  : "hover:ring-muted-foreground/50",
              )}
              style={{ backgroundColor: c }}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

function SessionCard({
  session,
  probe,
}: {
  session: SessionAd;
  probe?: ExtState["probe"];
}) {
  const [pass, setPass] = useState("");
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [checking, setChecking] = useState(false);
  const [pwError, setPwError] = useState(false);
  // Guards against an earlier check's stale result resolving the new one: we
  // only act once we've seen the fresh "in flight" (ok === null) for our check.
  const sawPending = useRef(false);

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
    else setPwError(true);
  }, [probe, checking]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 8, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.97 }}
      transition={{ type: "spring", stiffness: 350, damping: 28 }}
      className="space-y-2 rounded-xl bg-card p-3"
    >
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
          <Input
            type="password"
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
          {pwError && (
            <p className="px-0.5 text-[12px] text-red-400">{t("wrongPassword")}</p>
          )}
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
      <Input
        type="password"
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
                  <SessionCard key={s.session_id} session={s} probe={state.probe} />
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

      <HostCard project={state.project} error={state.error} />
    </div>
  );
}
