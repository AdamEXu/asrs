import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const USAGE = "usage: node build.mjs --upstream <dir> --harness <dir with node_modules> --out <dir>";

const NPM_PACKAGES = [
  "@mcap/core",
  "@msgpack/msgpack",
  "ansi_up",
  "bignumber.js",
  "color-convert",
  "compare-versions",
  "fuse.js",
  "mathjs",
  "protobufjs",
  "simple-statistics",
  "three"
];

const MODULE_ROOTS = ["shared", "hub"];
const MODULE_FILES = ["main/assetUtil.ts"];

function isExcludedModule(id) {
  return id === "hub/hub" || /Worker$/.test(id) || id.startsWith("shared/renderers/field3d/workers/");
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i];
    if (!key.startsWith("--") || argv[i + 1] === undefined) throw new Error(USAGE);
    out[key.slice(2)] = path.resolve(argv[i + 1]);
  }
  for (const key of ["upstream", "harness", "out"]) if (!out[key]) throw new Error(USAGE);
  return out;
}

function listUpstreamModules(srcDir) {
  const ids = [];
  const visit = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) visit(full);
      else if (entry.name.endsWith(".ts") && !entry.name.endsWith(".d.ts")) {
        const id = path.relative(srcDir, full).slice(0, -3).split(path.sep).join("/");
        if (!isExcludedModule(id)) ids.push(id);
      }
    }
  };
  MODULE_ROOTS.forEach((root) => visit(path.join(srcDir, root)));
  for (const file of MODULE_FILES) {
    const id = file.slice(0, -3);
    if (!isExcludedModule(id)) ids.push(id);
  }
  return ids.sort();
}

const args = parseArgs(process.argv.slice(2));
const here = path.dirname(fileURLToPath(import.meta.url));
const srcDir = path.join(args.upstream, "src");
const esbuild = createRequire(path.join(args.harness, "package.json"))("esbuild");

const upstreamModules = listUpstreamModules(srcDir);

const upstreamPackage = JSON.parse(fs.readFileSync(path.join(args.upstream, "package.json"), "utf8"));
// Mirrors the replacePlugin values in upstream's rolldown.config.mjs for the FRC6328 build. The
// build date is pinned instead of the wall clock so harness output stays reproducible.
const buildReplacements = {
  __distribution__: "FRC6328",
  __version__: upstreamPackage.version,
  __build_date__: "1970-01-01T00:00:00.000Z",
  __copyright__: upstreamPackage.build.copyright
};
const realSrcDir = fs.realpathSync(srcDir);
const buildReplacementPattern = new RegExp(`\\b(${Object.keys(buildReplacements).join("|")})\\b(?!\\.)(?!\\s*=[^=])`, "g");

const virtualModules = {
  "oracle:upstream-modules":
    "export const UPSTREAM_MODULES = {\n" +
    upstreamModules.map((id) => `  ${JSON.stringify(id)}: () => import(${JSON.stringify(path.join(srcDir, id + ".ts"))}),`).join("\n") +
    "\n};\n",
  "oracle:npm-modules":
    "export const NPM_MODULES = {\n" +
    NPM_PACKAGES.map((pkg) => `  ${JSON.stringify(pkg)}: () => import(${JSON.stringify(pkg)}),`).join("\n") +
    "\n};\n"
};

const oraclePlugin = {
  name: "oracle",
  setup(build) {
    build.onResolve({ filter: /^oracle:/ }, (resolveArgs) => ({ path: resolveArgs.path, namespace: "oracle-virtual" }));
    build.onLoad({ filter: /.*/, namespace: "oracle-virtual" }, (loadArgs) => ({
      contents: virtualModules[loadArgs.path],
      loader: "js",
      resolveDir: srcDir
    }));
    build.onLoad({ filter: /\.ts$/ }, (loadArgs) => {
      if (![srcDir, realSrcDir].some((dir) => loadArgs.path.startsWith(dir + path.sep))) return undefined;
      const source = fs.readFileSync(loadArgs.path, "utf8");
      buildReplacementPattern.lastIndex = 0;
      if (!buildReplacementPattern.test(source)) return undefined;
      return { contents: source.replace(buildReplacementPattern, (token) => buildReplacements[token]), loader: "ts" };
    });
    build.onResolve({ filter: /^@upstream\// }, (resolveArgs) => {
      const base = path.join(srcDir, resolveArgs.path.slice("@upstream/".length));
      const found = ["", ".ts", "/index.ts"].map((ext) => base + ext).find((p) => fs.statSync(p, { throwIfNoEntry: false })?.isFile());
      if (found === undefined) return { errors: [{ text: `cannot resolve ${resolveArgs.path}` }] };
      return { path: found };
    });
  }
};

const entries = fs
  .readdirSync(path.join(here, "src"), { withFileTypes: true })
  .filter((entry) => entry.isFile() && entry.name.endsWith(".ts") && !entry.name.endsWith(".d.ts"))
  .map((entry) => entry.name.slice(0, -3))
  .sort();

const realHere = fs.realpathSync(here);
const harnessInputs = {};
function recordInputs(metafile) {
  for (const [outPath, output] of Object.entries(metafile.outputs)) {
    if (!output.entryPoint) continue;
    const name = path.basename(outPath).replace(/\.m?js$/, "");
    const files = new Set(harnessInputs[name] ?? []);
    for (const input of Object.keys(output.inputs)) {
      if (/^([\w-]+|\(disabled\)):/.test(input) && !path.isAbsolute(input)) continue;
      const full = fs.realpathSync(path.resolve(here, input));
      const rel = path.relative(realHere, full);
      if (rel.startsWith("..") || path.isAbsolute(rel) || rel.split(path.sep).includes("node_modules")) continue;
      files.add(rel.split(path.sep).join("/"));
    }
    harnessInputs[name] = [...files].sort();
  }
}

const common = { absWorkingDir: here, metafile: true, bundle: true, target: "esnext", logLevel: "warning" };

recordInputs(
  (
    await esbuild.build({
      ...common,
      entryPoints: Object.fromEntries(entries.map((name) => [name, path.join(here, "src", name + ".ts")])),
      outdir: args.out,
      outExtension: { ".js": ".mjs" },
      format: "esm",
      platform: "node",
      sourcemap: "linked",
      nodePaths: [path.join(args.upstream, "node_modules")],
      define: { __UPSTREAM_DIR__: JSON.stringify(args.upstream), __HARNESS_DIR__: JSON.stringify(args.harness) },
      banner: { js: 'import { createRequire as __oracleCreateRequire } from "node:module"; const require = __oracleCreateRequire(import.meta.url);' },
      external: ["electron"],
      plugins: [oraclePlugin],
      logOverride: { "direct-eval": "silent", "commonjs-variable-in-esm": "silent", "equals-negative-zero": "silent" }
    })
  ).metafile
);

// The renderer check runs in a real Electron main process and BrowserWindow, not under the shims.
recordInputs(
  (
    await esbuild.build({
      ...common,
      entryPoints: [path.join(here, "src/renderer/main.ts")],
      outfile: path.join(args.out, "rendererMain.mjs"),
      format: "esm",
      platform: "node",
      external: ["electron"]
    })
  ).metafile
);
for (const [entry, outfile] of [
  ["src/renderer/page.ts", "rendererPage.js"],
  ["src/canvas/page.ts", "canvasPage.js"]
]) {
  recordInputs(
    (
      await esbuild.build({
        ...common,
        entryPoints: [path.join(here, entry)],
        outfile: path.join(args.out, outfile),
        format: "iife",
        platform: "browser"
      })
    ).metafile
  );
}
// The DOM harness evaluates the real hub inside a jsdom window, so it is a browser bundle of
// upstream sources with the WebGL-only 3D renderer replaced by a stub.
const field3dStub = {
  name: "field3d-stub",
  setup(build) {
    build.onResolve({ filter: /\/Field3dRendererImpl$/ }, () => ({ path: path.join(here, "src/dom/field3dStub.ts") }));
  }
};
recordInputs(
  (
    await esbuild.build({
      ...common,
      entryPoints: [path.join(here, "src/dom/page.ts")],
      outfile: path.join(args.out, "domPage.js"),
      format: "iife",
      platform: "browser",
      nodePaths: [path.join(args.upstream, "node_modules")],
      plugins: [field3dStub, oraclePlugin],
      logOverride: { "direct-eval": "silent", "equals-negative-zero": "silent" }
    })
  ).metafile
);
fs.copyFileSync(path.join(here, "src/renderer/renderer.html"), path.join(args.out, "renderer.html"));
fs.writeFileSync(
  path.join(args.out, "canvasPage.html"),
  '<!doctype html><html><head><meta charset="utf-8"><style>body{margin:0}</style></head>' +
    '<body><script src="canvasPage.js"></script></body></html>\n'
);

// Bundles an entry starts at runtime (by spawning or loading them in a window) count as its
// inputs, so `oracle gen` invalidates its vectors when they change.
const runtimeLoads = {
  protoRenderer: ["rendererMain", "rendererPage", "src/renderer/renderer.html"],
  canvasMain: ["canvasPage"],
  dom: ["domPage"]
};
const inputs = {};
for (const name of entries) {
  const files = new Set(harnessInputs[name]);
  for (const load of runtimeLoads[name] ?? []) {
    for (const file of harnessInputs[load] ?? [load]) files.add(file);
  }
  inputs[name] = [...files].sort();
}
fs.writeFileSync(path.join(args.out, "inputs.json"), JSON.stringify(inputs, null, 2) + "\n");

fs.writeFileSync(
  path.join(args.out, "modules.json"),
  JSON.stringify({ upstream: upstreamModules, npm: NPM_PACKAGES }, null, 2) + "\n"
);
console.log(`oracle: built ${entries.join(", ")} into ${args.out} (${upstreamModules.length} upstream modules)`);
