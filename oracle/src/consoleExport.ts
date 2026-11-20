import ConsoleRenderer from "@upstream/shared/renderers/ConsoleRenderer";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

class FakeElement {
  children: FakeElement[];
  className: string;
  value = "";
  hidden = false;
  innerText = "";
  style: Record<string, string> = {};
  classList = { contains: () => false, add: () => {}, remove: () => {}, toggle: () => {} };
  listeners = new Map<string, () => void>();

  constructor(className = "", children: FakeElement[] = []) {
    this.className = className;
    this.children = children;
  }
  get firstElementChild() {
    return this.children[0] ?? null;
  }
  get lastElementChild() {
    return this.children[this.children.length - 1] ?? null;
  }
  getElementsByClassName(name: string): FakeElement[] {
    return this.children.flatMap((c) => [...(c.className === name ? [c] : []), ...c.getElementsByClassName(name)]);
  }
  addEventListener(type: string, listener: () => void) {
    this.listeners.set(type, listener);
  }
}

// The element shape ConsoleRenderer's constructor walks in hub.html's console template.
function consoleRoot(): { root: FakeElement; exportButton: FakeElement } {
  const exportButton = new FakeElement();
  const fieldCell = new FakeElement("", [
    new FakeElement("", [new FakeElement(), new FakeElement()]),
    exportButton,
    new FakeElement("highlight-button"),
    new FakeElement()
  ]);
  const header = new FakeElement("", [new FakeElement("", [new FakeElement(), new FakeElement()]), fieldCell]);
  const container = new FakeElement("console-table-container", [new FakeElement("", [new FakeElement("", [header])])]);
  return { root: new FakeElement("", [container, new FakeElement("large-table-hand-icon")]), exportButton };
}

// Clicks the export button of a real ConsoleRenderer holding `values` and returns the main
// message it sends; for "export-console", the data is the bytes main's fs.writeFile stores.
export async function consoleExport(values: string[]): Promise<{ message: string; data: unknown }> {
  const g = globalThis as any;
  let sent: { message: string; data: unknown } | null = null;
  g.addEventListener ??= () => {};
  g.sendMainMessage = (message: string, data: unknown) => (sent = { message, data });

  const { root, exportButton } = consoleRoot();
  const renderer = new ConsoleRenderer(root as unknown as HTMLElement, true);
  (renderer as any).lines = values.map((value) => ({ timestamp: 0, value, isError: false, isWarning: false }));
  exportButton.listeners.get("click")!();
  if (sent === null) throw new Error("export click sent no message");
  const { message, data } = sent;
  if (message !== "export-console") return { message, data };

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "asrs-console-export-"));
  try {
    const file = path.join(dir, "export.txt");
    await new Promise<void>((resolve, reject) => fs.writeFile(file, data as string, (err) => (err ? reject(err) : resolve())));
    return { message, data: new Uint8Array(fs.readFileSync(file)) };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}
