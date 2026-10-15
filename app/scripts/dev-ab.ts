// Launch two app dev instances side by side (`bun run dev:ab`):
// A: plain `tauri dev` (Vite 1420, identifier com.reamesh.ReaMesh)
// B: `tauri dev --config src-tauri/tauri.dev-b.conf.json` (Vite 1421,
//    separate identifier and localStorage/config for a second profile)
// Both scan all control ports. Run two REAPER instances to test two peers
// on one machine (`open -na REAPER --args -newinst` on macOS).
// Output is prefixed [A]/[B]; Ctrl+C stops both.

import { resolve } from "node:path";

const app = resolve(import.meta.dirname, "..");
const bun = process.execPath;

// Check both ports before launching so a busy port cannot leave only one
// instance running. A previous dev:ab process may still be using them.
const PORTS = { A: 1420, B: 1421 } as const;
// Probe by connecting: Bun binds with SO_REUSEPORT, so binding is not a
// reliable availability check. Try both localhost addresses, ::1 and 127.0.0.1.
async function portBusy(port: number): Promise<boolean> {
  const tryHost = (hostname: string) =>
    new Promise<boolean>((done) => {
      const t = setTimeout(() => done(false), 500);
      Bun.connect({
        hostname,
        port,
        socket: {
          open(sock) { clearTimeout(t); sock.end(); done(true); },
          error() { clearTimeout(t); done(false); },
          connectError() { clearTimeout(t); done(false); },
          data() {},
        },
      }).catch(() => { clearTimeout(t); done(false); });
    });
  return (await tryHost("127.0.0.1")) || (await tryHost("::1"));
}
for (const [tag, port] of Object.entries(PORTS)) {
  if (await portBusy(port)) {
    console.error(
      `[dev-ab] port ${port} (instance ${tag}) is already in use. Is another \`bun run dev:ab\` ` +
        `or \`tauri dev\` still running? Stop it first (lsof -nP -iTCP:${port} -sTCP:LISTEN).`,
    );
    process.exit(1);
  }
}

function launch(tag: string, args: string[], env: Record<string, string | undefined> = process.env) {
  const proc = Bun.spawn([bun, "run", ...args], {
    cwd: app,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
    env,
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

// Both instances normally share target/debug/app. Windows cannot replace
// a running executable, so B needs a separate target directory there.
// Keeping it under target/ excludes it from Git and Tauri's file watcher.
const envB: Record<string, string | undefined> =
  process.platform === "win32"
    ? { ...process.env, CARGO_TARGET_DIR: resolve(app, "src-tauri", "target", "dev-b") }
    : process.env;

const procs = [launch("A", ["tauri", "dev"]), launch("B", ["dev:b"], envB)];

const stop = () => {
  for (const p of procs) p.kill("SIGINT");
  // A terminal Ctrl+C signals the whole foreground group, but a signal sent
  // to this script alone (a task runner, an IDE) reaches only the `bun run`
  // wrappers; the app windows tauri spawned two levels down would live on
  // with nothing serving their dev URL. Sweep this project's debug binary.
  if (process.platform !== "win32") {
    // cargo runs it as the relative `target/debug/app`, hence the loose match.
    Bun.spawnSync(["pkill", "-f", "(^|/)target/debug/app$"], { stdout: "ignore", stderr: "ignore" });
  } else {
    // Match the executable path; other projects may also use the name app.exe.
    const root = resolve(app, "src-tauri", "target").replaceAll("'", "''");
    Bun.spawnSync(
      ["powershell", "-NoProfile", "-Command",
       `Get-Process app -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '${root}\\*' } | Stop-Process -Force`],
      { stdout: "ignore", stderr: "ignore" },
    );
  }
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);

// Stop both instances if either exits or fails to build.
const first = await Promise.race(procs.map((p) => p.exited));
stop();
const codes = await Promise.all(procs.map((p) => p.exited));
process.exit(first !== 0 ? first : (codes.find((c) => c !== 0) ?? 0));
