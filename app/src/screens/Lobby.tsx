import { useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { LockKeyhole, ShieldAlert } from "lucide-react";
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
import { getProfile, saveProfile, PALETTE, type Profile } from "@/lib/profile";
import { initials, type ExtState, type SessionAd } from "@/lib/types";
import { cn } from "@/lib/utils";

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
          placeholder="Your name"
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

function SessionCard({ session }: { session: SessionAd }) {
  const [pass, setPass] = useState("");
  const [confirmOpen, setConfirmOpen] = useState(false);
  const join = () => {
    setConfirmOpen(false);
    send({
      cmd: "join",
      session_id: session.session_id,
      session_name: session.session_name,
      passphrase: pass,
    });
  };

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
          </p>
        </div>
        <Button
          size="sm"
          className="h-7 px-3 text-[12px]"
          disabled={session.auth && !pass}
          onClick={() => setConfirmOpen(true)}
        >
          Join
        </Button>
      </div>
      {session.auth && (
        <Input
          type="password"
          value={pass}
          placeholder="Password"
          className="h-8 border-0 bg-secondary text-[13px]"
          onChange={(e) => setPass(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && pass && setConfirmOpen(true)}
        />
      )}

      {/* Joining downloads and opens the host's project, and the host can load
          track effects/plugins live during the session; both run code on this
          machine. Make that trust decision explicit before connecting. */}
      <Dialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <DialogContent className="max-w-[340px] gap-4 rounded-2xl">
          <DialogHeader>
            <div className="mx-auto flex size-11 items-center justify-center rounded-full bg-amber-500/15 sm:mx-0">
              <ShieldAlert className="size-5 text-amber-500" />
            </div>
            <DialogTitle className="text-[15px]">
              Only join people you trust
            </DialogTitle>
            <DialogDescription className="text-[13px] leading-relaxed">
              Joining{" "}
              <span className="font-medium text-foreground">
                {session.session_name}
              </span>{" "}
              downloads and opens the host's project. Its tracks can load audio
              effects and plugins, and the host can add more while you're
              connected. These run as code on your computer. Only continue if
              you trust the person hosting.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="gap-2 sm:gap-2">
            <Button
              variant="outline"
              size="sm"
              className="h-8 flex-1 text-[12px]"
              onClick={() => setConfirmOpen(false)}
            >
              Cancel
            </Button>
            <Button
              size="sm"
              className="h-8 flex-1 text-[12px]"
              onClick={join}
            >
              Join anyway
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </motion.div>
  );
}

function HostCard({ project }: { project: string }) {
  const [pass, setPass] = useState("");
  return (
    <div className="space-y-2 rounded-xl bg-card p-3">
      <div className="flex items-center gap-3">
        <div className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-semibold">
            {project || "Untitled project"}
          </span>
          <p className="text-[12px] text-muted-foreground">
            Share this project with your network
          </p>
        </div>
        <Button
          size="sm"
          className="h-7 px-4 text-[12px]"
          onClick={() => send({ cmd: "host", passphrase: pass })}
        >
          Host
        </Button>
      </div>
      <Input
        type="password"
        value={pass}
        placeholder="Password (optional)"
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
      <ProfileRow profile={profile} onChange={updateProfile} />

      <div className="flex min-h-0 flex-1 flex-col gap-2">
        <h3 className="px-1 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Sessions on this network
        </h3>
        <ScrollArea className="min-h-0 flex-1 -mx-1 px-1">
          <div className="space-y-2">
            <AnimatePresence mode="popLayout">
              {state.sessions.map((s) => (
                <SessionCard key={s.session_id} session={s} />
              ))}
            </AnimatePresence>
            {state.sessions.length === 0 && (
              <div className="flex items-center justify-center rounded-xl border border-dashed border-border p-5 text-muted-foreground">
                <span className="text-[13px]">No sessions yet</span>
              </div>
            )}
          </div>
        </ScrollArea>
      </div>

      <HostCard project={state.project} />
    </div>
  );
}
