export const PATCH = 16;
export const COLUMNS = 32;

export function patchColours(): [number, number, number][] {
  const colours: [number, number, number][] = [];
  const seen = new Set<string>();
  const add = (c: [number, number, number]) => {
    const key = c.join(",");
    if (!seen.has(key)) {
      seen.add(key);
      colours.push(c);
    }
  };
  for (let i = 0; i < 8; i++) add([i & 1 ? 255 : 0, i & 2 ? 255 : 0, i & 4 ? 255 : 0]);
  for (const hex of ["2b66a2", "e5b31b", "af2437", "80588e", "e48b32", "c0b487", "858584", "3b875a", "d993aa", "5f4528", "ff8c00"]) {
    add([0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16)) as [number, number, number]);
  }
  for (let v = 0; v < 256; v++) add([v, v, v]);
  const steps = [0, 51, 102, 153, 204, 255];
  for (const r of steps) for (const g of steps) for (const b of steps) add([r, g, b]);
  for (let i = 0; colours.length < 512; i++) {
    const v = (i * 37 + 11) % 256;
    add([v, (v * 7 + 90) % 256, (v * 13 + 200) % 256]);
  }
  return colours;
}

export function patchPageHtml(): string {
  const cells = patchColours()
    .map(([r, g, b], i) => {
      const x = (i % COLUMNS) * PATCH;
      const y = Math.floor(i / COLUMNS) * PATCH;
      return `<div class="patch" data-rgb="${r},${g},${b}" style="position:absolute;left:${x}px;top:${y}px;width:${PATCH}px;height:${PATCH}px;background:rgb(${r},${g},${b})"></div>`;
    })
    .join("");
  return `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0;overflow:hidden;background:#000}</style></head><body>${cells}</body></html>`;
}
