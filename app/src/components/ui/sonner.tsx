import { Toaster as Sonner, type ToasterProps } from "sonner";

// Always dark: the app has a single charcoal theme.
const Toaster = ({ ...props }: ToasterProps) => {
  return (
    <Sonner
      theme="dark"
      // Under the title bar, never over the bottom action button.
      position="top-center"
      offset={{ top: 50 }}
      mobileOffset={{ top: 50 }}
      visibleToasts={2}
      // Two at once sit one under the other, both readable, instead of
      // stacking into each other.
      expand
      toastOptions={{
        // Regular text; only the names inside are bold.
        classNames: { title: "font-normal!" },
        style: {
          background: "var(--popover)",
          color: "var(--popover-foreground)",
          border: "1px solid var(--border)",
          borderRadius: "10px",
          padding: "12px 14px",
          // Flat like the rest of the app: the border sets it apart.
          boxShadow: "none",
        },
      }}
      {...props}
    />
  );
};

export { Toaster };
