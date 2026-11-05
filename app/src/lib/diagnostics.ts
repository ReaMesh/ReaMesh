import { getVersion } from "@tauri-apps/api/app";
import { getConnections } from "./connection";
import { getLangPref } from "./i18n";
import { getReaperStatus } from "./install";
import { localizeLogEntry } from "./log";
import { getOverlaySettings } from "./overlaySettings";
import { getUpdate, getUpdateChannel } from "./updater";
import { getRelays } from "./relays";
import { PROTO } from "./version";

/** Everything a bug report needs, as plain text: no project data, no names.
 *  For Settings → Copy diagnostics. */
export async function buildDiagnostics(): Promise<string> {
  const version = await getVersion().catch(() => "");
  const status = getReaperStatus();
  const { instances } = getConnections();
  return [
    `ReaMesh ${version || "?"} (proto ${PROTO}) · ${navigator.userAgent}`,
    `Language: ${getLangPref()}`,
    `REAPER folder found: ${status?.reaperFound ?? "?"} · looks like REAPER: ${status?.isReaperDir ?? "?"}`,
    `Extension installed: ${status?.installed ?? "?"} · up to date: ${status?.upToDate ?? "?"}`,
    `Update: ${getUpdate().phase} ${getUpdate().version} (${getUpdateChannel().channel}) · overlay: ${JSON.stringify(getOverlaySettings())}`,
    `Instances: ${instances.length}`,
    ...instances.map(
      (i) =>
        `  :${i.port} ext=${i.version ?? "?"}/${i.proto ?? "?"} mode=${i.state?.mode ?? "?"} state=${i.state?.state ?? "?"} people=${i.state?.peers.length ?? 0} busy=${!!i.busy}` +
        // Relay states, reasons and versions only: no addresses, no keys, not
        // the operator's texts.
        (i.relay?.status.length
          ? ` relays=${i.relay.status.map((r) => `${r.state}${r.reason ? `:${r.reason}` : ""}${i.relay?.info[r.peer]?.version ? `@${i.relay.info[r.peer].version}` : ""}`).join(",")}`
          : "") +
        ` links=${(i.state?.peers ?? []).filter((p) => p.link).map((p) => p.link).join(",") || "-"}`,
    ),
    `Relays configured: ${getRelays().length}`,
    "Recent events:",
    ...(instances[0]?.state?.log ?? []).slice(-12).map((e) => `  ${e.code || "-"}: ${localizeLogEntry(e)}`),
  ].join("\n");
}
