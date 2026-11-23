import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

declare const __UPSTREAM_DIR__: string;
declare const __HARNESS_DIR__: string;

// The real main.ts and download.ts, bundled at run time so `ssh2`, `fs`, `path`, `electron` and the
// timers main.ts sees come from `globalThis.__asrsDownload`, which the generator installs first.
// Nothing in the download section is replaced; main.ts's neighbours outside it are stubbed as
// build.mjs stubs them for the files harness. `ssh2` is whatever the generator hands over: a
// scripted Client for `script`, the real package for `live`.

const MAIN_STUBS: { [rel: string]: string } = {
  "main/electron/UpdateChecker": "export default class UpdateChecker { getShouldPrompt() { return false; } showPrompt() {} checkForUpdate() {} }",
  "main/electron/VideoProcessor": "export class VideoProcessor { static cleanup() {} }",
  "main/electron/XRControls": "export const XRControls = { addSourceUUIDCallback() {} };",
  "main/electron/XRServer": "export class XRServer {}",
  "main/electron/assetDownloader": "export function getAssetDownloadStatus() { return null; } export function startAssetDownloadLoop() {}",
  "main/electron/assetLoader":
    "export function createAssetFolders() {} export function getUserAssetsPath() { return '/fake/userData/userAssets'; } export function loadAssets() { return Promise.resolve({}); }",
  "main/electron/owletDownloadLoop": "export function getOwletDownloadStatus() { return null; } export function startOwletDownloadLoop() {}",
  "main/electron/owletInterface":
    "export function checkHootIsPro() { return Promise.resolve(true); } export function convertHoot() { return Promise.resolve(''); } export const CTRE_LICENSE_URL = '';",
  "main/github": "export const GITHUB_REPOSITORY = 'Mechanical-Advantage/AdvantageScope';"
};
const GLUE: { [specifier: string]: string } = {
  electron: "module.exports = globalThis.__asrsDownload.electron;",
  fs: "module.exports = globalThis.__asrsDownload.fs;",
  ssh2: "module.exports = globalThis.__asrsDownload.ssh2;",
  "lzma-native": "module.exports = {};",
  "@rev-robotics/revlog-converter": "export function parseREVLOG() { return null; }"
};

const silenced = { "direct-eval": "silent", "commonjs-variable-in-esm": "silent", "equals-negative-zero": "silent" };

async function build(options: any, name: string): Promise<string> {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-download-"));
  process.on("exit", () => fs.rmSync(dir, { recursive: true, force: true }));
  const outfile = path.join(dir, name);
  const upstream = fs.realpathSync(__UPSTREAM_DIR__);
  await createRequire(path.join(__HARNESS_DIR__, "package.json"))("esbuild").build({
    outfile,
    bundle: true,
    target: "esnext",
    logLevel: "warning",
    logOverride: silenced,
    nodePaths: [path.join(upstream, "node_modules")],
    ...options
  });
  return outfile;
}

/** main.ts and ElectronConstants. `flavour` picks the `path` module main.ts sees. */
export async function buildMain(flavour: "posix" | "win32"): Promise<string> {
  const srcDir = path.join(fs.realpathSync(__UPSTREAM_DIR__), "src");
  const version = JSON.parse(fs.readFileSync(path.join(srcDir, "../package.json"), "utf8"));
  const replacements: { [token: string]: string } = {
    __distribution__: "FRC6328",
    __version__: version.version,
    __build_date__: "1970-01-01T00:00:00.000Z",
    __copyright__: version.build.copyright
  };
  const file = (rel: string) => JSON.stringify(path.join(srcDir, rel));
  const outfile = await build(
    {
      stdin: {
        contents:
          `export * as Constants from ${file("main/electron/ElectronConstants.ts")};\n` +
          `export async function load() { await import(${file("main/electron/main.ts")}); }\n`,
        resolveDir: srcDir,
        loader: "ts"
      },
      format: "esm",
      platform: "node",
      banner: { js: 'import { createRequire as __asrsCreateRequire } from "node:module"; const require = __asrsCreateRequire(import.meta.url);' },
      define: {
        __dirname: JSON.stringify("/Applications/AdvantageScope.app/Contents/Resources/app.asar/bundles"),
        setTimeout: "globalThis.__asrsDownload.setTimeout",
        clearTimeout: "globalThis.__asrsDownload.clearTimeout",
        setInterval: "globalThis.__asrsDownload.setInterval",
        clearInterval: "globalThis.__asrsDownload.clearInterval"
      },
      plugins: [
        {
          name: "asrs-download-glue",
          setup(build: any) {
            build.onResolve({ filter: /^(electron|fs|node:fs|graceful-fs|path|ssh2|lzma-native|@rev-robotics\/revlog-converter)$/ }, (args: any) => {
              if (!args.importer.includes("/src/main/") && !args.importer.includes("node_modules/jsonfile")) return undefined;
              const name = args.path === "node:fs" || args.path === "graceful-fs" ? "fs" : args.path;
              if (name === "path" && !args.importer.endsWith("/main/electron/main.ts")) return undefined;
              return { path: name, namespace: "asrs-download-glue" };
            });
            build.onResolve({ filter: /^\./ }, (args: any) => {
              if (!args.importer.startsWith(path.join(srcDir, "main/electron") + path.sep)) return undefined;
              const rel = path.relative(srcDir, path.resolve(path.dirname(args.importer), args.path)).split(path.sep).join("/");
              return MAIN_STUBS[rel] === undefined ? undefined : { path: "stub:" + rel, namespace: "asrs-download-glue" };
            });
            build.onLoad({ filter: /.*/, namespace: "asrs-download-glue" }, (args: any) => {
              if (args.path.startsWith("stub:")) return { contents: MAIN_STUBS[args.path.slice(5)], loader: "js" };
              if (args.path === "path") return { contents: `module.exports = require("node:path").${flavour};`, loader: "js" };
              return { contents: GLUE[args.path], loader: "js" };
            });
            build.onLoad({ filter: /buildConstants\.ts$/ }, (args: any) => ({
              contents: fs
                .readFileSync(args.path, "utf8")
                .replace(/\b(__distribution__|__version__|__build_date__|__copyright__)\b(?!\.)(?!\s*=[^=])/g, (t) => replacements[t]),
              loader: "ts"
            }));
          }
        }
      ]
    },
    "main.mjs"
  );
  return pathToFileURL(outfile).href;
}

/** download.ts as a script; its `window` and `document` are the caller's globals. */
export async function buildWindow(): Promise<string> {
  const srcDir = path.join(fs.realpathSync(__UPSTREAM_DIR__), "src");
  return fs.readFileSync(
    await build({ entryPoints: [path.join(srcDir, "download.ts")], format: "iife", platform: "browser" }, "download.js"),
    "utf8"
  );
}

export function upstreamRequire(): NodeJS.Require {
  return createRequire(path.join(fs.realpathSync(__UPSTREAM_DIR__), "package.json"));
}
