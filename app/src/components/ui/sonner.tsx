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
      toastOptions={{
        style: {
          background: "var(--popover)",
          color: "var(--popover-foreground)",
          border: "1px solid var(--border)",
        },
      }}
      {...props}
    />
  );
};

export { Toaster };
