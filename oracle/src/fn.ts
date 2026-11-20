import "./shims";
import { encode, fromTree } from "./cve1";
import { invoke, listModules } from "./registry";

const USAGE = `usage: fn [--list]

Reads JSONL requests on stdin, one per line:
  {"fn": "<name>", "args": [<CVE-1 value>, ...]}
and writes one JSONL response per request, in order:
  {"ok":<CVE-1 value>}      the call returned (promises are awaited)
  {"throw":<CVE-1 value>}   the call threw this value
  {"error":"<message>"}     the request itself was invalid

Names:
  Math.sin                      global path, called with its parent as this
  Number.prototype.toFixed      prototype method, args[0] is the receiver
  new Date                      "new " prefix constructs
  shared/util:cleanFloat        upstream module (path under src/, no extension)
  shared/units:Units.convertWithPreset
  shared/log/Log:default        default export
  npm:color-convert:hex.hsl     npm package from upstream's node_modules
  oracle:identity               harness helper (see --list)

--list prints the available upstream modules, npm packages and helpers.`;

function respond(value: unknown, thrown: boolean): string {
  return (thrown ? '{"throw":' : '{"ok":') + encode(value) + "}";
}

async function handle(line: string): Promise<string> {
  let name: string;
  let args: unknown[];
  try {
    const request = JSON.parse(line);
    if (request === null || typeof request !== "object" || typeof request.fn !== "string") {
      throw new Error('request must be an object with a string "fn"');
    }
    const rawArgs = request.args ?? [];
    if (!Array.isArray(rawArgs)) throw new Error('"args" must be an array');
    name = request.fn;
    args = rawArgs.map(fromTree);
  } catch (error) {
    return JSON.stringify({ error: String((error as Error)?.message ?? error) });
  }
  let result: unknown;
  try {
    result = await invoke(name, args);
  } catch (error) {
    try {
      return respond(error, true);
    } catch (encodeError) {
      return JSON.stringify({ error: String((encodeError as Error).message) });
    }
  }
  try {
    return respond(result, false);
  } catch (encodeError) {
    return JSON.stringify({ error: String((encodeError as Error).message) });
  }
}

async function main() {
  const argv = process.argv.slice(2);
  if (argv.includes("--help") || argv.includes("-h")) {
    process.stdout.write(USAGE + "\n");
    return;
  }
  if (argv.includes("--list")) {
    process.stdout.write(JSON.stringify(listModules(), null, 2) + "\n");
    return;
  }

  let buffered: string[] = [];
  let flushScheduled = false;
  const flush = () => {
    flushScheduled = false;
    if (buffered.length === 0) return;
    process.stdout.write(buffered.join(""));
    buffered = [];
  };

  const processLine = async (line: string) => {
    if (line.endsWith("\r")) line = line.slice(0, -1);
    if (line.trim() === "") return;
    const response = await handle(line);
    buffered.push(response + "\n");
    if (!flushScheduled) {
      flushScheduled = true;
      setImmediate(flush);
    }
  };

  const decoder = new TextDecoder();
  let pending = "";
  for await (const chunk of process.stdin) {
    pending += decoder.decode(chunk as Uint8Array, { stream: true });
    let newline: number;
    while ((newline = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, newline);
      pending = pending.slice(newline + 1);
      await processLine(line);
    }
  }
  pending += decoder.decode();
  await processLine(pending);
  flush();
}

main().then(
  () => process.stdout.write("", () => process.exit(0)),
  (error) => {
    console.error(error);
    process.exit(2);
  }
);
