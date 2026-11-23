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
