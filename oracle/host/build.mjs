import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const USAGE = "usage: node host/build.mjs --upstream <dir> --harness <dir with node_modules> --out <dir>";

const args = {};
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i += 2) {
  if (!argv[i].startsWith("--") || argv[i + 1] === undefined) throw new Error(USAGE);
  args[argv[i].slice(2)] = path.resolve(argv[i + 1]);
}
for (const key of ["upstream", "harness", "out"]) if (!args[key]) throw new Error(USAGE);

const here = path.dirname(fileURLToPath(import.meta.url));
const esbuild = createRequire(path.join(args.harness, "package.json"))("esbuild");

await esbuild.build({
  entryPoints: [path.join(here, "hostMain.ts")],
  outfile: path.join(args.out, "hostMain.mjs"),
  bundle: true,
  format: "esm",
  platform: "node",
  target: "esnext",
  logLevel: "warning",
  define: { __UPSTREAM_DIR__: JSON.stringify(args.upstream) },
  banner: { js: 'import { createRequire as __oracleCreateRequire } from "node:module"; const require = __oracleCreateRequire(import.meta.url);' },
  external: ["electron"]
});
