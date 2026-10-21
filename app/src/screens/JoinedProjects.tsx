import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Trash2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Divider, Group, SectionLabel } from "@/components/Group";
import { useConnections } from "@/lib/connection";
import { formatBytes } from "@/lib/format";
import { resolveLang, t, useLang } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import { cn, focusRing } from "@/lib/utils";

// The downloaded copies of joined sessions (one folder each under REAPER's
// resource dir), with their size, so a big one can go to the Trash before the
// extension's own three-day sweep gets to it. Reached from Settings.

export interface JoinedProject {
  dir: string;
  name: string;
  path: string;
  bytes: number;
  modified: number;
  projects: string[];
}

export const listJoinedProjects = () => invoke<JoinedProject[]>("joined_projects");

/** The extension's folder name for a session (`sanitize_dir_name` in the core). */
function sessionDirName(name: string): string {
  const cleaned = [...name].map((c) => (/[\p{L}\p{N}\-_ .]/u.test(c) ? c : "_")).join("");
  return cleaned.trim().replace(/^\.+/, "") || "session";
}

/** Comparable form of a path: one separator, no trailing one, and case-folded
 *  (the default macOS and Windows file systems ignore case). */
function normPath(p: string): string {
  return p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

export function JoinedProjects({ onBack }: { onBack: () => void }) {
  useLang();
  const [list, setList] = useState<JoinedProject[] | null>(null);
  const [target, setTarget] = useState<JoinedProject | null>(null);
  const [busy, setBusy] = useState(false);
  const { instances } = useConnections();

  const load = useCallback(() => {
    listJoinedProjects().then(setList, (e) => {
      setList([]);
      toast.error(String(e));
    });
  }, []);
  useEffect(load, [load]);

  // A copy one of the running REAPERs has open (or is downloading into)
  // stays: any path it reports that lies inside the copy's folder.
  const inUse = (p: JoinedProject) => {
    const dir = normPath(p.path);
    return instances.some(({ state }) => {
      if (!state) return false;
      if (state.in_use) {
        return state.in_use.some((u) => {
          const n = normPath(u);
          return n === dir || n.startsWith(dir + "/");
        });
      }
      // An older extension reports only names: match on those.
      if (state.project && p.projects.includes(state.project)) return true;
      return (
        (state.mode === "downloading" || state.mode === "client") &&
        sessionDirName(state.session) === p.name
      );
    });
  };

  const trash = async () => {
    if (!target) return;
    setBusy(true);
    try {
      await invoke("trash_joined_project", { dir: target.dir });
      toast.success(t(isWindows ? "trashedToastWin" : "trashedToast"));
      setList((l) => l?.filter((p) => p.dir !== target.dir) ?? l);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
      setTarget(null);
      load();
    }
  };

  const total = list?.reduce((n, p) => n + p.bytes, 0) ?? 0;
  const date = (ms: number) =>
    ms > 0 ? new Date(ms).toLocaleDateString(resolveLang(), { dateStyle: "medium" }) : "";
  const trashLabel = t(isWindows ? "moveToTrashWin" : "moveToTrash");

  return (
    <div className={cn("flex flex-1 flex-col overflow-y-auto", isWindows && "scroll-slim")}>
      <div className="w-full space-y-[18px] px-4 pt-1 pb-5">
        <div className="relative flex h-8 items-center">
          <Button
            variant="ghost"
            size="sm"
            className="-ml-1 h-8 gap-1.5 px-2 text-[13px] text-foreground/65 hover:text-foreground"
            onClick={onBack}
          >
            <ArrowLeft className="size-4" aria-hidden="true" />
            {t("back")}
          </Button>
          <h1 className="pointer-events-none absolute inset-0 grid place-items-center text-[15px] font-semibold">
            {t("joinedProjects")}
          </h1>
        </div>

        <section className="space-y-2">
          <SectionLabel aside={list && list.length > 0 ? formatBytes(total) : undefined}>
            {t("storage")}
          </SectionLabel>
          <Group>
            {list === null ? (
              <p className="px-3 py-3 text-[13px] text-muted-foreground">{t("joinedLoading")}</p>
            ) : list.length === 0 ? (
              <p className="px-3 py-3 text-[13px] text-muted-foreground">{t("joinedEmpty")}</p>
            ) : (
              list.map((p, i) => {
                const used = inUse(p);
                return (
                  <div key={p.dir} className="flex flex-col">
                    {i > 0 && <Divider inset={false} />}
                    <div className="flex min-h-11 items-center gap-2 pr-2">
                      <button
                        type="button"
                        title={t(isWindows ? "showInExplorer" : "showInFinder")}
                        onClick={() => revealItemInDir(p.path).catch(() => {})}
                        className={cn(
                          "min-w-0 flex-1 px-3 py-2 text-left transition-colors hover:bg-white/[0.03]",
                          focusRing,
                        )}
                      >
                        <p className="truncate text-[13px]">{p.name}</p>
                        <p className="truncate text-[12px] text-muted-foreground tabular-nums">
                          {[formatBytes(p.bytes), date(p.modified)].filter(Boolean).join(" · ")}
                        </p>
                      </button>
                      {used ? (
                        <span className="shrink-0 px-1.5 text-[12px] text-muted-foreground">
                          {t("joinedInUse")}
                        </span>
                      ) : (
                        <Button
                          variant="ghost"
                          size="sm"
                          title={trashLabel}
                          aria-label={`${trashLabel}: ${p.name}`}
                          className="size-8 shrink-0 p-0 text-muted-foreground hover:text-destructive"
                          onClick={() => setTarget(p)}
                        >
                          <Trash2 className="size-4" aria-hidden="true" />
                        </Button>
                      )}
                    </div>
                  </div>
                );
              })
            )}
          </Group>
          <p className="px-1 text-[11px] leading-snug text-muted-foreground">
            {t("joinedAutoClean")}
          </p>
        </section>
      </div>

      <Dialog open={target !== null} onOpenChange={(o) => !o && !busy && setTarget(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t(isWindows ? "trashTitleWin" : "trashTitle", { name: target?.name ?? "" })}
            </DialogTitle>
            <DialogDescription>
              {t(isWindows ? "trashBodyWin" : "trashBody", {
                size: formatBytes(target?.bytes ?? 0),
              })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="secondary" disabled={busy} onClick={() => setTarget(null)}>
              {t("cancel")}
            </Button>
            <Button variant="destructive" disabled={busy} onClick={trash}>
              {trashLabel}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
