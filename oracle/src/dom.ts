import { UPSTREAM_DIR } from "./shims";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// dom [<spec or fixture file>]
//   Loads upstream's www/hub.html in jsdom, evaluates the hub (src/hub/hub.ts with
//   src/dom/env.ts installed first) inside that window and runs one fixture. Without an
//   argument the fixture is the "fixture" key of the spec named by $ASRS_ORACLE_SPEC, which
//   `oracle gen` sets. See oracle/README.md, "DOM harness".

declare const __HARNESS_DIR__: string;

const file = process.argv[2] ?? process.env.ASRS_ORACLE_SPEC;
if (file === undefined) {
  process.stderr.write("usage: dom <spec or fixture file>\n");
  process.exit(2);
}
const parsed = JSON.parse(fs.readFileSync(file, "utf8"));
const isSpec = "fixture" in parsed && "entry" in parsed;
const fixture = isSpec ? parsed.fixture : parsed;

// "assets", "preferences", "state" and "typeMemory", and the operations of a "log" step, may
// name a JSON file relative to the fixture instead. Under a spec every such file must be listed
// in "inputs", so the vector's generator hash covers it.
const declared: string[] = isSpec ? (parsed.inputs ?? []).map((p: string) => path.resolve(path.dirname(file), p)) : [];
function load(holder: any, key: string) {
  if (typeof holder?.[key] !== "string") return;
  const target = path.resolve(path.dirname(file!), holder[key]);
  if (isSpec && !declared.includes(target)) {
    process.stderr.write(`dom: ${file}: ${holder[key]} is not listed in "inputs"\n`);
    process.exit(2);
  }
  holder[key] = JSON.parse(fs.readFileSync(target, "utf8"));
}
for (const key of ["assets", "preferences", "state", "typeMemory"]) load(fixture, key);
for (const step of fixture.steps ?? []) load(step, "log");

const { JSDOM, VirtualConsole } = createRequire(path.join(__HARNESS_DIR__, "package.json"))("jsdom");
const log = (text: string) => process.stderr.write(text + "\n");
const virtualConsole = new VirtualConsole();
for (const level of ["log", "info", "warn", "error", "debug"]) {
  virtualConsole.on(level, (...args: unknown[]) => log(`page ${level}: ${args.map(String).join(" ")}`));
}
virtualConsole.on("jsdomError", (error: Error) => log(`jsdom: ${error.message}`));

const hubHtml = path.join(UPSTREAM_DIR, "www", "hub.html");
const dom = new JSDOM(fs.readFileSync(hubHtml, "utf8"), {
  url: pathToFileURL(hubHtml).href,
  runScripts: "outside-only",
  virtualConsole
});
const page = dom.window;
const lines: string[] = [];
page.__oracleFixture = page.JSON.parse(JSON.stringify(fixture));
page.__oracleYield = () => new Promise((resolve) => setImmediate(resolve));
page.__oracleWrite = (line: string) => lines.push(line);
page.__oracleLog = log;
page.__oracleOuter = { TextEncoder, TextDecoder };
process.on("unhandledRejection", (reason) => {
  const error = reason as Error;
  log(`unhandled rejection: ${error?.stack ?? String(reason)}`);
  lines.push(JSON.stringify({ exception: "unhandledRejection", error: `${error?.name}: ${error?.message}` }));
});

const here = path.dirname(fileURLToPath(import.meta.url));
page.eval(fs.readFileSync(path.join(here, "domPage.js"), "utf8"));
await page.__oracleRun();
process.stdout.write(
  lines.map((line) => line + "\n").join(""),
  () => process.exit(0)
);
