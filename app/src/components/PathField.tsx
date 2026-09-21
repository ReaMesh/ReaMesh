import { Folder } from "lucide-react";

/** A long path in one line: the middle folds away, both ends stay readable. */
export function PathField({ path, action }: { path: string; action?: React.ReactNode }) {
  const cut = Math.max(0, path.length - 22);
  return (
    <div className="flex h-8 min-w-0 items-center gap-2 rounded-lg bg-muted pl-2.5 pr-1" title={path}>
      <Folder className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
      <span className="flex min-w-0 flex-1 text-[12px] text-foreground/80">
        <span className="truncate">{path.slice(0, cut)}</span>
        <span className="shrink-0 whitespace-pre">{path.slice(cut)}</span>
      </span>
      {action}
    </div>
  );
}
