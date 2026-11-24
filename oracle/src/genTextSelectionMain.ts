import { app, BrowserWindow, clipboard } from "electron";
import fs from "node:fs";

// Electron main process for `genTextSelection` (not run under ELECTRON_RUN_AS_NODE). Each of the
// fixture's pages has its `html`, where `data-b` elements are the blocks of selectable text in
// document order, `data-u="n"` elements, which cannot be selected, come before block n, and
// `data-t="b a"` tables have the empty blocks b and a for the positions before and after them. For
// each of its sequences it runs operations a seeded generator picks, at points in the text:
//   ["press", target, clickCount, shift], ["move", target], ["release", target],
//   ["menu", target], a press and release of the right button, and ["all"], Select All.
// A target is ["char", block, offset, side, wraps, link], a quarter into or before the end of the
// character there, whether the line wraps at that position and the range of the link it is in,
// ["after", block], right of the block's last line, ["cell", block], an empty block's
// middle, or ["other", n], the middle of element `data-u="n"`.
// After each it writes {"page", "sequence", "op", "ends", "copied"}: the selection's anchor and
// focus as [block, offset in the block's text] or null, and what a copy leaves on an emptied
// clipboard, null between a press and its release.

const request = JSON.parse(fs.readFileSync(process.argv[process.argv.length - 1], "utf8"));

function fail(message: string): never {
  process.stderr.write(`genTextSelectionMain: ${message}\n`);
  app.exit(1);
  throw new Error(message);
}

function mulberry32(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const PAGE_SCRIPT = `(() => {
  const blocks = [];
  for (const block of document.querySelectorAll("[data-b]")) blocks[Number(block.dataset.b)] = block;
  for (const table of document.querySelectorAll("[data-t]")) for (const i of table.dataset.t.split(" ")) blocks[Number(i)] = null;
  const tables = (node, side) => node !== undefined && node.nodeType === 1 && node.dataset.t !== undefined ? Number(node.dataset.t.split(" ")[side]) : null;
  const leaves = (block) => {
    const out = [];
    const walk = document.createTreeWalker(block, NodeFilter.SHOW_ALL);
    for (let node = walk.nextNode(); node !== null; node = walk.nextNode()) {
      if (node.nodeType === 3) out.push([node, node.data.length]);
      else if (node.nodeName === "BR") out.push([node, 1]);
    }
    return out;
  };
  const offsetBefore = (block, node) => {
    let sum = 0;
    for (const [leaf, length] of leaves(block)) {
      if (leaf === node || !(leaf.compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING)) break;
      sum += length;
    }
    return sum;
  };
  const position = (node, offset) => {
    if (node === null) return null;
    const block = node.nodeType === 1 && node.dataset.b !== undefined ? node : (node.nodeType === 1 ? node : node.parentElement).closest("[data-b]");
    if (block !== null) {
      const index = blocks.indexOf(block);
      if (node.nodeType === 3) return [index, offsetBefore(block, node) + offset];
      if (node.nodeName === "BR") return [index, offsetBefore(block, node)];
      const child = node.childNodes[offset];
      if (child === undefined) return [index, leaves(block).reduce((n, [leaf, length]) => node.contains(leaf) || (leaf.compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING) ? n + length : n, 0)];
      return [index, offsetBefore(block, child)];
    }
    const edge = tables(node.childNodes[offset], 0) ?? tables(node.childNodes[offset - 1], 1);
    if (edge !== null) return [edge, 0];
    const range = document.createRange();
    range.setStart(node, offset);
    const next = blocks.findIndex((b) => b !== null && range.comparePoint(b, 0) >= 0);
    if (next >= 0) return [next, 0];
    const last = blocks.length - 1;
    return [last, leaves(blocks[last]).reduce((n, [, length]) => n + length, 0)];
  };
  window.__texts = () => blocks.map((b) => (b === null ? [] : leaves(b)).map(([leaf]) => (leaf.nodeType === 3 ? leaf.data : "\\n")).join(""));
  // What [offset, side] of a block has around it.
  window.__around = (index, offset, side) => {
    const block = blocks[index];
    const text = window.__texts()[index];
    const rect = (at) => {
      for (const [leaf, length] of leaves(block)) {
        if (at >= 0 && at < length && leaf.nodeType === 3) {
          const range = document.createRange();
          range.setStart(leaf, at);
          range.setEnd(leaf, at + 1);
          return [...range.getClientRects()].find((r) => r.width > 0) ?? null;
        }
        if (at < length) return null;
        at -= length;
      }
      return null;
    };
    const caret = offset + side;
    const collapses = getComputedStyle(block).whiteSpace === "normal";
    const before = rect(caret - 1);
    let next = caret;
    while (collapses && text[next] === " ") next++;
    const after = rect(next);
    const on = rect(offset);
    const wraps = caret > 0 && caret < text.length && before !== null && after !== null && after.top > before.top + 2 && Math.abs(on.top - before.top) <= 2;
    let at = offset;
    for (const [leaf, length] of leaves(block)) {
      if (at < length) {
        const link = leaf.parentElement.closest("a");
        return [wraps, link === null ? null : [offsetBefore(block, link), offsetBefore(block, link) + link.textContent.length]];
      }
      at -= length;
    }
    return [wraps, null];
  };
  window.__tables = () => blocks.flatMap((b, i) => (b === null ? [i] : []));
  window.__reaches = (target) => {
    const [x, y] = window.__point(target);
    const element = document.elementFromPoint(x, y);
    return element !== null && element.closest("[data-b]") === blocks[target[1]];
  };
  window.__others = () => [...document.querySelectorAll("[data-u]")].map((e) => Number(e.dataset.u));
  window.__ends = () => {
    const s = getSelection();
    return s.anchorNode === null ? null : [position(s.anchorNode, s.anchorOffset), position(s.focusNode, s.focusOffset)];
  };
  window.__clear = () => getSelection().removeAllRanges();
  window.__point = (target) => {
    const [kind, index, offset, side] = target;
    const centre = (r) => [r.x + r.width / 2, r.y + r.height / 2];
    if (kind === "other") return centre(document.querySelector('[data-u="' + index + '"]').getBoundingClientRect());
    const block = blocks[index];
    if (kind === "cell") return centre(block.getBoundingClientRect());
    if (kind === "after") {
      const range = document.createRange();
      range.selectNodeContents(block);
      const rects = [...range.getClientRects()].filter((r) => r.width > 0);
      const last = rects[rects.length - 1];
      return [last.right + 3, last.y + last.height / 2];
    }
    let at = offset;
    for (const [leaf, length] of leaves(block)) {
      if (leaf.nodeType === 3 && at < length) {
        const range = document.createRange();
        range.setStart(leaf, at);
        range.setEnd(leaf, at + 1);
        const r = [...range.getClientRects()].find((r) => r.width > 0);
        return [r.x + r.width * (side === 0 ? 0.25 : 0.75), r.y + r.height / 2];
      }
      at -= length;
    }
    throw new Error("no character " + JSON.stringify(target));
  };
  return true;
})()`;

app.setPath("userData", request.userData);
app.dock?.hide();
app
  .whenReady()
  .then(async () => {
    // Copying from an offscreen page crashes Electron on Linux.
    const offscreen = process.platform === "darwin";
    const win = new BrowserWindow({ width: 1000, height: 700, useContentSize: true, show: false, webPreferences: { offscreen, backgroundThrottling: false, spellcheck: false } });
    if (!offscreen) win.showInactive();
    win.webContents.on("render-process-gone", (_event, details) => fail(`renderer gone: ${details.reason}`));
    win.webContents.debugger.attach("1.3");
    const send = (method: string, params: object) => win.webContents.debugger.sendCommand(method, params);
    const evaluate = async (expression: string): Promise<any> => {
      const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
      if (result.exceptionDetails !== undefined) fail(`${expression}: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
      return result.result.value;
    };
    const settle = () => evaluate("new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)))");
    const copy = async () => {
      clipboard.clear();
      win.webContents.copy();
      const selected = (await evaluate("getSelection().type")) === "Range";
      for (let i = 0; i < (selected ? 20 : 3); i++) {
        await new Promise((r) => setTimeout(r, 5));
        if (clipboard.readText() !== "") break;
      }
      return clipboard.readText();
    };
    if (offscreen) win.webContents.setFrameRate(240);
    const lines: string[] = [];
    for (const page of request.fixture.pages) {
      fs.writeFileSync(request.page, `<!doctype html><html><head><meta charset="utf-8"><style>${page.css}</style></head><body${page.body ?? ""}>${page.html}</body></html>`);
      await win.loadFile(request.page);
      await send("Emulation.setFocusEmulationEnabled", { enabled: true });
      if ((await evaluate("devicePixelRatio")) !== 1) fail("devicePixelRatio is not 1");
      await evaluate(PAGE_SCRIPT);
      await settle();
      const texts: string[] = await evaluate("window.__texts()");
      const others: number[] = await evaluate("window.__others()");
      const tables: number[] = await evaluate("window.__tables()");
      lines.push(JSON.stringify({ page: page.name, platform: process.platform, texts }));
      const random = mulberry32(page.seed);
      const pick = (n: number) => Math.floor(random() * n);
      const target = async (): Promise<any[]> => {
        const roll = random();
        if (others.length > 0 && roll < 0.12) return ["other", others[pick(others.length)]];
        let block = pick(texts.length);
        while (tables.includes(block)) block = pick(texts.length);
        const text = texts[block];
        if (text === "") return ["cell", block];
        if (roll < 0.22 && !text.endsWith("\n") && (await evaluate(`window.__reaches(["after", ${block}])`))) return ["after", block];
        for (;;) {
          const offset = pick(text.length);
          if (text[offset] !== "\n" && !/[\ud800-\udfff]/.test(text[offset])) {
            const side = pick(2);
            return ["char", block, offset, side, ...(await evaluate(`window.__around(${block}, ${offset}, ${side})`))];
          }
        }
      };
      const caretOf = (t: any[]): number[] | null => {
        if (t[0] === "char") return [t[1], t[2] + t[3]];
        if (t[0] === "after") return [t[1], texts[t[1]].length];
        return t[0] === "cell" ? [t[1], 0] : null;
      };
      const order = (a: number[], b: number[]) => a[0] - b[0] || a[1] - b[1];
      for (let sequence = 0; sequence < page.sequences; sequence++) {
        await evaluate("window.__clear()");
        const run = async (op: any[], copies: boolean) => {
          const record = { page: page.name, sequence, op, ends: await evaluate("window.__ends()"), copied: copies ? await copy() : null };
          lines.push(JSON.stringify(record));
        };
        const mouse = async (type: string, t: any[], clickCount: number, shift: boolean, button = "left") => {
          const [x, y] = await evaluate(`window.__point(${JSON.stringify(t)})`);
          const buttons = type === "mouseReleased" || type === "mouseMoved" && clickCount === 0 ? 0 : button === "left" ? 1 : 2;
          await send("Input.dispatchMouseEvent", { type, x, y, button: type === "mouseMoved" && clickCount === 0 ? "none" : button, buttons, clickCount, modifiers: shift ? 8 : 0 });
        };
        for (let step = 0; step < page.steps; step++) {
          const roll = random();
          if (roll < 0.08) {
            win.webContents.selectAll();
            await settle();
            await new Promise((r) => setTimeout(r, 10));
            await run(["all"], true);
            continue;
          }
          const at = await target();
          const ends: number[][] | null = await evaluate("window.__ends()");
          const caret = caretOf(at);
          const shift = random() < 0.2;
          const sorted = ends === null ? null : [...ends].sort(order);
          const inside = sorted !== null && caret !== null && order(sorted[0], sorted[1]) !== 0 && order(sorted[0], caret) <= 0 && order(caret, sorted[1]) <= 0;
          await mouse("mouseMoved", at, 0, false);
          if (roll < 0.16) {
            await mouse("mousePressed", at, 1, false, "right");
            await mouse("mouseReleased", at, 1, false, "right");
            await run(["menu", at], true);
            continue;
          }
          const clicks = shift ? 1 : 1 + pick(3);
          for (let click = 1; click <= clicks; click++) {
            await mouse("mousePressed", at, click, shift);
            await run(["press", at, click, shift], false);
            if (click < clicks) {
              await mouse("mouseReleased", at, click, shift);
              await run(["release", at], false);
            }
          }
          let last = at;
          // A drag of selected text would start an operating system drag.
          if (clicks > 1 || !inside || shift) {
            for (let moves = pick(3); moves > 0; moves--) {
              last = await target();
              await mouse("mouseMoved", last, 1, false);
              await run(["move", last], false);
            }
          }
          await mouse("mouseReleased", last, clicks, false);
          await run(["release", last], true);
        }
      }
    }
    fs.writeFileSync(request.output, lines.map((l) => l + "\n").join(""));
    app.exit(0);
  })
  .catch((error) => fail(String(error?.stack ?? error)));
