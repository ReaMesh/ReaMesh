import { getVersion } from "@tauri-apps/api/app";
import { getAutoInstall } from "./autoInstall";
import { getConnections } from "./connection";
import { getLangPref } from "./i18n";
import { getReaperStatus } from "./install";
import { localizeLogEntry } from "./log";
import { getOverlaySettings } from "./overlaySettings";

/** Everything a bug report needs, as plain text: no project data, no names.
 *  Shared by the in-app Settings screen and the native macOS settings window
 *  (which asks for it over `settings:copy-diagnostics`). */
export async function buildDiagnostics(): Promise<string> {
  const version = await getVersion().catch(() => "");
  const status = getReaperStatus();
  const { instances } = getConnections();
  return [
    `ReaMesh ${version || "?"} · ${navigator.userAgent}`,
    `Language: ${getLangPref()}`,
    `REAPER folder found: ${status?.reaperFound ?? "?"} · looks like REAPER: ${status?.isReaperDir ?? "?"}`,
    `Extension installed: ${status?.installed ?? "?"} · up to date: ${status?.upToDate ?? "?"}`,
    `Auto-install: ${getAutoInstall() ?? true} · overlay: ${JSON.stringify(getOverlaySettings())}`,
    `Instances: ${instances.length}`,
    ...instances.map(
      (i) =>
        `  :${i.port} mode=${i.state?.mode ?? "?"} state=${i.state?.state ?? "?"} people=${i.state?.peers.length ?? 0} busy=${!!i.busy}`,
    ),
    "Recent events:",
    ...(instances[0]?.state?.log ?? []).slice(-12).map((e) => `  ${e.code || "-"}: ${localizeLogEntry(e)}`),
  ].join("\n");
}
