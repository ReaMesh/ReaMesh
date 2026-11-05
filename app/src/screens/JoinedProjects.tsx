import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, FolderDown, LoaderCircle, Pin, Trash2, Unlink } from "lucide-react";
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
import { Hint } from "@/components/Hint";
import { useConnections } from "@/lib/connection";
import { copyDaysLeft } from "@/lib/copyLink";
import { formatBytes } from "@/lib/format";
import { resolveLang, t, useLang } from "@/lib/i18n";
import { isWindows } from "@/lib/platform";
import {
  copyPercent,
  refreshLinkedCopies,
  saveCopyAs,
  setCopyKept,
  unlinkCopy,
  useLinkedCopies,
  useRunningCopy,
} from "@/lib/savedCopies";
import { cn, focusRing } from "@/lib/utils";

// The downloaded copies of joined sessions (one folder each under REAPER's
// resource dir), with their size, so a big one can go to the Trash before the
// extension's own seven-day sweep gets to it, be kept where it is, or be saved
// somewhere for good. A saved one is listed above them: it is linked to its project
// from then on, and being the user's own folder it can be unlinked but never
// trashed. A kept one is linked just the same and stays in this list, where the
// same button lets it go again. Reached from Settings.

export interface JoinedProject {
  dir: string;
  name: string;
  path: string;
  bytes: number;
  modified: number;
  projects: string[];
  kept?: boolean;
  /** What it is a copy of, if it knows. */
  project?: string | null;
  owner?: string | null;
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
  const running = useRunningCopy();
  const linked = useLinkedCopies();
  useEffect(() => {
    void refreshLinkedCopies();
  }, []);

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

  // A copy in a running session (or a join on its way there) keeps changing:
  // it can be saved once the session is over.
  const inSession = (p: JoinedProject) => {
    const dir = normPath(p.path);
    return instances.some(
      ({ state }) =>
        (state?.mode === "client" || state?.mode === "downloading") &&
        !!state.session_dir &&
        normPath(state.session_dir) === dir,
    );
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

  const toggleKept = (p: JoinedProject) => {
    const keep = !p.kept;
    setCopyKept(p.dir, keep).then(
      () => setList((l) => l?.map((x) => (x.dir === p.dir ? { ...x, kept: keep } : x)) ?? l),
      (e) => toast.error(String(e)),
    );
  };

  const saved = linked?.filter((c) => !c.kept) ?? [];
  // The folder this copy was saved to, which makes this one of no further use.
  const savedTo = (p: JoinedProject) =>
    p.project ? saved.find((c) => c.project === p.project && c.owner === p.owner) : undefined;

  const total = list?.reduce((n, p) => n + p.bytes, 0) ?? 0;
  const date = (ms: number) =>
    ms > 0 ? new Date(ms).toLocaleDateString(resolveLang(), { dateStyle: "medium" }) : "";
  const trashLabel = t(isWindows ? "moveToTrashWin" : "moveToTrash");
  // Only where it applies: the sweep passes over a kept copy and one in use.
  const daysLeft = (p: JoinedProject, used: boolean) => {
    if (p.kept || used) return "";
    const days = copyDaysLeft(p.modified, Date.now());
    if (days === null) return "";
    if (days === 0) return t("daysLeftNone");
    return days === 1 ? t("daysLeftOne") : t("daysLeft", { count: days });
  };

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

        {saved.length > 0 && (
          <section className="space-y-2">
            <SectionLabel>{t("savedCopies")}</SectionLabel>
            <Group>
              {saved.map((copy, i) => (
                <div key={copy.path} className="flex flex-col">
                  {i > 0 && <Divider inset={false} />}
                  <div className="flex min-h-11 items-center gap-2 pr-2">
                    <Hint label={t(isWindows ? "showInExplorer" : "showInFinder")} slow>
                      <button
                        type="button"
                        onClick={() => revealItemInDir(copy.path).catch(() => {})}
                        className={cn(
                          "min-w-0 flex-1 px-3 py-2 text-left transition-colors hover:bg-white/[0.03]",
                          focusRing,
                        )}
                      >
                        <p className="truncate text-[13px]">{copy.name}</p>
                        <p className="truncate text-[12px] text-muted-foreground">{copy.path}</p>
                      </button>
                    </Hint>
                    <Hint label={t("unlink")}>
                      <Button
                        variant="ghost"
                        size="sm"
                        aria-label={`${t("unlink")}: ${copy.name}`}
                        className="size-8 shrink-0 p-0 text-muted-foreground hover:text-foreground"
                        onClick={() => unlinkCopy(copy.path).catch((e) => toast.error(String(e)))}
                      >
                        <Unlink className="size-4" aria-hidden="true" />
                      </Button>
                    </Hint>
                  </div>
                </div>
              ))}
            </Group>
          </section>
        )}

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
                const copying = running?.dir === p.dir ? running : null;
                const savedCopy = savedTo(p);
                const left = daysLeft(p, used);
                return (
                  <div key={p.dir} className="flex flex-col">
                    {i > 0 && <Divider inset={false} />}
                    <div className="flex min-h-11 items-center gap-2 pr-2">
                      <Hint label={t(isWindows ? "showInExplorer" : "showInFinder")} slow>
                        <button
                          type="button"
                          onClick={() => revealItemInDir(p.path).catch(() => {})}
                          className={cn(
                            "min-w-0 flex-1 px-3 py-2 text-left transition-colors hover:bg-white/[0.03]",
                            focusRing,
                          )}
                        >
                          <p className="truncate text-[13px]">{p.name}</p>
                          {/* Size and date side by side, each its own fact. */}
                          <p className="flex gap-3 truncate text-[12px] text-muted-foreground tabular-nums">
                            <span>{formatBytes(p.bytes)}</span>
                            {p.kept ? (
                              <span className="truncate">{t("keptCopy")}</span>
                            ) : savedCopy ? (
                              <Hint label={savedCopy.path}>
                                <span className="truncate">{t("savedElsewhere")}</span>
                              </Hint>
                            ) : (
                              date(p.modified) && <span className="truncate">{date(p.modified)}</span>
                            )}
                            {left && <span className="shrink-0">{left}</span>}
                          </p>
                        </button>
                      </Hint>
                      <Hint label={t(p.kept ? "stopKeeping" : "keepCopy")}>
                        <Button
                          variant="ghost"
                          size="sm"
                          aria-label={`${t("keepCopy")}: ${p.name}`}
                          aria-pressed={!!p.kept}
                          className={cn(
                            "size-8 shrink-0 p-0 hover:text-foreground",
                            p.kept ? "text-foreground" : "text-muted-foreground",
                          )}
                          onClick={() => toggleKept(p)}
                        >
                          <Pin className={cn("size-4", p.kept && "fill-current")} aria-hidden="true" />
                        </Button>
                      </Hint>
                      {copying ? (
                        <span
                          role="status"
                          className="flex shrink-0 items-center gap-1.5 px-1.5 text-[12px] text-muted-foreground tabular-nums"
                        >
                          <LoaderCircle className="size-3.5 motion-safe:animate-spin" aria-hidden="true" />
                          {copyPercent(copying)}%
                        </span>
                      ) : (
                        !inSession(p) && (
                          <Hint label={t("saveCopyAs")}>
                            <Button
                              variant="ghost"
                              size="sm"
                              aria-label={`${t("saveCopyAs")}: ${p.name}`}
                              disabled={!!running}
                              className="size-8 shrink-0 p-0 text-muted-foreground hover:text-foreground"
                              onClick={() => void saveCopyAs(p.dir)}
                            >
                              <FolderDown className="size-4" aria-hidden="true" />
                            </Button>
                          </Hint>
                        )
                      )}
                      {used ? (
                        <span className="shrink-0 px-1.5 text-[12px] text-muted-foreground">
                          {t("joinedInUse")}
                        </span>
                      ) : (
                        <Hint label={trashLabel}>
                          <Button
                            variant="ghost"
                            size="sm"
                            aria-label={`${trashLabel}: ${p.name}`}
                            disabled={!!copying}
                            className="size-8 shrink-0 p-0 text-muted-foreground hover:text-destructive"
                            onClick={() => setTarget(p)}
                          >
                            <Trash2 className="size-4" aria-hidden="true" />
                          </Button>
                        </Hint>
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
              {target?.kept && ` ${t("trashKeptNote")}`}
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
