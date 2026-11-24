// What `--tooltips` records at a capture instead of its pixels. Functions here are sent to the
// page like page.ts's.

export type Titled = { title: string; rect: number[]; point: [number, number] };
export type TitleMap = {
  titles: Titled[];
  /** What `grid` indexes. */
  texts: string[];
  /** A row a line, a point every GRID_STEP px: "?" near an edge, "." without a title, else a letter of `texts`. */
  grid: string[];
};

export const GRID_STEP = 24;
const EDGE = 3;

export function titleMap(step: number, edge: number): TitleMap {
  // HitTestResult::Title: the nearest inclusive ancestor with the attribute, empty or not.
  const titleAt = (x: number, y: number) => document.elementFromPoint(x, y)?.closest("[title]")?.getAttribute("title") ?? null;
  const titles: Titled[] = [];
  for (const el of Array.from(document.querySelectorAll<HTMLElement>("[title]"))) {
    const r = el.getBoundingClientRect();
    if (el.title === "" || r.width === 0 || r.height === 0) continue;
    const point: [number, number] = [Math.round(r.x + r.width / 2), Math.round(r.y + r.height / 2)];
    const at = document.elementFromPoint(...point)?.closest("[title]");
    if (at !== el) continue;
    titles.push({ title: el.title, rect: [r.x, r.y, r.width, r.height], point });
  }
  const texts: string[] = [];
  const grid: string[] = [];
  for (let y = step / 2; y < window.innerHeight; y += step) {
    let row = "";
    for (let x = step / 2; x < window.innerWidth; x += step) {
      const title = titleAt(x, y);
      const around = [-edge, 0, edge].flatMap((dx) => [-edge, 0, edge].map((dy) => titleAt(x + dx, y + dy)));
      if (!around.every((other) => other === title)) row += "?";
      else if (title === null) row += ".";
      else {
        if (!texts.includes(title)) texts.push(title);
        row += String.fromCharCode(0x41 + texts.indexOf(title));
      }
    }
    grid.push(row);
  }
  return { titles, texts, grid };
}

export const titleMapArgs = [GRID_STEP, EDGE] as const;
