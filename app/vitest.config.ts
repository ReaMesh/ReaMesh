import path from "node:path";
import { defineConfig } from "vitest/config";

// Unit tests for the app's pure logic (lib/): ticket parsing, plugin
// matching, state normalization, formatting, log localization. Deliberately
// node-environment with tiny shims (src/test/setup.ts) instead of a DOM
// package: nothing under test renders.
export default defineConfig({
  resolve: {
    alias: { "@": path.resolve(__dirname, "src") },
  },
  test: {
    environment: "node",
    setupFiles: ["src/test/setup.ts"],
    include: ["src/**/*.test.ts"],
  },
});
