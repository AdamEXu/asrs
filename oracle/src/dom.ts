import { UPSTREAM_DIR } from "./shims";
import { readFixture } from "./dom/fixtureFile";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// dom [<spec or fixture file>]
//   Loads upstream's www/hub.html in jsdom, evaluates the hub (src/hub/hub.ts with
//   src/dom/env.ts installed first) inside that window and runs one fixture. Without an
//   argument the fixture is the "fixture" key of the spec named by $ASRS_ORACLE_SPEC, which
//   `oracle gen` sets. See oracle/README.md, "DOM harness". domBlink.ts runs the same page
//   in Blink.

declare const __HARNESS_DIR__: string;

const { fixture } = readFixture("dom", process.argv[2]);
if (fixture.geometry !== undefined && fixture.geometry !== "scripted") {
  process.stderr.write(`dom: jsdom has no layout; geometry ${JSON.stringify(fixture.geometry)} needs the blink backend\n`);
  process.exit(2);
}

const { JSDOM, VirtualConsole } = createRequire(path.join(__HARNESS_DIR__, "package.json"))("jsdom");
const log = (text: string) => process.stderr.write(text + "\n");
const virtualConsole = new VirtualConsole();
for (const level of ["log", "info", "warn", "error", "debug"]) {
  virtualConsole.on(level, (...args: unknown[]) => log(`page ${level}: ${args.map(String).join(" ")}`));
}
const lines: string[] = [];
virtualConsole.on("jsdomError", (error: Error & { type?: string; cause?: unknown }) => {
  log(`jsdom: ${error.message}`);
  if (error.type === "not-implemented") return;
  const cause = (error.cause ?? error) as Error;
  lines.push(JSON.stringify({ exception: "event", error: `${cause?.name}: ${cause?.message}` }));
});

const hubHtml = path.join(UPSTREAM_DIR, "www", "hub.html");
const dom = new JSDOM(fs.readFileSync(hubHtml, "utf8"), {
  url: pathToFileURL(hubHtml).href,
  runScripts: "outside-only",
  virtualConsole
});
const page = dom.window;
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
