import { spawn } from "child_process";
import { watch } from "fs";
import path from "path";
import type { ViteDevServer } from "vite";

export interface WasmPackContext {
  cratePath: string;
  features: string;
  outDir: string;
  debounce: number;
}

let building = false;

export function runWasmPack(ctx: WasmPackContext): Promise<boolean> {
  return new Promise((done) => {
    if (building) {
      done(false);
      return;
    }
    building = true;
    console.log("[theta] Building WASM...");

    const args = [
      "build",
      "--target",
      "web",
      "--out-dir",
      ctx.outDir,
    ];
    if (ctx.features.trim()) {
      args.push("--features", ctx.features);
    }

    const proc = spawn("wasm-pack", args, { cwd: ctx.cratePath, stdio: "pipe" });

    let stderr = "";
    proc.stderr?.on("data", (d: Buffer) => (stderr += d.toString()));

    proc.on("close", (code: number | null) => {
      building = false;
      if (code === 0) {
        console.log("[theta] WASM build succeeded");
        done(true);
      } else {
        console.error("[theta] WASM build failed:\n" + stderr);
        done(false);
      }
    });
  });
}

export function watchRustSources(
  ctx: WasmPackContext,
  server: ViteDevServer | undefined,
  onRebuilt?: () => void,
  /** Additional source dirs to watch (e.g. sub-crate src/ dirs). */
  extraSrcDirs?: string[]
): void {
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  const onChange = (_event: unknown, filename: unknown) => {
    if (filename && filename.toString().endsWith(".rs")) {
      console.log(`[theta] ${filename} changed, rebuilding...`);
      if (debounceTimer) clearTimeout(debounceTimer);
      debounceTimer = setTimeout(async () => {
        const ok = await runWasmPack(ctx);
        if (ok) {
          onRebuilt?.();
          server?.ws.send({ type: "full-reload", path: "*" });
        }
      }, ctx.debounce);
    }
  };

  const dirsToWatch = [
    ...(extraSrcDirs ?? []),
  ];

  for (const dir of dirsToWatch) {
    watch(dir, { recursive: true }, onChange);
  }
}
