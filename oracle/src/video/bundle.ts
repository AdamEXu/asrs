import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import type { Platform } from "./world";
import { PLATFORM_PATHS } from "./world";

declare const __UPSTREAM_DIR__: string;
declare const __HARNESS_DIR__: string;

// build.mjs stubs VideoProcessor for every main-process harness, so this entry bundles the real
// one itself at run time. Each import specifier the fakes replace resolves to a module reading
// `globalThis.__asrsVideo`, which World installs before every import; `path` is the platform's
// flavour so win32 joins use backslashes. One bundle per platform, imported once per case under
// a distinct query so the class's static state starts fresh.

const GLUE: { [specifier: string]: string } = {
  electron:
    "const g = globalThis.__asrsVideo.electron; export const app = g.app; export const clipboard = g.clipboard; " +
    "export const dialog = g.dialog; export const Menu = g.Menu; export const MenuItem = g.MenuItem; export const BrowserWindow = g.BrowserWindow;",
  fs: "module.exports = globalThis.__asrsVideo.fs;",
  "node:fs": "module.exports = globalThis.__asrsVideo.fs;",
  "graceful-fs": "module.exports = globalThis.__asrsVideo.fs;",
  child_process: "module.exports = globalThis.__asrsVideo.childProcess;",
  download: "module.exports = globalThis.__asrsVideo.download;",
  "tesseract.js": "module.exports = globalThis.__asrsVideo.tesseract;",
  "node:crypto": "module.exports = globalThis.__asrsVideo.crypto;"
};

export async function buildBundles(): Promise<{ [p in Platform]: string }> {
  const esbuild = createRequire(path.join(__HARNESS_DIR__, "package.json"))("esbuild");
  const srcDir = path.join(__UPSTREAM_DIR__, "src");
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-video-"));
  process.on("exit", () => fs.rmSync(outDir, { recursive: true, force: true }));
  const out: any = {};
  for (const platform of ["darwin", "linux", "win32"] as Platform[]) {
    const flavour = platform === "win32" ? "win32" : "posix";
    const glue = { ...GLUE, path: `module.exports = require("node:path").${flavour};` };
    const entry =
      `export { VideoProcessor } from ${JSON.stringify(path.join(srcDir, "main/electron/VideoProcessor.ts"))};\n` +
      `export * as Constants from ${JSON.stringify(path.join(srcDir, "main/electron/ElectronConstants.ts"))};\n` +
      `export { default as getElectronPlatform } from ${JSON.stringify(path.join(srcDir, "main/electron/getElectronPlatform.ts"))};\n`;
    const outfile = path.join(outDir, platform + ".mjs");
    await esbuild.build({
      stdin: { contents: entry, resolveDir: srcDir, loader: "ts" },
      outfile,
      bundle: true,
      format: "esm",
      platform: "node",
      target: "esnext",
      logLevel: "warning",
      logOverride: { "direct-eval": "silent", "commonjs-variable-in-esm": "silent", "equals-negative-zero": "silent" },
      nodePaths: [path.join(__UPSTREAM_DIR__, "node_modules")],
      banner: { js: 'import { createRequire as __asrsCreateRequire } from "node:module"; const require = __asrsCreateRequire(import.meta.url);' },
      define: {
        __dirname: JSON.stringify(PLATFORM_PATHS[platform].dirname),
        fetch: "globalThis.__asrsVideo.fetch",
        setTimeout: "globalThis.__asrsVideo.setTimeout",
        "AbortSignal.timeout": "globalThis.__asrsVideo.abortTimeout",
        "Math.random": "globalThis.__asrsVideo.random",
        console: "globalThis.__asrsVideo.console"
      },
      plugins: [
        {
          name: "asrs-video-glue",
          setup(build: any) {
            const filter = new RegExp("^(" + Object.keys(glue).map((k) => k.replace(/[.]/g, "\\.")).join("|") + ")$");
            build.onResolve({ filter }, (args: any) => ({ path: args.path, namespace: "asrs-video-glue" }));
            build.onLoad({ filter: /.*/, namespace: "asrs-video-glue" }, (args: any) => ({
              contents: (glue as any)[args.path],
              loader: "js"
            }));
          }
        }
      ]
    });
    out[platform] = pathToFileURL(outfile).href;
  }
  return out;
}
