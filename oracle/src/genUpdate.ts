import "./environment";
import { toTree } from "./cve1";
import { Platform, buildCheckerBundles, buildLicensesBundle } from "./update/bundle";
import {
  CheckerCase,
  Fetch,
  LicenseCase,
  dateInputs,
  fuzzDateInputs,
  handCheckerCases,
  handLicenseCases,
  randomCheckerCases,
  randomLicenseCases
} from "./update/cases";

// Ground truth for P07: upstream's UpdateChecker.ts (with its real betaUtil, github,
// ElectronConstants and shared/util), main.ts's About, beta and update flows, `new Date(value)`
// as it parses `published_at`, and getLicenses.mjs, all unmodified, against scripted electron,
// network and filesystem fakes.
//
//   genUpdate checker <count> <seed>
//     The hand cases in update/cases.ts, then <count> generated ones. Records:
//       {case, name, config: {platform, arch, translation, packaged, version, beta, repository}}
//       {case, step, op: "check", fetch, rejected, effects}    fetch: {reject} or {status, body}
//       {case, step, op: "shouldPrompt", result}
//       {case, step, op: "prompt", response, effects}
//     Effects, in call order: {k: "fetch", url, init}, {k: "box", parent, options} (`icon` is
//     the string "WINDOW_ICON" when it is ElectronConstants' WINDOW_ICON), {k: "open", url}.
//   genUpdate dates <count> <seed>
//     {case, input, time}: `new Date(input).getTime()` for the hand list and <count> generated
//     strings, in the zone the vector names.
//   genUpdate datefuzz <count> <seed>
//     The same records for <count> strings from the independent fuzz generator.
//   genUpdate dialogs
//     main.ts's About, beta, feedback and update flows (see update/dialogs.ts).
//   genUpdate licenses <count> <seed>
//     getLicenses.mjs run in a fake working directory: {case, name, files, effects} with
//     effects {k: "fetch", url, reply}, {k: "error" | "log", text}, {k: "write", path, data},
//     and `threw` (`String(error)`) when the script throws.

const [mode, countArg, seedArg] = process.argv.slice(2);
const count = Number(countArg) || 0;
const seed = Number(seedArg) >>> 0;
const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");
const settle = async () => {
  for (let i = 0; i < 20; i++) await new Promise((resolve) => setImmediate(resolve));
};

let failure: unknown = null;
process.on("unhandledRejection", (reason) => {
  failure = reason;
});
const assertNoFailure = (ctx: string) => {
  if (failure !== null) throw new Error(`${ctx}: unhandled rejection ${String(failure)}`);
};

const silentConsole = new Proxy({}, { get: () => () => undefined });

function setProcess(platform: string, arch: string) {
  Object.defineProperty(process, "platform", { value: platform, configurable: true });
  Object.defineProperty(process, "arch", { value: arch, configurable: true });
}

function describeFetch(fetch: Fetch): unknown {
  return "reject" in fetch ? { reject: true } : { status: fetch.status, body: toTree(fetch.body) };
}

class CheckerWorld {
  effects: unknown[] = [];
  fetch: Fetch | null = null;
  response = 0;
  windowIcon: unknown = undefined;
  glue: any;

  constructor(c: CheckerCase) {
    const world = this;
    this.glue = {
      beta: c.beta,
      repository: c.repository,
      console: silentConsole,
      fs: {},
      electron: {
        app: {
          isPackaged: c.packaged,
          runningUnderARM64Translation: c.translation,
          getVersion: () => c.version,
          getPath: (name: string) => "/fake/" + name
        },
        dialog: {
          showMessageBox(...args: any[]) {
            const [parent, options] = args.length >= 2 ? [args[0], args[1]] : [null, args[0]];
            const described: any = {};
            for (const key of Object.keys(options)) {
              described[key] = key === "icon" && options.icon === world.windowIcon ? "WINDOW_ICON" : options[key];
            }
            world.effects.push({ k: "box", parent: parent === null ? null : toTree(parent), options: toTree(described) });
            return Promise.resolve({ response: world.response, checkboxChecked: false });
          }
        },
        net: {
          fetch(url: unknown, init: unknown) {
            world.effects.push({ k: "fetch", url: toTree(url), init: toTree(init) });
            const scripted = world.fetch;
            if (scripted === null) throw new Error("fetch without a scripted response");
            if ("reject" in scripted) return Promise.reject(new TypeError("fetch failed"));
            return Promise.resolve(new Response(scripted.body, { status: scripted.status }));
          }
        },
        shell: {
          openExternal(url: unknown) {
            world.effects.push({ k: "open", url: toTree(url) });
            return Promise.resolve();
          }
        }
      }
    };
  }

  take(): unknown[] {
    return this.effects.splice(0, this.effects.length);
  }
}

async function checkerMode() {
  const bundles = await buildCheckerBundles();
  const cases = [...handCheckerCases(), ...randomCheckerCases(count, seed)];
  let instance = 0;
  for (let index = 0; index < cases.length; index++) {
    const c = cases[index];
    const world = new CheckerWorld(c);
    (globalThis as any).__asrsUpdate = world.glue;
    setProcess(c.platform, c.arch);
    const mod = await import(bundles[c.platform as Platform] + "?instance=" + instance++);
    world.windowIcon = mod.Constants.WINDOW_ICON;
    const checker = new mod.UpdateChecker();
    out({
      case: index,
      name: c.name,
      config: {
        platform: c.platform,
        arch: c.arch,
        translation: c.translation,
        packaged: c.packaged,
        version: c.version,
        beta: c.beta,
        repository: mod.GITHUB_REPOSITORY
      }
    });
    let step = 0;
    for (const op of c.ops) {
      if ("check" in op) {
        world.fetch = op.check;
        const rejected = await checker.check().then(
          () => false,
          () => true
        );
        await settle();
        assertNoFailure(c.name);
        out({ case: index, step: step++, op: "check", fetch: describeFetch(op.check), rejected, effects: world.take() });
      } else if ("shouldPrompt" in op) {
        out({ case: index, step: step++, op: "shouldPrompt", result: checker.getShouldPrompt() });
      } else {
        world.response = op.prompt;
        await checker.showPrompt();
        await settle();
        assertNoFailure(c.name);
        out({ case: index, step: step++, op: "prompt", response: op.prompt, effects: world.take() });
      }
    }
    if (world.effects.length > 0) throw new Error(`${c.name}: effects outside an op`);
  }
}

function datesMode(inputs: unknown[]) {
  inputs.forEach((input, index) => {
    out({ case: index, input: toTree(input), time: toTree(new Date(input as any).getTime()) });
  });
}

class LicenseWorld {
  effects: unknown[] = [];
  files = new Map<string, Uint8Array | null>();
  replies: { [id: string]: any };

  constructor(c: LicenseCase) {
    for (const entry of c.files) this.files.set(entry.path, entry.kind === "dir" ? null : entry.data!);
    this.replies = c.spdx;
  }

  private norm(p: string): string {
    const parts: string[] = [];
    for (const part of p.split("/")) {
      if (part === "" || part === ".") continue;
      parts.push(part);
    }
    return parts.join("/");
  }

  private enoent(syscall: string, p: string): Error {
    const e: any = new Error(`ENOENT: no such file or directory, ${syscall} '${p}'`);
    e.code = "ENOENT";
    return e;
  }

  get glue() {
    const world = this;
    const isDir = (p: string) => p === "" || world.files.get(p) === null;
    return {
      console: {
        error: (...args: unknown[]) => world.effects.push({ k: "error", text: args.map(String).join(" ") }),
        log: (...args: unknown[]) => world.effects.push({ k: "log", text: args.map(String).join(" ") })
      },
      fetch: async (url: string) => {
        const id = decodeURIComponent(url.slice(url.lastIndexOf("/") + 1).replace(/\.json$/, ""));
        const reply = world.replies[id] ?? { status: 404, body: new Uint8Array(Buffer.from("404: Not Found")) };
        world.effects.push({ k: "fetch", url, reply: "reject" in reply ? { reject: true } : { status: reply.status, body: toTree(reply.body) } });
        if ("reject" in reply) throw new TypeError("fetch failed");
        return new Response(reply.body, { status: reply.status });
      },
      fs: {
        existsSync: (p: string) => world.files.has(world.norm(p)) || world.norm(p) === "",
        readdirSync: (p: string) => {
          const base = world.norm(p);
          if (!isDir(base)) throw world.enoent("scandir", p);
          const prefix = base === "" ? "" : base + "/";
          const names: string[] = [];
          for (const key of world.files.keys()) {
            if (key.startsWith(prefix) && key.length > prefix.length && !key.slice(prefix.length).includes("/")) names.push(key.slice(prefix.length));
          }
          return names;
        },
        statSync: (p: string) => {
          const key = world.norm(p);
          if (!world.files.has(key) && key !== "") throw world.enoent("stat", p);
          return { isDirectory: () => isDir(key) };
        },
        readFileSync: (p: string, options?: unknown) => {
          const key = world.norm(p);
          const data = world.files.get(key);
          if (data === undefined) throw world.enoent("open", p);
          if (data === null) {
            const e: any = new Error("EISDIR: illegal operation on a directory, read");
            e.code = "EISDIR";
            throw e;
          }
          const buffer = Buffer.from(data);
          return options === undefined ? buffer : buffer.toString(options as BufferEncoding);
        },
        writeFileSync: (p: string, data: unknown) => {
          world.effects.push({ k: "write", path: p, data: toTree(data) });
        }
      }
    };
  }
}

async function licensesMode() {
  const bundle = await buildLicensesBundle();
  const cases = [...handLicenseCases(), ...randomLicenseCases(count, seed)];
  for (let index = 0; index < cases.length; index++) {
    const c = cases[index];
    const world = new LicenseWorld(c);
    (globalThis as any).__asrsLicenses = world.glue;
    let threw: string | undefined;
    try {
      await import(bundle + "?instance=" + index);
    } catch (error) {
      threw = String(error);
    }
    await settle();
    assertNoFailure(c.name);
    out({
      case: index,
      name: c.name,
      files: c.files.map((f) => (f.kind === "dir" ? { dir: f.path } : { file: f.path, data: toTree(f.data) })),
      effects: world.effects,
      ...(threw === undefined ? {} : { threw })
    });
  }
}

if (mode === "checker") await checkerMode();
else if (mode === "dates") datesMode(dateInputs(count, seed));
else if (mode === "datefuzz") datesMode(fuzzDateInputs(count, seed));
else if (mode === "licenses") await licensesMode();
else if (mode === "dialogs") await (await import("./update/dialogs")).run();
else throw new Error(`genUpdate: unknown mode ${mode}`);
