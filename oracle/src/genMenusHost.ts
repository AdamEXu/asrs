import "./environment";
import { UPSTREAM_DIR } from "./shims";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { zstdDecompressSync } from "node:zlib";

// genMenusHost <win32|linux>
//   `genMenus electron` on the platform it runs on, whose role table Electron's real
//   Menu.buildFromTemplate uses: the `fresh` template of the committed menus/template-<platform>,
//   which genMenus records on one platform for all (build.mjs's fakes do not resolve on Windows),
//   handed to genMenusMain.mjs. Its vectors name that platform in `platforms`.

const platform = process.argv[2];
if (platform !== process.platform) {
  process.stderr.write(`genMenusHost: ${platform} needs its own host, this is ${process.platform}\n`);
  process.exit(2);
}

const CLICK = "__genMenusClick";
const here = path.dirname(fileURLToPath(import.meta.url));

function fail(message: string): never {
  process.stderr.write(`genMenusHost: ${message}\n`);
  process.exit(1);
}

function run(args: string[], env: NodeJS.ProcessEnv): string {
  const child = spawnSync(process.execPath, args, { env, encoding: "utf8", maxBuffer: 1 << 26, stdio: ["ignore", "pipe", "inherit"] });
  if (child.status !== 0) fail(`${args.join(" ")} exited with ${child.status ?? child.signal}`);
  return child.stdout;
}

const plain = (value: any): any => {
  if (Array.isArray(value)) return value.map(plain);
  if (value === null || typeof value !== "object") return value;
  if ("fn" in value) return { [CLICK]: value.fn };
  if ("u" in value) return undefined;
  if (!("o" in value)) fail(`unexpected value ${JSON.stringify(value)}`);
  return Object.fromEntries(value.o.map(([key, v]: [string, any]) => [key, plain(v)]).filter(([, v]: [string, any]) => v !== undefined));
};

const vector = path.join(UPSTREAM_DIR, "../../..", `parity/vectors/menus/template-${platform}.jsonl.zst`);
const lines = zstdDecompressSync(fs.readFileSync(vector)).toString("utf8").trim().split("\n");
const first = lines.map((line) => JSON.parse(line)).find((record) => record.op === "template");
const upstreamPackage = JSON.parse(fs.readFileSync(path.join(UPSTREAM_DIR, "package.json"), "utf8"));
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-gen-menus-"));
let built: string;
try {
  const requestPath = path.join(scratch, "request.json");
  fs.writeFileSync(
    requestPath,
    JSON.stringify({ appName: upstreamPackage.productName, click: CLICK, template: plain(first.template), userData: path.join(scratch, "userData") })
  );
  const env = { ...process.env, ELECTRON_DISABLE_SECURITY_WARNINGS: "true" };
  delete env.ELECTRON_RUN_AS_NODE;
  built = run(["--lang=en-US", "--log-level=3", path.join(here, "genMenusMain.mjs"), requestPath], env);
} finally {
  fs.rmSync(scratch, { recursive: true, force: true });
}
// Electron's main process starts its output with an empty line on Windows.
const records = built.split("\n").filter((line) => line.trim() !== "");
process.stdout.write([JSON.stringify({ op: "config", platform, scenario: "electron" }), ...records].join("\n") + "\n", () => process.exit(0));
