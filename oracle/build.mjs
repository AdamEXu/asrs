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

const entries = [
  "fn",
  "selftest",
  "cve1Torture",
  "corpusA",
  "corpusB",
  "corpusFuzz",
  "genCaseData",
  "genUtf8",
  "genJson",
  "genCollation",
  "genCollationUnsafe",
  "genSort",
  "genRegex",
  "genMsgpack",
  "decode",
  "export",
  "historicalRun",
  "protoRenderer",
  "canvasMain"
];

await esbuild.build({
  entryPoints: Object.fromEntries(entries.map((name) => [name, path.join(here, "src", name + ".ts")])),
  outdir: args.out,
  outExtension: { ".js": ".mjs" },
  bundle: true,
  format: "esm",
  platform: "node",
  target: "esnext",
  sourcemap: "linked",
  logLevel: "warning",
  nodePaths: [path.join(args.upstream, "node_modules")],
  define: { __UPSTREAM_DIR__: JSON.stringify(args.upstream) },
  banner: { js: 'import { createRequire as __oracleCreateRequire } from "node:module"; const require = __oracleCreateRequire(import.meta.url);' },
  external: ["electron"],
  plugins: [oraclePlugin],
  logOverride: { "direct-eval": "silent", "commonjs-variable-in-esm": "silent", "equals-negative-zero": "silent" }
});

// The renderer check runs in a real Electron main process and BrowserWindow, not under the shims.
await esbuild.build({
  entryPoints: [path.join(here, "src/renderer/main.ts")],
  outfile: path.join(args.out, "rendererMain.mjs"),
  bundle: true,
  format: "esm",
  platform: "node",
  target: "esnext",
  external: ["electron"],
  logLevel: "warning"
});
for (const [entry, outfile] of [
  ["src/renderer/page.ts", "rendererPage.js"],
  ["src/canvas/page.ts", "canvasPage.js"]
]) {
  await esbuild.build({
    entryPoints: [path.join(here, entry)],
    outfile: path.join(args.out, outfile),
    bundle: true,
    format: "iife",
    platform: "browser",
    target: "esnext",
    logLevel: "warning"
  });
}
fs.copyFileSync(path.join(here, "src/renderer/renderer.html"), path.join(args.out, "renderer.html"));
fs.writeFileSync(
  path.join(args.out, "canvasPage.html"),
  '<!doctype html><html><head><meta charset="utf-8"><style>body{margin:0}</style></head>' +
    '<body><script src="canvasPage.js"></script></body></html>\n'
);

fs.writeFileSync(
  path.join(args.out, "modules.json"),
  JSON.stringify({ upstream: upstreamModules, npm: NPM_PACKAGES }, null, 2) + "\n"
);
console.log(`oracle: built ${entries.join(", ")} into ${args.out} (${upstreamModules.length} upstream modules)`);
