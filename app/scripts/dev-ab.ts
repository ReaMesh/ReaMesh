// Launch two companion-app dev instances side by side (`bun run dev:ab`):
// A: plain `tauri dev` (Vite 1420, identifier app.reamesh.companion)
// B: `tauri dev --config src-tauri/tauri.dev-b.conf.json` (Vite 1421,
//    separate identifier and localStorage/config for a second profile)
// Both scan all control ports. Run two REAPER instances to test two peers
// on one machine (`open -na REAPER --args -newinst` on macOS).
// Output is prefixed [A]/[B]; Ctrl+C stops both.

import { resolve } from "node:path";

const app = resolve(import.meta.dirname, "..");
const bun = process.execPath;

function launch(tag: string, args: string[]) {
  const proc = Bun.spawn([bun, "run", ...args], {
    cwd: app,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
    env: process.env,
  });
  const forward = async (stream: ReadableStream<Uint8Array>, out: NodeJS.WriteStream) => {
    let carry = "";
    const dec = new TextDecoder();
    for await (const chunk of stream) {
      const lines = (carry + dec.decode(chunk)).split(/\r?\n/);
      carry = lines.pop() ?? "";
      for (const line of lines) out.write(`[${tag}] ${line}\n`);
    }
    if (carry) out.write(`[${tag}] ${carry}\n`);
  };
  void forward(proc.stdout, process.stdout);
  void forward(proc.stderr, process.stderr);
  return proc;
}

const procs = [launch("A", ["tauri", "dev"]), launch("B", ["dev:b"])];

const stop = () => {
  for (const p of procs) p.kill("SIGINT");
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);

const codes = await Promise.all(procs.map((p) => p.exited));
process.exit(codes.find((c) => c !== 0) ?? 0);
