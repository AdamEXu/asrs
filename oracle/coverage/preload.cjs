"use strict";
// Loaded by NODE_OPTIONS=--require into every process and worker thread of an
// `oracle coverage` run: the entry under ELECTRON_RUN_AS_NODE, its worker threads, and the real
// Electron processes it spawns. Writes V8 precise coverage to $ASRS_COVERAGE_OUT and changes
// nothing the entry computes, so a harness source never knows about coverage and no vector's
// generator hash depends on it.
const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");
const { Session } = require("node:inspector");
const workers = require("node:worker_threads");

const out = process.env.ASRS_COVERAGE_OUT;
if (out) install();

function install() {
  const { appendFileSync, writeFileSync, mkdirSync, copyFileSync } = fs;
  const exit = process.exit;
  let serial = 0;
  const raw = path.join(out, `${process.pid}-${workers.threadId}.jsonl`);
  const save = (result) => {
    const scripts = result.filter((s) => s.url.startsWith("file:"));
    if (scripts.length > 0) appendFileSync(raw, JSON.stringify(scripts) + "\n");
  };
  const fail = (what, error) => {
    process.stderr.write(`coverage: ${what}: ${error?.stack ?? error?.message ?? error}\n`);
    exit.call(process, 97);
  };

  const session = new Session();
  session.connect();
  const post = (method, params) => {
    let result;
    session.post(method, params, (error, value) => {
      if (error) fail(method, error);
      result = value;
    });
    return result;
  };
  post("Profiler.enable");
  post("Profiler.startPreciseCoverage", { callCount: true, detailed: true });
  const take = () => save(post("Profiler.takePreciseCoverage").result);

  if (workers.isMainThread) {
  } else {
    // A terminated worker runs no exit handler, so take coverage before each message leaves it.
    const port = workers.parentPort;
    const postMessage = port.postMessage;
    port.postMessage = function (...args) {
      take();
      return postMessage.apply(this, args);
    };
  }
  process.on("exit", take);

  const load = Module._load;
  let hooked = false;
  Module._load = function (request, ...rest) {
    const exported = load.call(this, request, ...rest);
    if (request === "esbuild") return withSourceMaps(exported);
    if (request === "electron" && process.type === "browser" && !hooked && exported.app) {
      hooked = true;
      hookBrowser(exported.app);
    }
    return exported;
  };
  // An ESM `import ... from "electron"` in the main process does not pass through
  // Module._load, so load it from here as soon as Electron's own startup has registered it.
  if (process.versions.electron && process.type === "browser") {
    setImmediate(() => {
      try {
        require("electron");
      } catch (error) {
        fail("loading electron", error);
      }
    });
  }

  // Entries that bundle upstream at run time write to a directory they delete at exit.
  function withSourceMaps(esbuild) {
    return {
      ...esbuild,
      build: async (options) => {
        const result = await esbuild.build({ ...options, sourcemap: "linked" });
        if (options.outfile) {
          const dir = path.join(out, "bundles");
          mkdirSync(dir, { recursive: true });
          const real = fs.realpathSync(options.outfile);
          const copy = path.join(dir, `${process.pid}-${serial++}`);
          copyFileSync(real, `${copy}.js`);
          const map = JSON.parse(fs.readFileSync(`${real}.map`, "utf8"));
          // esbuild writes them relative to the directory as it was named, symlinks and all.
          map.sources = map.sources.map((s) => path.resolve(path.dirname(options.outfile), s));
          writeFileSync(`${copy}.map`, JSON.stringify(map));
          writeFileSync(`${copy}.url`, JSON.stringify([options.outfile, real]));
        }
        return result;
      }
    };
  }

  function hookBrowser(app) {
    const attached = [];
    app.on("web-contents-created", (_event, contents) => {
      const dbg = contents.debugger;
      const sendCommand = dbg.sendCommand;
      const send = (method, params) => sendCommand.call(dbg, method, params);
      const attach = dbg.attach;
      try {
        attach.call(dbg, "1.3");
      } catch (error) {
        fail("attaching the debugger", error);
      }
      // The harness attaches the same debugger itself after dom-ready.
      dbg.attach = () => {};
      // Sent before the first load and not awaited: a session holds its messages until a renderer
      // exists, so waiting for the reply here would block the load that creates it.
      const started = Promise.all([
        send("Profiler.enable"),
        send("Profiler.startPreciseCoverage", { callCount: true, detailed: true })
      ]).catch((error) => fail("starting renderer coverage", error));
      // A window may be destroyed as soon as its last command answers.
      const entry = { contents, taken: false };
      entry.take = async () => {
        await started;
        save((await send("Profiler.takePreciseCoverage")).result);
        entry.taken = true;
      };
      attached.push(entry);
      const then = (target, method) => {
        const original = target[method];
        target[method] = async (...args) => {
          const result = await original.apply(target, args);
          await entry.take();
          return result;
        };
      };
      then(contents, "executeJavaScript");
      then(dbg, "sendCommand");
    });
    const appExit = app.exit;
    app.exit = function (code) {
      (async () => {
        for (const entry of attached) {
          try {
            if (!entry.contents.isDestroyed()) await entry.take();
          } catch (error) {
            if (!entry.taken) throw error;
          }
        }
      })().then(
        () => appExit.call(app, code),
        (error) => fail("taking renderer coverage", error)
      );
    };
  }
}
