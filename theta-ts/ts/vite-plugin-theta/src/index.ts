import { createRequire } from "module";
import path from "path";
import type { Plugin, ViteDevServer } from "vite";
import wasm from "vite-plugin-wasm";
import { parseActors, type ActorInfo } from "./actors.js";
import { runWasmPack, watchRustSources, type WasmPackContext } from "./wasm-pack.js";
import { generateTypeDeclaration } from "./type-gen.js";
import { generateUmbrellaCrate, readCratePackageName, type UmbrellaCrateEntry } from "./umbrella.js";

export interface ThetaCrateEntry {
  /** Path to the Rust crate (relative to Vite project root). */
  path: string;
  /** wasm-pack --features for this crate (default: plugin-level features). */
  features?: string;
}

export interface ThetaPluginOptions {
  /**
   * Single crate path (relative to Vite project root).
   * Shorthand for `crates: [{ path: '...' }]`.
   * Cannot be used together with `crates`.
   */
  crate?: string;
  /**
   * Multiple crate paths (or entry objects) for a multi-crate build.
   * The plugin auto-generates an umbrella crate that re-exports all of them
   * and runs a single wasm-pack build.
   * Cannot be used together with `crate`.
   */
  crates?: Array<string | ThetaCrateEntry>;
  /** Default wasm-pack --features value (default: "ts,remote"). */
  features?: string;
  /** wasm-pack --out-dir relative to crate (default: "pkg"). */
  outDir?: string;
  /** Rebuild debounce in ms (default: 300). */
  debounce?: number;
}

const VIRTUAL_ID = "theta:actors";
const RESOLVED_VIRTUAL_ID = "\0theta:actors";

export default function theta(options: ThetaPluginOptions): Plugin[] {
  if (options.crate && options.crates) {
    throw new Error("[theta] Specify either `crate` or `crates`, not both.");
  }
  if (!options.crate && !options.crates) {
    throw new Error("[theta] You must specify either `crate` or `crates`.");
  }

  const {
    features: defaultFeatures = "ts,remote",
    outDir = "pkg",
    debounce = 300,
  } = options;

  // True if we're in multi-crate mode (auto-umbrella)
  const isMulti = Boolean(options.crates);

  let projectRoot: string;
  /** Absolute path to the crate wasm-pack will build (single or umbrella). */
  let buildCratePath: string;
  /** Absolute path to the wasm-pack pkg/ output directory. */
  let pkgDir: string;
  /** wasm-pack output module name (filename stem). */
  let moduleName: string;
  /** Absolute src/ dirs of all sub-crates to watch in HMR mode. */
  let subCrateSrcDirs: string[] = [];
  let server: ViteDevServer | undefined;
  /** Detected actors from wasm-pack output .d.ts. */
  let actorInfos: ActorInfo[] = [];
  /** Whether theta-ts is available in the project. */
  let hooksEnabled = false;

  function resolve(): void {
    if (options.crate) {
      // ── Single-crate mode ──────────────────────────────────────────────
      buildCratePath = path.resolve(projectRoot, options.crate);
      pkgDir = path.resolve(buildCratePath, outDir);
      moduleName = readCratePackageName(buildCratePath).replace(/-/g, "_");
    } else {
      // ── Multi-crate mode: generate umbrella ────────────────────────────
      const rawEntries = options.crates!;
      const entries: UmbrellaCrateEntry[] = rawEntries.map((e) => {
        if (typeof e === "string") {
          return { cratePath: path.resolve(projectRoot, e), features: defaultFeatures };
        }
        return { cratePath: path.resolve(projectRoot, e.path), features: e.features ?? defaultFeatures };
      });

      const umbrellaDir = path.resolve(projectRoot, "node_modules/.theta/umbrella");
      moduleName = generateUmbrellaCrate(umbrellaDir, entries);
      buildCratePath = umbrellaDir;
      pkgDir = path.resolve(umbrellaDir, outDir);

      subCrateSrcDirs = entries.map((e) => path.resolve(e.cratePath, "src"));
    }
  }

  const ctx = (): WasmPackContext => ({
    cratePath: buildCratePath,
    features: isMulti ? "" : defaultFeatures,
    outDir,
    debounce,
  });

  const thetaPlugin: Plugin = {
    name: "vite-plugin-theta",
    enforce: "pre",

    configResolved(config) {
      projectRoot = config.root;
      resolve();
    },

    configureServer(srv) {
      server = srv;
      // Ensure wasm pkg dir is in the allow list (it may be outside project root)
      const fsConfig = srv.config.server.fs;
      if (fsConfig.allow) {
        fsConfig.allow.push(pkgDir);
      } else {
        fsConfig.allow = [pkgDir];
      }
    },

    resolveId(id) {
      if (id === VIRTUAL_ID) return RESOLVED_VIRTUAL_ID;
    },

    load(id) {
      if (id === RESOLVED_VIRTUAL_ID) {
        const jsFile = path.join(pkgDir, `${moduleName}.js`);
        // Re-export everything from the wasm-pack output (actors, types, raw init).
        // Also inject a convenience `initTheta()` that calls wasm-bindgen's `init()`
        // followed by `initThetaRemote()` so callers never need to know about the
        // two-step initialisation.
        const lines = [
          `export * from ${JSON.stringify(jsFile)};`,
          `export { default } from ${JSON.stringify(jsFile)};`,
          `import __wasmInit, { initThetaRemote as __initThetaRemote } from ${JSON.stringify(jsFile)};`,
          `let __initialized = false;`,
          `let __initPromise = null;`,
          `export async function initTheta() {`,
          `  if (__initialized) return;`,
          `  if (__initPromise) return __initPromise;`,
          `  __initPromise = (async () => {`,
          `    await __wasmInit();`,
          `    const key = await __initThetaRemote();`,
          `    __initialized = true;`,
          `    return key;`,
          `  })();`,
          `  return __initPromise;`,
          `}`,
        ];

        // Per-actor React hook aliases (only when theta-ts is available)
        if (hooksEnabled && actorInfos.length > 0) {
          lines.push(`export { useActorView, useActorRef } from 'theta-ts/react';`);
          lines.push(`import { useActorView as _uav } from 'theta-ts/react';`);
          for (const actor of actorInfos) {
            lines.push(`export const use${actor.name}View = _uav;`);
          }
        }

        return lines.join("\n");
      }
    },

    async buildStart() {
      await runWasmPack(ctx());
      actorInfos = parseActors(pkgDir, moduleName);

      // Auto-detect theta-ts availability for hook alias generation
      try {
        const require = createRequire(path.join(projectRoot, "package.json"));
        require.resolve("theta-ts/react");
        hooksEnabled = true;
      } catch {
        hooksEnabled = false;
      }

      generateTypeDeclaration(projectRoot, pkgDir, moduleName, hooksEnabled ? actorInfos : []);

      // In serve mode, watch for .rs changes
      if (server !== undefined) {
        watchRustSources(ctx(), server, () => {
          // Regenerate umbrella in case crate re-exports changed
          if (isMulti) {
            const rawEntries = options.crates!;
            const entries: UmbrellaCrateEntry[] = rawEntries.map((e) => {
              if (typeof e === "string") {
                return { cratePath: path.resolve(projectRoot, e), features: defaultFeatures };
              }
              return { cratePath: path.resolve(projectRoot, e.path), features: e.features ?? defaultFeatures };
            });
            const umbrellaDir = path.resolve(projectRoot, "node_modules/.theta/umbrella");
            generateUmbrellaCrate(umbrellaDir, entries);
          }
          generateTypeDeclaration(projectRoot, pkgDir, moduleName, hooksEnabled ? actorInfos : []);
        }, subCrateSrcDirs);
      }
    },

    config() {
      return {
        optimizeDeps: {
          exclude: [VIRTUAL_ID],
        },
        // SharedArrayBuffer (required by WASM threads) needs COOP/COEP headers.
        // Users who set these manually in their own vite.config will take precedence
        // (Vite deep-merges plugin config, user config wins on conflict).
        server: {
          headers: {
            "Cross-Origin-Opener-Policy": "same-origin",
            "Cross-Origin-Embedder-Policy": "require-corp",
          },
        },
      };
    },
  };

  return [thetaPlugin, wasm() as Plugin];
}
