import { AnimatePresence, motion } from "motion/react";
import { Crown } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { PeerAvatar } from "@/components/PeerAvatar";
import { send } from "@/lib/connection";
import { displayName, type ExtState, type Peer } from "@/lib/types";

function PeerRow({ peer }: { peer: Peer }) {
  return (
    <motion.div
      layout
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0 }}
      exit={{ opacity: 0, x: 10 }}
      transition={{ type: "spring", stiffness: 350, damping: 28 }}
      className="flex items-center gap-3 rounded-lg px-2 py-1.5"
    >
      <PeerAvatar peer={peer} />
      <span className="min-w-0 flex-1 truncate text-[13px] font-medium">
        {displayName(peer)}
      </span>
      {peer.is_owner && (
        <span title="Session host">
          <Crown className="size-3.5 text-amber-400" />
        </span>
      )}
      {peer.is_self && (
        <Badge variant="secondary" className="px-1.5 py-0 text-[10px]">
          you
        </Badge>
      )}
    </motion.div>
  );
}

export function Session({ state }: { state: ExtState }) {
  const hosting = state.mode === "hosting";

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 p-4">
      <div className="flex items-center gap-3">
        <h2 className="min-w-0 flex-1 truncate text-[15px] font-semibold tracking-tight">
          {state.session || "Session"}
        </h2>
        <Badge variant={hosting ? "default" : "secondary"} className="shrink-0">
          {hosting ? "Hosting" : "Connected"}
        </Badge>
      </div>

      <div className="flex min-h-0 flex-col gap-2">
        <h3 className="px-1 text-[12px] font-medium text-muted-foreground">
          {state.peers.length === 1
            ? "Just you so far"
            : `${state.peers.length} people in this session`}
        </h3>
        <ScrollArea className="min-h-0 rounded-xl bg-card p-2">
          {/* initial={false} suppresses entrance animations for people
              already present when the screen mounts (they fade in with the
              whole screen), while later join/leave still animate. */}
          <AnimatePresence mode="popLayout" initial={false}>
            {state.peers.map((p) => (
              <PeerRow key={p.id} peer={p} />
            ))}
          </AnimatePresence>
        </ScrollArea>
      </div>

      {/* Absorbs slack so the peer card hugs its content and the Leave
          button stays pinned to the bottom. */}
      <div className="min-h-0 flex-1" />

      <Button
        variant="secondary"
        className="shrink-0"
        onClick={() => send({ cmd: "leave" })}
      >
        {hosting ? "Stop hosting" : "Leave session"}
      </Button>
    </div>
  );
}
