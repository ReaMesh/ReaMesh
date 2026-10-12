import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { PROTO, protoSkew } from "./version";

// App, extension and core release together under one version and one PROTO.
// These keep the copies from drifting apart.
const repo = fileURLToPath(new URL("../../../", import.meta.url));
const read = (p: string) => readFileSync(repo + p, "utf8");
const protoIn = (p: string) => Number(/const PROTO: u32 = (\d+);/.exec(read(p))?.[1]);
const cargoVersion = (p: string) => /^version = "([^"]+)"/m.exec(read(p))?.[1];

describe("PROTO", () => {
  it("is the same in the core, the app backend and the webview", () => {
    expect(protoIn("rust/src/lib.rs")).toBe(PROTO);
    expect(protoIn("app/src-tauri/src/lib.rs")).toBe(PROTO);
  });
  it("orders other builds against ours", () => {
    expect(protoSkew(PROTO)).toBe("same");
    expect(protoSkew(PROTO + 1)).toBe("newer");
    expect(protoSkew(undefined)).toBe("older");
  });
});

describe("version", () => {
  it("is one number across every manifest", () => {
    const v = JSON.parse(read("app/package.json")).version;
    expect(JSON.parse(read("app/src-tauri/tauri.conf.json")).version).toBe(v);
    expect(cargoVersion("app/src-tauri/Cargo.toml")).toBe(v);
    expect(cargoVersion("rust/Cargo.toml")).toBe(v);
  });
});
