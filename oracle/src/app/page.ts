// Each function here is sent to the page with Function.prototype.toString and must not
// reference anything outside its own body.

export function hubReady(): boolean {
  const w = window as any;
  return (
    document.readyState === "complete" &&
    w.__asrsParity !== undefined &&
    w.messagePort !== null &&
    w.preferences !== null &&
    w.assets !== null &&
    w.platform !== ""
  );
}

export function busyReasons(): string[] {
  const w = window as any;
  const reasons: string[] = [];
  if (document.fonts.status !== "loaded") reasons.push("fonts");
  if (typeof w.getLoadingFields === "function" && w.getLoadingFields().size > 0) reasons.push("fields loading");
  if (document.documentElement.style.getPropertyValue("--show-loading-glow") === "1") reasons.push("log loading");
  if (/\((Loading)\)/.test(document.title)) reasons.push("title loading");
  if (document.querySelector(".spinner-cubes-container.visible") !== null) reasons.push("3D field or robot loading");
  const satellite = document.body.classList.contains("satellite");
  if (!satellite && w.selection && typeof w.selection.getMode === "function" && [2, 3].includes(w.selection.getMode())) reasons.push("playback or live");
  return reasons;
}

export function sendMainMessage(name: string, data: unknown) {
  const port = (window as any).messagePort as MessagePort;
  port.dispatchEvent(new MessageEvent("message", { data: { name, data } }));
}

/** Whether the download window shows its list ("listed"), an alert or progress text starting with this, or this much of its progress bar. */
export function downloadShows(expected: string | number): boolean {
  const alert = document.querySelector("div.alert-text") as HTMLElement;
  const progress = document.querySelector("progress") as HTMLProgressElement;
  const details = document.querySelector("div.progress-details") as HTMLElement;
  if (typeof expected === "number") return !progress.hidden && progress.value === expected;
  if (expected === "listed") return (document.querySelector("div.loading") as HTMLElement).hidden && alert.hidden && progress.hidden;
  return [alert, details].some((el) => !el.hidden && el.innerText.startsWith(expected));
}

/**
 * `document.activeElement`: what it is, whether Tab reaches it (a scroller that holds nothing to focus has no `tabindex`
 * and is), its box, whether it takes `global.css`'s outline, and an input's value and selection; null when it is `body`.
 */
export function focused() {
  const el = document.activeElement as HTMLElement | null;
  if (el === null || el === document.body) return null;
  const r = el.getBoundingClientRect();
  const style = getComputedStyle(el);
  const classes = typeof el.className === "string" ? el.className.trim().split(/\s+/).filter((c) => c !== "") : [];
  const text = el instanceof HTMLInputElement && ["text", "search", "number"].includes(el.type) ? el : null;
  return {
    element: [el.tagName.toLowerCase(), ...classes].join("."),
    title: el.title,
    stop: el.tabIndex >= 0 || !el.hasAttribute("tabindex"),
    rect: [r.x, r.y, r.width, r.height],
    ring: style.outline === "rgb(68, 108, 227) solid 2px",
    value: text?.value,
    selection: text?.selectionStart == null ? undefined : [text.selectionStart, text.selectionEnd]
  };
}

export function domDump(probes: string[], maskSelectors: string[]) {
  const rect = (el: Element | Range) => {
    const r = el.getBoundingClientRect();
    return [r.x, r.y, r.width, r.height];
  };
  const visible = (el: Element) => el.getClientRects().length > 0 && getComputedStyle(el).visibility !== "hidden";

  const probed: Record<string, number[][]> = {};
  for (const selector of probes) {
    probed[selector] = Array.from(document.querySelectorAll(selector))
      .filter(visible)
      .map((el) => rect(el));
  }

  const texts: { text: string; rects: number[][]; font: string; color: string; parent: string }[] = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    const text = node.textContent ?? "";
    const parent = node.parentElement;
    if (text.trim() === "" || parent === null || !visible(parent)) continue;
    range.selectNodeContents(node);
    const rects = Array.from(range.getClientRects())
      .filter((r) => r.width > 0 && r.height > 0)
      .map((r) => [r.x, r.y, r.width, r.height]);
    if (rects.length === 0) continue;
    const style = getComputedStyle(parent);
    texts.push({
      text,
      rects,
      font: `${style.fontStyle} ${style.fontWeight} ${style.fontSize} ${style.fontFamily}`,
      color: style.color,
      parent: parent.tagName.toLowerCase() + (parent.className && typeof parent.className === "string" ? "." + parent.className.trim().split(/\s+/).join(".") : "")
    });
  }
  const controls = Array.from(document.querySelectorAll("input, select, textarea"))
    .filter(visible)
    .map((el) => ({
      tag: el.tagName.toLowerCase(),
      type: (el as HTMLInputElement).type,
      value: (el as HTMLInputElement).value,
      rect: rect(el)
    }));

  const masks: { reason: string; rect: number[] }[] = [];
  const active = document.activeElement;
  if (active !== null && active.matches("input:not([type=checkbox]):not([type=radio]):not([type=range]), textarea, [contenteditable]")) {
    masks.push({ reason: "focused text input", rect: rect(active) });
  }
  for (const el of Array.from(document.querySelectorAll("webview"))) if (visible(el)) masks.push({ reason: "webview", rect: rect(el) });
  for (const selector of maskSelectors) {
    for (const el of Array.from(document.querySelectorAll(selector))) if (visible(el)) masks.push({ reason: selector, rect: rect(el) });
  }

  return {
    devicePixelRatio: window.devicePixelRatio,
    viewport: [window.innerWidth, window.innerHeight],
    language: navigator.language,
    darkScheme: window.matchMedia("(prefers-color-scheme: dark)").matches,
    title: document.title,
    probes: probed,
    texts,
    controls,
    masks
  };
}

/** Where `[x, y]` is on this page, and what is there. `titleBar` points were measured in a hub
 *  whose viewer starts 38 px down; here the viewer may start at the top, with only the tab
 *  controls where they were. */
export function placePoint(x: number, y: number, titleBar: boolean, on: string | null, right: number | null): { x: number; y: number; hit: string | null } {
  const viewer = titleBar ? document.querySelector("div.viewer") : null;
  if (viewer !== null && y >= 38) {
    const top = document.querySelector("div.viewer")!.getBoundingClientRect();
    const handle = document.querySelector("div.controls-handle")!.getBoundingClientRect();
    const controls = handle.height > 0 ? handle.top : Infinity;
    if (x >= top.left && y < controls) y += top.top - 38;
  }
  const at = (px: number, py: number) => document.elementFromPoint(px, py);
  if (on !== null && (right !== null || at(x, y)?.closest(on) === null)) {
    const shown = Array.from(document.querySelectorAll(on)).find((el) => {
      const box = el.getBoundingClientRect();
      return box.width > 0 && box.height > 0;
    });
    if (shown === undefined) throw new Error(`no ${on} is shown`);
    const box = shown.getBoundingClientRect();
    [x, y] = [Math.round(right === null ? box.x + box.width / 2 : box.right - right), Math.round(box.y + box.height / 2)];
    if (at(x, y)?.closest(on) !== shown) throw new Error(`${on} is covered at [${x}, ${y}]`);
  }
  const hit = at(x, y);
  const names = [];
  for (let el = hit; el !== null && names.length < 3; el = el.parentElement) {
    names.unshift(el.localName + Array.from(el.classList, (c) => `.${c}`).join("") + (el.id === "" ? "" : `#${el.id}`));
  }
  return { x, y, hit: hit === null ? null : names.join(" > ") };
}
