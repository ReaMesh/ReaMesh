import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// The Windows installer's window (../setup): its own page, built into
// dist-setup/ and embedded into the Setup binary. Same aliases and styles as
// the app, so it is drawn with the app's own tokens and components.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(import.meta.dirname, "./src"),
      "@proprietary": path.resolve(import.meta.dirname, "../proprietary"),
    },
  },
  clearScreen: false,
  build: {
    outDir: "dist-setup",
    emptyOutDir: true,
    rollupOptions: { input: path.resolve(import.meta.dirname, "setup.html") },
  },
});
