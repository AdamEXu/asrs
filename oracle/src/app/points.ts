/** Where `[x, y]` is on this page, and what is there. `titleBar` points were measured in a hub
 *  whose viewer starts 38 px down; here the viewer may start at the top, with only the tab
 *  controls where they were. */
export function placePoint(x: number, y: number, titleBar: boolean, on: string | null, right: number | null): { x: number; y: number; hit: string | null } {
  const viewer = titleBar ? document.querySelector("div.viewer") : null;
  if (viewer !== null && y >= 38) {
    const top = viewer.getBoundingClientRect();
    const handle = document.querySelector("div.controls-handle")!.getBoundingClientRect();
    const controls = handle.height > 0 ? handle.top : Infinity;
    if (x >= top.left && y < controls) y += top.top - 38;
  }
  const at = (px: number, py: number) => document.elementFromPoint(px, py);
  if (on !== null && (right !== null || !at(x, y)?.closest(on))) {
    const shown = Array.from(document.querySelectorAll(on)).find((el) => {
      const box = el.getBoundingClientRect();
      return box.width > 0 && box.height > 0;
    });
    if (shown === undefined) throw new Error(`no ${on} is shown`);
    const box = shown.getBoundingClientRect();
    const middle = (from: number, to: number, size: number) => Math.round((Math.max(from, 0) + Math.min(to, size)) / 2);
    // Upstream's text does not fit every window on every host: what is scrolled out cannot be hit.
    if (box.top < innerHeight && box.bottom > 0) {
      x = right === null ? middle(box.left, box.right, innerWidth) : Math.round(box.right - right);
      y = middle(box.top, box.bottom, innerHeight);
      if (at(x, y)?.closest(on) !== shown) throw new Error(`${on} is covered at [${x}, ${y}]`);
    }
  }
  const hit = at(x, y);
  const names = [];
  for (let el = hit; el !== null && names.length < 3; el = el.parentElement) {
    names.unshift(el.localName + Array.from(el.classList, (c) => `.${c}`).join("") + (el.id === "" ? "" : `#${el.id}`));
  }
  return { x, y, hit: hit === null ? null : names.join(" > ") };
}
