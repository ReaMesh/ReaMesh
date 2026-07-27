// Catch render failures so a frameless Tauri window does not become blank.
// Offer a reload and copyable diagnostics for bug reports.

import React from "react";
import { RotateCw, Copy, Check } from "lucide-react";
import { Button } from "@/components/ui/button";
import { t } from "@/lib/i18n";

interface Props {
  children: React.ReactNode;
  /** Extra context for the copied report (e.g. "overlay window"). */
  where?: string;
}

interface State {
  error: Error | null;
  info: string;
  copied: boolean;
}

export class ErrorBoundary extends React.Component<Props, State> {
  state: State = { error: null, info: "", copied: false };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  override componentDidCatch(error: Error, info: React.ErrorInfo) {
    this.setState({ info: info.componentStack ?? "" });
    console.error("[reamesh] render error", error, info);
  }

  private report(): string {
    const { error, info } = this.state;
    return [
      `ReaMesh ${this.props.where ?? "app"} crashed`,
      `when: ${new Date().toISOString()}`,
      `agent: ${navigator.userAgent}`,
      `url: ${window.location.href}`,
      "",
      String(error?.stack || error),
      info && `\ncomponent stack:${info}`,
    ]
      .filter(Boolean)
      .join("\n");
  }

  private async copy() {
    try {
      await navigator.clipboard.writeText(this.report());
      this.setState({ copied: true });
      setTimeout(() => this.setState({ copied: false }), 1600);
    } catch {
      // The diagnostics remain selectable if clipboard access fails.
    }
  }

  override render() {
    if (!this.state.error) return this.props.children;
    return (
      <div
        role="alert"
        className="flex h-full flex-col justify-center gap-4 overflow-y-auto p-6"
      >
        <div>
          <h1 className="text-[15px] font-semibold">{t("crashTitle")}</h1>
          <p className="mt-1 text-[12px] text-muted-foreground">{t("crashBody")}</p>
        </div>
        <pre className="max-h-40 shrink-0 overflow-auto rounded-lg bg-card p-3 text-[11px] leading-relaxed text-muted-foreground select-text">
          {String(this.state.error?.message || this.state.error)}
        </pre>
        <div className="flex gap-2">
          <Button size="sm" onClick={() => window.location.reload()}>
            <RotateCw className="size-3.5" aria-hidden="true" />
            {t("crashReload")}
          </Button>
          <Button size="sm" variant="secondary" onClick={() => this.copy()}>
            {this.state.copied ? (
              <Check className="size-3.5" aria-hidden="true" />
            ) : (
              <Copy className="size-3.5" aria-hidden="true" />
            )}
            {this.state.copied ? t("crashCopied") : t("crashCopy")}
          </Button>
        </div>
      </div>
    );
  }
}
