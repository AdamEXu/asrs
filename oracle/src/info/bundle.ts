import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";

declare const __UPSTREAM_DIR__: string;
declare const __HARNESS_DIR__: string;

// upstream's src/licenses.ts with the one import that is data, licenses.json, read from the page
// (`globalThis.__asrsLicenses`), so a case can give it a list. buildConstants has the FRC 6328
// build's distribution, as rolldown.config.mjs gives it.
export async function buildLicensesPage(): Promise<string> {
  const upstream = fs.realpathSync(__UPSTREAM_DIR__);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-info-"));
  process.on("exit", () => fs.rmSync(dir, { recursive: true, force: true }));
  const outfile = path.join(dir, "licenses.js");
  const esbuild = createRequire(path.join(__HARNESS_DIR__, "package.json"))("esbuild");
  await esbuild.build({
    entryPoints: [path.join(upstream, "src/licenses.ts")],
    outfile,
    bundle: true,
    format: "esm",
    platform: "browser",
    target: "esnext",
    logLevel: "warning",
    plugins: [
      {
        name: "asrs-info",
        setup(build: any) {
          build.onResolve({ filter: /^\.\/licenses\.json$/ }, () => ({ path: "licenses", namespace: "asrs-info" }));
          build.onLoad({ filter: /.*/, namespace: "asrs-info" }, () => ({ contents: "export default globalThis.__asrsLicenses;", loader: "js" }));
          build.onLoad({ filter: /buildConstants\.ts$/ }, (args: any) => ({
            contents: fs.readFileSync(args.path, "utf8").replace("Distribution.__distribution__", "Distribution.FRC6328"),
            loader: "ts"
          }));
        }
      }
    ]
  });
  return outfile;
}
