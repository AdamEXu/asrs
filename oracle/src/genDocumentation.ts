import DocumentationRenderer from "@upstream/shared/renderers/DocumentationRenderer";
import { UPSTREAM_DIR } from "./shims";
import { toTree } from "./cve1";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Ground truth for the Documentation tab: upstream's DocumentationRenderer, unmodified, on the
// #renderer0 template of the real www/hub.html in an Electron window, its iframe loading the
// documentation `cargo xtask docs` built. Each case is a fresh page and one record of what
// saveState() returned: right after each operation (`immediate`) and once the page inside the
// frame has stopped changing its hash (`settled`).
//
//   genDocumentation hash
//
// This file runs twice: as the generator, and as the main process of the Electron it spawns.

const STATES: unknown[] = [
  "#/",
  "",
  "#",
  "#/tab-reference/line-graph",
  "#/tab-reference/line-graph/",
  "#/tab-reference/line-graph#adding-fields",
  "#/tab-reference/console?field=a&filter=!b#usage",
  "/tab-reference/2d-field",
  "tab-reference/2d-field",
  "#tab-reference/2d-field",
  "#!/tab-reference/2d-field",
  "##/whats-new",
  "#/overview/navigation/keyboard",
  "#/no/such/page",
  "#/a b",
  "#/éü漢",
  "#/📖",
  "#/\ud800x",
  "#/%7Efoo%2",
  "#/\"<>`{}|^\\[]'",
  "#/a\nb\tc\rd",
  "  #/whats-new  ",
  "#/\u0000\u0001\u001f\u007f\u0080"
];
const IGNORED: unknown[] = [null, 1, true, ["#/whats-new"], { hash: "#/whats-new" }];

type Op = { op: "restore"; state: unknown } | { op: "render" | "load" | "settle" | "save" } | { op: "click"; selector: string };
const restore = (state: unknown): Op => ({ op: "restore", state });
const [render, load, settle, save] = (["render", "load", "settle", "save"] as const).map((op) => ({ op }));
const opened: Op[] = [render, load, render, settle];

const CASES: { name: string; ops: Op[] }[] = [
  { name: "fresh", ops: [save, render, save, load, render, settle] },
  { name: "unopened", ops: [restore("#/whats-new"), save, restore("#/legal/support"), save, render, save, load, save, render, settle] },
  { name: "ignored", ops: [...opened, restore("#/whats-new"), settle, ...IGNORED.map(restore), settle] },
  { name: "ignored-unopened", ops: [...IGNORED.map(restore), ...opened] },
  { name: "links", ops: [...opened, { op: "click", selector: 'a.menu__link[href="#/tab-reference/2d-field"]' }, settle, restore("#/whats-new"), settle, { op: "click", selector: "a.navbar__brand" }, settle] },
  ...STATES.map((state, i) => ({ name: `queued-${i}`, ops: [restore(state), ...opened] })),
  { name: "sequence", ops: [...opened, ...STATES.flatMap((state) => [restore(state), settle])] }
];

const PAGE = (renderer: string) => `
  window.DocumentationRenderer = (${renderer});
  document.getElementById("rendererTemplates").hidden = false;
  window.frame = document.querySelector("#renderer0 iframe");
  window.loads = 0;
  window.frame.addEventListener("load", () => window.loads++);
  window.renderer = new window.DocumentationRenderer(document.getElementById("renderer0"));
  window.until = async (test) => {
    const start = performance.now();
    while (!test()) {
      if (performance.now() - start > 20000) throw new Error("timed out: " + test);
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
  };
  window.ops = {
    save: () => {},
    restore: (op) => window.renderer.restoreState(op.state),
    render: () => window.renderer.render(null),
    load: () => window.until(() => window.renderer.loaded),
    click: async (op) => {
      let link;
      await window.until(() => (link = window.frame.contentDocument.querySelector(op.selector)));
      link.click();
    },
    settle: async () => {
      let last = window.renderer.saveState();
      let since = performance.now();
      await window.until(() => {
        const now = window.renderer.saveState();
        if (now !== last || window.frame.contentDocument.querySelector("main") === null) {
          last = now;
          since = performance.now();
        }
        return performance.now() - since > 250;
      });
    }
  };
  window.run = async (ops) => {
    const saved = [];
    for (const op of ops) {
      await window.ops[op.op](op);
      saved.push(window.renderer.saveState());
    }
    return saved;
  };
  true;
`;

async function electronMain() {
  const { app, BrowserWindow, net, protocol } = await import("electron");
  const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));
  const real = fs.realpathSync(request.upstream);
  const hubHtml = path.join(request.upstream, "www", "hub.html");
  const served = path.join(request.upstream, "docs", "build");
  const never = () => new Response(new ReadableStream({ start() {} }));
  app.setPath("userData", request.userData);
  app.on("window-all-closed", () => {});
  app.dock?.hide();
  await app.whenReady();
  protocol.handle("file", (req) => {
    const file = fileURLToPath(req.url.split(/[?#]/)[0]);
    for (const base of [request.upstream, real]) {
      if (file === path.join(base, "bundles", "hub.js")) return new Response("", { headers: { "content-type": "text/javascript" } });
      const docs = path.join(base, "docs", "build") + path.sep;
      if (file.startsWith(docs)) {
        const target = path.join(request.docs, file.slice(docs.length));
        return net.fetch(new URL(`file://${target}`).href, { bypassCustomProtocolHandlers: true });
      }
    }
    if (file.endsWith("hub.html") || file.endsWith(".css")) return net.fetch(req, { bypassCustomProtocolHandlers: true });
    return never();
  });
  protocol.handle("http", never);
  protocol.handle("https", never);
  if (fs.existsSync(served)) throw new Error(`${served} exists; the generator serves its own`);

  const results: unknown[] = [];
  for (const item of CASES) {
    const win = new BrowserWindow({
      width: 1100,
      height: 650,
      useContentSize: true,
      show: false,
      webPreferences: { offscreen: true, backgroundThrottling: false, spellcheck: false, webviewTag: true }
    });
    const ready = new Promise<void>((resolve) => win.webContents.once("dom-ready", () => resolve()));
    win.loadFile(hubHtml).catch(() => {});
    await ready;
    win.webContents.on("console-message", (event) => process.stderr.write(`page ${event.level}: ${event.message}\n`));
    win.webContents.debugger.attach("1.3");
    const evaluate = async (expression: string): Promise<any> => {
      const result = await win.webContents.debugger.sendCommand("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (result.exceptionDetails !== undefined) {
        throw new Error(`${item.name}: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
      }
      return result.result.value;
    };
    await evaluate(PAGE(request.renderer));
    const saved: unknown[] = await evaluate(`window.run(${JSON.stringify(item.ops)}).then((saved) => JSON.stringify(saved))`).then(JSON.parse);
    if (saved.length !== item.ops.length) throw new Error(`${item.name}: ${saved.length} results`);
    results.push({ name: item.name, steps: item.ops.map((op, i) => ({ ...op, saved: saved[i] })) });
    win.destroy();
  }
  fs.writeFileSync(request.out, JSON.stringify(results));
  app.exit(0);
}

function generate() {
  if (process.argv[2] !== "hash") {
    console.error("usage: genDocumentation hash");
    process.exit(2);
  }
  const docs = path.join(UPSTREAM_DIR, "../../docs/build");
  if (!fs.existsSync(path.join(docs, "index.html"))) {
    console.error("genDocumentation: the documentation is not built; run `cargo xtask docs`");
    process.exit(1);
  }
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-documentation-"));
  let results: { name: string }[];
  try {
    const requestPath = path.join(scratch, "request.json");
    const out = path.join(scratch, "out.json");
    fs.writeFileSync(
      requestPath,
      JSON.stringify({
        upstream: UPSTREAM_DIR,
        docs: fs.realpathSync(docs),
        renderer: DocumentationRenderer.toString(),
        userData: path.join(scratch, "userData"),
        out
      })
    );
    const env = { ...process.env };
    delete env.ELECTRON_RUN_AS_NODE;
    env.ELECTRON_DISABLE_SECURITY_WARNINGS = "true";
    const run = spawnSync(process.execPath, ["--lang=en-US", "--log-level=3", fileURLToPath(import.meta.url), requestPath], {
      env,
      stdio: ["ignore", "inherit", "inherit"]
    });
    if (run.status !== 0) {
      console.error(`genDocumentation: Electron exited with ${run.status ?? run.signal}`);
      process.exit(1);
    }
    results = JSON.parse(fs.readFileSync(out, "utf8"));
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
  for (const result of results) process.stdout.write(JSON.stringify(toTree(result)) + "\n");
}

if ((process as { type?: string }).type === "browser") {
  electronMain().catch((error) => {
    process.stderr.write(`genDocumentation: ${error?.stack ?? error}\n`);
    process.exit(1);
  });
} else {
  generate();
}
