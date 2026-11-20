import { fromTree, toTree } from "../cve1";

// Records every call into the real SourceList instances so the port can replay it (fixture key
// "sourceLists", see oracle/README.md "Source lists"). Only outermost calls are recorded:
// a hooked method that runs inside another (setState calling clear, a field drag calling
// addField) is part of the outer op.

const w = window as any;
let SL: any;
const lists: any[] = [];
let depth = 0;
let memoryRef: unknown = undefined;
let numberCalls: unknown[][] = [];
const write = (record: object) => w.__oracleWrite(JSON.stringify(record));

function errorText(error: unknown): string {
  return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

function rect(el: Element): number[] {
  const r = el.getBoundingClientRect();
  return [r.x, r.y, r.width, r.height];
}

function geometry(sl: any): object {
  return { root: rect(sl.ROOT), list: rect(sl.LIST), items: Array.from(sl.LIST.children as HTMLCollection).map(rect) };
}

function isDark(): boolean {
  return w.matchMedia("(prefers-color-scheme: dark)").matches;
}

function supplemental(sl: any): unknown {
  return sl.supplementalStateSuppliers.map((supplier: () => unknown) => supplier());
}

function idOf(sl: any): number {
  let id = lists.indexOf(sl);
  if (id !== -1) return id;
  id = lists.length;
  lists.push(sl);
  write({
    sl: id,
    create: toTree({
      uuid: sl.UUID,
      config: JSON.parse(sl.configStr),
      title: sl.TITLE.innerText,
      editButton: !sl.EDIT_BUTTON.hidden,
      numberPreview: sl.getNumberPreview !== undefined,
      supplemental: sl.supplementalStateSuppliers.length
    })
  });
  if (sl.getNumberPreview !== undefined) {
    const preview = sl.getNumberPreview;
    sl.getNumberPreview = (key: string, time: number) => {
      const text = preview(key, time);
      numberCalls.push([key, time, text]);
      return text;
    };
  }
  return id;
}

function syncMemory() {
  if (w.typeMemory === memoryRef) return;
  memoryRef = w.typeMemory;
  write({ slTypeMemory: toTree(w.typeMemory) });
}

function op(sl: any, value: unknown[], extra: object = {}) {
  const id = idOf(sl);
  syncMemory();
  write({ sl: id, op: toTree(value), dark: isDark(), ...extra });
}

function hook(name: string, describe: (sl: any, args: any[]) => unknown[]) {
  const original = SL.prototype[name];
  SL.prototype[name] = function (this: any, ...args: any[]) {
    if (depth > 0) return original.apply(this, args);
    const id = idOf(this);
    syncMemory();
    const value = toTree(describe(this, args));
    numberCalls = [];
    const extra: { [key: string]: unknown } = { dark: isDark() };
    depth++;
    try {
      return original.apply(this, args);
    } catch (error) {
      extra.threw = toTree(errorText(error));
      throw error;
    } finally {
      depth--;
      if (name === "updateAllPreviews") extra.numberPreviews = toTree(numberCalls);
      if (name !== "updateAllItems" || "threw" in extra) write({ sl: id, op: value, ...extra });
    }
  };
}

function itemDrag(sl: any, data: any): unknown[] {
  const d = data.data;
  const source = lists.find((other) => other.UUID === d.sourceListUUID);
  return [
    "itemDrag",
    {
      source: source === undefined ? null : idOf(source),
      sameConfig: d.sourceListConfigStr === sl.configStr,
      index: d.sourceListIndex,
      isChild: d.sourceListIsChild,
      end: data.end,
      x: data.x,
      y: data.y
    },
    geometry(sl)
  ];
}

function fieldDrag(sl: any, data: any): unknown[] {
  return [
    "fieldDrag",
    { fields: data.data.fields, children: data.data.children, end: data.end, x: data.x, y: data.y },
    geometry(sl),
    supplemental(sl)
  ];
}

function frame(name: string) {
  return () => [name, isDark(), w.selection.getRenderTime()];
}

function owner(target: EventTarget | null): any {
  if (!(target instanceof w.Element)) return undefined;
  return lists.find((sl) => sl.ROOT.contains(target));
}

function itemIndex(sl: any, target: Element): number {
  const item = target.closest("div.item");
  if (item === null || item.parentElement !== sl.LIST) return -1;
  return Array.from(sl.LIST.children).indexOf(item);
}

function onEvent(event: any) {
  const sl = owner(event.target);
  if (sl === undefined) return;
  const target = event.target as Element;
  const inList = sl.LIST.contains(target);
  const index = itemIndex(sl, target);
  switch (event.type) {
    case "mousedown":
      if (inList) op(sl, ["mouseDown", event.clientX, event.clientY]);
      return;
    case "mouseup":
      if (inList) op(sl, ["mouseUp"]);
      return;
    case "mousemove":
      if (inList) op(sl, ["mouseMove", event.clientX, event.clientY, geometry(sl)]);
      return;
    case "contextmenu":
      if (index !== -1) op(sl, ["contextMenu", index, event.clientX, event.clientY]);
      return;
    case "click": {
      if (target.closest("button.clear") !== null && sl.ROOT.contains(target.closest("button.clear"))) {
        op(sl, ["clearClick"]);
        return;
      }
      if (index === -1) return;
      if (target.closest("button.type") !== null) op(sl, ["promptType", index]);
      else if (target.closest("button.hide") !== null) op(sl, ["toggleHidden", index]);
      else if (target.closest("button.remove") !== null) op(sl, ["remove", index]);
      else if (target.closest(".type-name, .key-container") !== null) op(sl, ["nameClick", index, new Date().getTime()]);
      return;
    }
  }
}

function callbacks(name: string, opName: string) {
  const store: { [uuid: string]: Function } = {};
  SL[name] = new Proxy(store, {
    set(target, uuid: string, callback: Function) {
      target[uuid] = (...args: unknown[]) => {
        const sl = lists.find((other) => other.UUID === uuid);
        if (sl !== undefined) op(sl, [opName, ...args]);
        depth++;
        try {
          return callback(...args);
        } finally {
          depth--;
        }
      };
      return true;
    }
  });
}

// The hub's SourceList class is reached through a live instance: importing it here would bundle
// a second copy of the module under the oracle's symlinked upstream path.
export function install() {
  const existing: any[] = [];
  for (const tab of w.tabs.tabList) {
    for (const value of Object.values(tab.controller)) {
      if (typeof value === "object" && value !== null && "configStr" in value && "LIST" in value) existing.push(value);
    }
  }
  if (existing.length === 0) throw new Error("sourceLists: no SourceList instance to hook");
  SL = existing[0].constructor;
  existing.forEach(idOf);
  hook("setTitle", (_, [title]) => ["setTitle", title]);
  hook("setState", (_, [state]) => ["setState", state]);
  hook("clear", () => ["clear"]);
  hook("refresh", () => ["refresh"]);
  hook("stop", () => ["stop"]);
  hook("setOptionValues", (_, [type, option, values]) => ["setOptionValues", type, option, values]);
  hook("addField", (sl, [key, parentIndex]) => ["addField", key, parentIndex, supplemental(sl)]);
  hook("handleItemDrag", (sl, [data]) => itemDrag(sl, data));
  hook("handleFieldDrag", (sl, [data]) => fieldDrag(sl, data));
  hook("updateAllItems", frame("frame"));
  hook("updateAllPreviews", frame("frame"));
  callbacks("typePromptCallbacks", "typeResponse");
  callbacks("clearPromptCallbacks", "clearResponse");
  for (const type of ["mousedown", "mouseup", "mousemove", "contextmenu", "click"]) {
    w.addEventListener(type, onEvent, true);
  }
  const startDrag = w.startDrag;
  w.startDrag = (x: number, y: number, offsetX: number, offsetY: number, data: any) => {
    const source = lists.find((other) => other.UUID === data?.sourceListUUID);
    if (source !== undefined) {
      write({
        sl: idOf(source),
        dragStart: toTree({ x, y, offsetX, offsetY, index: data.sourceListIndex, isChild: data.sourceListIsChild })
      });
    }
    return startDrag(x, y, offsetX, offsetY, data);
  };
}

export function recordLog(ops: unknown[][]) {
  write({ slLog: toTree(ops.map((o) => o.map((a) => fromTree(a)))) });
}

function itemDump(sl: any, item: HTMLElement): unknown[] {
  const objects = item.getElementsByTagName("object");
  const keyContainer = item.getElementsByClassName("key-container")[0] as HTMLElement;
  const keySpan = keyContainer.firstElementChild as HTMLElement;
  const hideButton = item.getElementsByClassName("hide")[0] as HTMLElement;
  const value = item.getElementsByClassName("value")[0] as HTMLElement;
  const valueSymbol = item.getElementsByClassName("value-symbol")[0] as HTMLElement;
  return [
    objects[1].getAttribute("data"),
    objects[0].getAttribute("type-color"),
    (item.getElementsByClassName("type-name")[0] as HTMLElement).innerText,
    keySpan.innerText,
    keySpan.style.textDecoration,
    keyContainer.title,
    (hideButton.firstElementChild as HTMLElement).getAttribute("src"),
    hideButton.title,
    item.classList.contains("hidden"),
    item.classList.contains("child"),
    item.classList.contains("parent-highlight"),
    valueSymbol.hidden,
    value.hidden,
    value.innerText,
    item.style.height
  ];
}

export function capture() {
  lists.forEach((sl, id) => {
    const highlight = sl.DRAG_HIGHLIGHT as HTMLElement;
    write({
      sl: id,
      dump: toTree({
        title: sl.TITLE.innerText,
        state: sl.state,
        displayed: sl.getState(true),
        config: sl.config,
        stopped: sl.stopped,
        handIcon: [sl.HAND_ICON.style.opacity, sl.HAND_ICON.style.transition],
        highlight: [highlight.hidden, highlight.style.left, highlight.style.top, highlight.style.width, highlight.style.height],
        buttons: [sl.EDIT_BUTTON.hidden, sl.CLEAR_BUTTON.hidden, sl.HELP_BUTTON.hidden],
        items: Array.from(sl.LIST.children as HTMLCollection).map((item) => itemDump(sl, item as HTMLElement))
      })
    });
  });
  write({ slMemory: toTree(w.typeMemory) });
}
