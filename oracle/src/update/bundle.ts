import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

declare const __UPSTREAM_DIR__: string;
declare const __HARNESS_DIR__: string;

export type Platform = "darwin" | "linux" | "win32";

// build.mjs stubs UpdateChecker and main/github for every main-process harness, so this entry
// bundles the real ones itself at run time (like genVideo). Every specifier the fakes replace
// resolves to a module reading `globalThis.__asrsUpdate`, which the generator installs before
// each import. `betaConfig` and `github` re-export upstream's real values unless a case swaps
// in a config variant (BETA_CONFIG null for a stable build, another repository for asrs's
// release builds); the code under test is never replaced.

export const DIRNAME: { [p in Platform]: string } = {
  darwin: "/Applications/AdvantageScope.app/Contents/Resources/app.asar/bundles",
  linux: "/opt/AdvantageScope/resources/app.asar/bundles",
  win32: "C:\\Program Files\\AdvantageScope\\resources\\app.asar\\bundles"
};

const silenced = { "direct-eval": "silent", "commonjs-variable-in-esm": "silent", "equals-negative-zero": "silent" };

function gluePlugin(glue: { [specifier: string]: string }, rewrite: { [pattern: string]: string }) {
  return {
    name: "asrs-update-glue",
    setup(build: any) {
      const filter = new RegExp("^(" + Object.keys(glue).map((k) => k.replace(/[.]/g, "\\.")).join("|") + ")$");
      build.onResolve({ filter }, (args: any) => ({ path: args.path, namespace: "asrs-update-glue" }));
      for (const [pattern, name] of Object.entries(rewrite)) {
        build.onResolve({ filter: new RegExp(pattern) }, (args: any) =>
          args.importer.includes("/src/") ? { path: name, namespace: "asrs-update-glue" } : undefined
        );
      }
      build.onLoad({ filter: /.*/, namespace: "asrs-update-glue" }, (args: any) => ({
        contents: glue[args.path],
        loader: "js",
        resolveDir: path.join(__UPSTREAM_DIR__, "src")
      }));
    }
  };
}

function esbuild() {
  return createRequire(path.join(__HARNESS_DIR__, "package.json"))("esbuild");
}

function outDir(): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-update-"));
  process.on("exit", () => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

export async function buildCheckerBundles(): Promise<{ [p in Platform]: string }> {
  const srcDir = path.join(__UPSTREAM_DIR__, "src");
  const dir = outDir();
  const out: any = {};
  for (const platform of ["darwin", "linux", "win32"] as Platform[]) {
    const flavour = platform === "win32" ? "win32" : "posix";
    const glue: { [specifier: string]: string } = {
      electron:
        "const g = globalThis.__asrsUpdate.electron; export const app = g.app; export const dialog = g.dialog; " +
        "export const net = g.net; export const shell = g.shell;",
      path: `module.exports = require("node:path").${flavour};`,
      fs: "module.exports = globalThis.__asrsUpdate.fs;",
      "graceful-fs": "module.exports = globalThis.__asrsUpdate.fs;",
      "asrs-update:betaConfig":
        `import { BETA_CONFIG as real } from ${JSON.stringify(path.join(srcDir, "main/betaConfig.ts"))};\n` +
        "export const BETA_CONFIG = globalThis.__asrsUpdate.beta ? real : null;",
      "asrs-update:github":
        `import * as real from ${JSON.stringify(path.join(srcDir, "main/github.ts"))};\n` +
        "export const GITHUB_REPOSITORY = globalThis.__asrsUpdate.repository ?? real.GITHUB_REPOSITORY;\n" +
        "export const GITHUB_ASSETS_REPOSITORY = real.GITHUB_ASSETS_REPOSITORY;\n" +
        "export const GITHUB_ASSET_TAG_DEFAULT = real.GITHUB_ASSET_TAG_DEFAULT;\n" +
        "export const GITHUB_ASSET_TAG_FRC6328 = real.GITHUB_ASSET_TAG_FRC6328;"
    };
    const entry =
      `export { default as UpdateChecker } from ${JSON.stringify(path.join(srcDir, "main/electron/UpdateChecker.ts"))};\n` +
      `export * as Constants from ${JSON.stringify(path.join(srcDir, "main/electron/ElectronConstants.ts"))};\n` +
      'export { GITHUB_REPOSITORY } from "asrs-update:github";\n';
    const outfile = path.join(dir, platform + ".mjs");
    await esbuild().build({
      stdin: { contents: entry, resolveDir: srcDir, loader: "ts" },
      outfile,
      bundle: true,
      format: "esm",
      platform: "node",
      target: "esnext",
      logLevel: "warning",
      logOverride: silenced,
      nodePaths: [path.join(__UPSTREAM_DIR__, "node_modules")],
      banner: { js: 'import { createRequire as __asrsCreateRequire } from "node:module"; const require = __asrsCreateRequire(import.meta.url);' },
      define: {
        __dirname: JSON.stringify(DIRNAME[platform]),
        console: "globalThis.__asrsUpdate.console"
      },
      plugins: [gluePlugin(glue, { "^\\.\\./betaConfig$": "asrs-update:betaConfig", "^\\.\\./github$": "asrs-update:github" })]
    });
    out[platform] = pathToFileURL(outfile).href;
  }
  return out;
}

export async function buildLicensesBundle(): Promise<string> {
  const dir = outDir();
  const glue = {
    fs: "export default globalThis.__asrsLicenses.fs;",
    path: 'import path from "node:path"; export default path.posix;'
  };
  const outfile = path.join(dir, "getLicenses.mjs");
  await esbuild().build({
    entryPoints: [path.join(__UPSTREAM_DIR__, "getLicenses.mjs")],
    outfile,
    bundle: true,
    format: "esm",
    platform: "node",
    target: "esnext",
    logLevel: "warning",
    logOverride: silenced,
    define: {
      fetch: "globalThis.__asrsLicenses.fetch",
      console: "globalThis.__asrsLicenses.console"
    },
    plugins: [gluePlugin(glue, {})]
  });
  return pathToFileURL(outfile).href;
}

// main.ts's neighbours outside P07, stubbed as build.mjs stubs them for the files harness.
// UpdateChecker and github stay real here.
const MAIN_STUBS: { [rel: string]: string } = {
  "main/electron/VideoProcessor": "export class VideoProcessor { static cleanup() {} }",
  "main/electron/XRControls": "export const XRControls = { addSourceUUIDCallback() {} };",
  "main/electron/XRServer": "export class XRServer {}",
  "main/electron/assetDownloader": "export function getAssetDownloadStatus() { return null; } export function startAssetDownloadLoop() {}",
  "main/electron/assetLoader":
    "export function createAssetFolders() {} export function getUserAssetsPath() { return '/fake/userData/userAssets'; } export function loadAssets() { return Promise.resolve({}); }",
  "main/electron/owletDownloadLoop": "export function getOwletDownloadStatus() { return null; } export function startOwletDownloadLoop() {}",
  "main/electron/owletInterface":
    "export function checkHootIsPro() { return Promise.resolve(true); } export function convertHoot() { return Promise.resolve(''); } export const CTRE_LICENSE_URL = '';"
};
const PACKAGE_STUBS: { [name: string]: string } = {
  "lzma-native": "module.exports = {};",
  ssh2: "export class Client {}",
  "@rev-robotics/revlog-converter": "export function parseREVLOG() { return null; }"
};

// The real main.ts against the `files` fakes, which the generator imports statically and hands over
// through `globalThis.__asrsMain` so this bundle and the generator share one instance of each.
// Every path is resolved through realpath so main.ts and the entry's own imports
// (ElectronConstants, betaConfig) are one module instance even when target/ is a symlink.
// `neighbours` replaces a stub's source, or with `null` bundles that neighbour for real.
export async function buildMainBundle(neighbours: { [rel: string]: string | null } = {}): Promise<string> {
  const stubs: { [rel: string]: string | null } = { ...MAIN_STUBS, ...neighbours };
  const upstream = fs.realpathSync(__UPSTREAM_DIR__);
  const srcDir = path.join(upstream, "src");
  const upstreamPackage = JSON.parse(fs.readFileSync(path.join(upstream, "package.json"), "utf8"));
  const replacements: { [token: string]: string } = {
    __distribution__: "FRC6328",
    __version__: upstreamPackage.version,
    __build_date__: "1970-01-01T00:00:00.000Z",
    __copyright__: upstreamPackage.build.copyright
  };
  const dir = outDir();
  const outfile = path.join(dir, "main.mjs");
  const file = (rel: string) => JSON.stringify(path.join(srcDir, rel));
  const entry =
    `export * as Constants from ${file("main/electron/ElectronConstants.ts")};\n` +
    `export { BETA_CONFIG } from ${file("main/betaConfig.ts")};\n` +
    `export async function load() { await import(${file("main/electron/main.ts")}); }\n`;
  await esbuild().build({
    stdin: { contents: entry, resolveDir: srcDir, loader: "ts" },
    outfile,
    bundle: true,
    format: "esm",
    platform: "node",
    target: "esnext",
    logLevel: "warning",
    logOverride: silenced,
    nodePaths: [path.join(upstream, "node_modules")],
    banner: { js: 'import { createRequire as __asrsCreateRequire } from "node:module"; const require = __asrsCreateRequire(import.meta.url);' },
    define: { __dirname: JSON.stringify(DIRNAME.darwin) },
    plugins: [
      {
        name: "asrs-main-glue",
        setup(build: any) {
          build.onResolve({ filter: /^(electron|graceful-fs|fs|node:fs)$/ }, (args: any) => {
            if (!args.importer.includes("/src/main/") && !args.importer.includes("node_modules/jsonfile")) return undefined;
            return { path: args.path === "electron" ? "electron" : "fs", namespace: "asrs-main-glue" };
          });
          build.onResolve({ filter: /^(lzma-native|ssh2|@rev-robotics\/revlog-converter)$/ }, (args: any) => ({
            path: "pkg:" + args.path,
            namespace: "asrs-main-glue"
          }));
          build.onResolve({ filter: /^\./ }, (args: any) => {
            if (!args.importer.startsWith(path.join(srcDir, "main/electron") + path.sep)) return undefined;
            const rel = path.relative(srcDir, path.resolve(path.dirname(args.importer), args.path)).split(path.sep).join("/");
            return stubs[rel] === undefined || stubs[rel] === null ? undefined : { path: "stub:" + rel, namespace: "asrs-main-glue" };
          });
          build.onLoad({ filter: /.*/, namespace: "asrs-main-glue" }, (args: any) => {
            if (args.path === "electron") return { contents: "module.exports = globalThis.__asrsMain.electron;", loader: "js" };
            if (args.path === "fs") return { contents: "module.exports = globalThis.__asrsMain.fs;", loader: "js" };
            if (args.path.startsWith("pkg:")) return { contents: PACKAGE_STUBS[args.path.slice(4)], loader: "js" };
            return { contents: stubs[args.path.slice(5)]!, loader: "ts" };
          });
          build.onLoad({ filter: /buildConstants\.ts$/ }, (args: any) => {
            const source = fs.readFileSync(args.path, "utf8");
            return {
              contents: source.replace(/\b(__distribution__|__version__|__build_date__|__copyright__)\b(?!\.)(?!\s*=[^=])/g, (t) => replacements[t]),
              loader: "ts"
            };
          });
        }
      }
    ]
  });
  return pathToFileURL(outfile).href;
}
