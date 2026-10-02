import { useEffect, useRef, useState } from "react";
import { ArrowLeft, Check, Copy, ExternalLink } from "lucide-react";
import { motion } from "motion/react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Divider, Group, SectionLabel } from "@/components/Group";
import { PathField } from "@/components/PathField";
import { ProfileEditor } from "@/components/ProfileEditor";
import { buildDiagnostics } from "@/lib/diagnostics";
import { getLangPref, setLangPref, t, useLang, type LangPref } from "@/lib/i18n";
import {
  getReaperPath,
  installExtension,
  refreshStatus,
  removeExtension,
  setReaperPath,
  useReaperStatus,
} from "@/lib/install";
import { setAutoInstall, useAutoInstall } from "@/lib/autoInstall";
import {
  type OverlaySettings,
  setOverlaySetting,
  useOverlaySettings,
} from "@/lib/overlaySettings";
import { isWindows } from "@/lib/platform";
import { cn, focusRing } from "@/lib/utils";

// `hud` rows only mean something while the HUD is shown and grey out without it.
// The tint is drawn by REAPER itself, so it stays independent.
const OVERLAY_TOGGLES: { key: keyof OverlaySettings; label: () => string; hud?: true }[] = [
  { key: "hud", label: () => t("olShow") },
  { key: "roster", label: () => t("olRoster"), hud: true },
  { key: "rememberPos", label: () => t("olRememberPos"), hud: true },
  { key: "tint", label: () => t("olItemTint") },
];

/** A settings row: label (and optional status line) left, one control right. */
function Row({
  label,
  sub,
  children,
}: {
  label: React.ReactNode;
  sub?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <div className="flex min-h-11 items-center gap-3 px-3 py-2">
      <div className="min-w-0 flex-1">
        <p className="truncate text-[13px]">{label}</p>
        {sub && <p className="truncate text-[12px] text-muted-foreground">{sub}</p>}
      </div>
      {children}
    </div>
  );
}

function Switch({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-[22px] w-[38px] shrink-0 rounded-full transition-colors disabled:pointer-events-none",
        checked ? "bg-found" : "bg-white/15",
        focusRing,
      )}
    >
      <span
        className={cn(
          "absolute top-0.5 left-0.5 size-[18px] rounded-full bg-white transition-transform",
          checked && "translate-x-4",
        )}
      />
    </button>
  );
}

// Full-screen app settings, reached from the Windows title-bar gear or Ctrl+,
// (macOS has the native AppKit settings window instead, from Cmd+,). Grouped rows with one
// control each; your name and colour on top (the Lobby edits them too).
export function Settings({ onBack }: { onBack: () => void }) {
  const [busy, setBusy] = useState(false);
  const status = useReaperStatus();
  const hasCustom = !!getReaperPath();
  const overlay = useOverlaySettings();
  const autoInstall = useAutoInstall() ?? true;
  useLang(); // re-render when the language changes

  const toggleAuto = async (next: boolean) => {
    await setAutoInstall(next);
    // Enabling may have just installed/updated; reflect the new status.
    await refreshStatus();
  };

  const pickFolder = async () => {
    const dir = await open({
      directory: true,
      title: t("locateReaperFolder"),
    });
    if (typeof dir === "string") {
      try {
        await setReaperPath(dir);
      } catch (e) {
        toast.error(String(e));
        return;
      }
      await refreshStatus();
    }
  };

  const useDefault = async () => {
    try {
      await setReaperPath(null);
    } catch (e) {
      toast.error(String(e));
      return;
    }
    await refreshStatus();
  };

  const install = async () => {
    setBusy(true);
    try {
      await installExtension();
      await refreshStatus(); // installExtension no longer publishes; refresh the badge
      toast.success(t("extInstalledToast"));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      await removeExtension();
      await refreshStatus();
      toast.success(t("extRemovedToast"));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const extStatus = !status
    ? ""
    : !status.installed
      ? t("notInstalled")
      : status.upToDate
        ? t("installed")
        : t("updateAvailable");

  const langOptions: { value: LangPref; label: string }[] = [
    { value: "system", label: t("languageSystem") },
    { value: "en", label: "English" },
    { value: "de", label: "Deutsch" },
  ];

  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);
  // Repeated clicks only extend the checkmark's stay, so spamming the row can't
  // make it flicker; a click while a copy is still being built is ignored.
  const [copied, setCopied] = useState(false);
  const copying = useRef(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(copiedTimer.current), []);
  const copyDiagnostics = async () => {
    if (copying.current) return;
    copying.current = true;
    try {
      await navigator.clipboard.writeText(await buildDiagnostics());
      setCopied(true);
      clearTimeout(copiedTimer.current);
      copiedTimer.current = setTimeout(() => setCopied(false), 1600);
    } catch (e) {
      toast.error(String(e));
    } finally {
      copying.current = false;
    }
  };
  const ghost = "h-7 shrink-0 px-2.5 text-[12px]";

  return (
    // Style WebView2's scrollbar on Windows; keep the native macOS overlay.
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
            {t("settings")}
          </h1>
        </div>

        <section className="space-y-2">
          <SectionLabel>{t("profile")}</SectionLabel>
          <Group className="p-3">
            <ProfileEditor />
          </Group>
        </section>

        <section className="space-y-2">
          <SectionLabel>{t("language")}</SectionLabel>
          <div role="radiogroup" aria-label={t("language")} className="flex gap-1 rounded-xl bg-card p-1">
            {langOptions.map((o) => (
              <button
                key={o.value}
                type="button"
                role="radio"
                aria-checked={getLangPref() === o.value}
                onClick={() => setLangPref(o.value)}
                className={cn(
                  "h-7 flex-1 rounded-lg text-[12px] font-medium transition-colors",
                  getLangPref() === o.value
                    ? "bg-foreground/[0.14] text-foreground"
                    : "text-muted-foreground hover:text-foreground",
                  focusRing,
                )}
              >
                {o.label}
              </button>
            ))}
          </div>
        </section>

        <section className="space-y-2">
          <SectionLabel>REAPER</SectionLabel>
          <Group>
            <div className="space-y-2 p-3">
              <PathField
                path={status?.resourcePath ?? t("notFoundBadge")}
                action={
                  <Button variant="ghost" size="sm" className={ghost} onClick={pickFolder}>
                    {t("change")}
                  </Button>
                }
              />
              {hasCustom && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 w-full text-[12px] text-muted-foreground hover:text-foreground"
                  onClick={useDefault}
                >
                  {t("useDefault")}
                </Button>
              )}
              {/* Explain when the selected folder does not look like REAPER. */}
              {status && status.reaperFound && !status.isReaperDir && (
                <p role="status" className="px-0.5 text-[12px] leading-normal text-amber-500">
                  {t("notReaperDirWarning")}
                </p>
              )}
              {status && !status.reaperFound && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 w-full gap-1.5 text-[12px] text-muted-foreground hover:text-foreground"
                  onClick={() => openUrl("https://www.reaper.fm/download.php")}
                >
                  {t("dontHaveReaper")}
                  <ExternalLink className="size-3" aria-hidden="true" />
                </Button>
              )}
            </div>
            <Divider inset={false} />
            <Row
              label={t("reaperExtension")}
              sub={
                // Neutral when all is well; only an available update is worth a colour.
                <span className={cn(status?.installed && !status.upToDate && "text-amber-500")}>
                  {extStatus}
                </span>
              }
            >
              <Button
                size="sm"
                variant={status?.installed && status.upToDate ? "secondary" : "default"}
                className={ghost}
                disabled={busy || !status?.reaperFound}
                onClick={install}
              >
                {busy
                  ? t("installing")
                  : !status?.installed
                    ? t("installShort")
                    : status.upToDate
                      ? t("reinstallShort")
                      : t("updateShort")}
              </Button>
              {/* Offer explicit removal because uninstalling the app leaves the
                  extension in REAPER's UserPlugins folder. */}
              {status?.installed && (
                <Button
                  variant="ghost"
                  size="sm"
                  className={cn(ghost, "text-muted-foreground hover:text-destructive")}
                  disabled={busy}
                  onClick={remove}
                >
                  {t("removeShort")}
                </Button>
              )}
            </Row>
            <Divider inset={false} />
            <Row label={t("autoInstallExt")}>
              <Switch checked={autoInstall} onChange={toggleAuto} label={t("autoInstallExt")} />
            </Row>
          </Group>
        </section>

        <section className="space-y-2">
          <SectionLabel>{t("overlaySettingsTitle")}</SectionLabel>
          <Group>
            {OVERLAY_TOGGLES.map((row, i) => {
              const off = row.hud && !overlay.hud;
              return (
                <div key={row.key}>
                  {i > 0 && <Divider inset={false} />}
                  <div className={cn("transition-opacity", off && "opacity-40")}>
                    <Row label={row.label()}>
                      <Switch
                        checked={overlay[row.key]}
                        onChange={(v) => setOverlaySetting(row.key, v)}
                        label={row.label()}
                        disabled={off}
                      />
                    </Row>
                  </div>
                </div>
              );
            })}
          </Group>
        </section>

        <section className="space-y-2">
          <SectionLabel>{t("privacy")}</SectionLabel>
          <Group>
            <Row label={t("networkIdentity")}>
              <Button
                variant="ghost"
                size="sm"
                title={t("resetIdentityHint")}
                className={cn(ghost, "text-muted-foreground hover:text-destructive")}
                onClick={async () => {
                  try {
                    const had = await invoke<boolean>("reset_network_identity");
                    toast.success(had ? t("identityWasReset") : t("noIdentityYet"));
                  } catch (e) {
                    toast.error(String(e));
                  }
                }}
              >
                {t("resetShort")}
              </Button>
            </Row>
          </Group>
        </section>

        <section className="space-y-2">
          <SectionLabel>{t("help")}</SectionLabel>
          <Group>
            <button
              type="button"
              onClick={() => openUrl("https://reamesh.com/docs")}
              className={cn("text-left transition-colors hover:bg-white/[0.03]", focusRing)}
            >
              <Row label={t("documentation")}>
                <ExternalLink className="size-3.5 text-muted-foreground" aria-hidden="true" />
              </Row>
            </button>
            <Divider inset={false} />
            <button
              type="button"
              onClick={copyDiagnostics}
              className={cn("text-left transition-colors hover:bg-white/[0.03]", focusRing)}
            >
              <Row label={t("copyDiagnostics")}>
                <span className="relative size-3.5 shrink-0" aria-hidden="true">
                  <motion.span
                    className="absolute inset-0"
                    initial={false}
                    animate={copied ? { opacity: 0, scale: 0.5 } : { opacity: 1, scale: 1 }}
                    transition={{ type: "spring", duration: 0.35, bounce: 0 }}
                  >
                    <Copy className="size-3.5 text-muted-foreground" />
                  </motion.span>
                  <motion.span
                    className="absolute inset-0"
                    initial={false}
                    animate={copied ? { opacity: 1, scale: 1 } : { opacity: 0, scale: 0.5 }}
                    transition={{ type: "spring", duration: 0.35, bounce: copied ? 0.4 : 0 }}
                  >
                    <Check className="size-3.5 text-found" strokeWidth={2.5} />
                  </motion.span>
                </span>
                <span className="sr-only" aria-live="polite">
                  {copied ? t("diagnosticsCopied") : ""}
                </span>
              </Row>
            </button>
          </Group>
        </section>

        {version && (
          <p className="text-center text-[11.5px] text-muted-foreground">ReaMesh {version}</p>
        )}
      </div>
    </div>
  );
}
