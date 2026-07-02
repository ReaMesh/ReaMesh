import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { Eye, EyeOff } from "lucide-react";
import { Input } from "@/components/ui/input";
import { t } from "@/lib/i18n";
import { cn } from "@/lib/utils";

// Password field with a local visibility toggle. Hide the toggle while empty.
export function PasswordInput({
  className,
  ...props
}: React.ComponentProps<"input">) {
  const [visible, setVisible] = useState(false);
  const hasText = String(props.value ?? "").length > 0;

  // Once the field empties, re-mask so the next password starts hidden (and the
  // reappearing button reads as "reveal", not "hide").
  useEffect(() => {
    if (!hasText && visible) setVisible(false);
  }, [hasText, visible]);

  return (
    <div className="relative">
      <Input
        {...props}
        type={visible ? "text" : "password"}
        className={cn("pr-9", className)}
      />
      <AnimatePresence>
        {hasText && (
          <motion.button
            type="button"
            // This control only toggles masking, so keep it out of tab order.
            tabIndex={-1}
            aria-label={visible ? t("hidePassword") : t("showPassword")}
            title={visible ? t("hidePassword") : t("showPassword")}
            onClick={() => setVisible((v) => !v)}
            initial={{ opacity: 0, scale: 0.7 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.7 }}
            transition={{ duration: 0.15, ease: "easeOut" }}
            className="absolute inset-y-0 right-0 flex w-8 items-center justify-center rounded-r-md text-muted-foreground transition-colors hover:text-foreground"
          >
            {visible ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}
