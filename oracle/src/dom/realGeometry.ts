import { toTree } from "../cve1";

// `real` geometry: Blink lays the page out, and each geometry read upstream makes is written as
// a `read` record (and each scroll position write as a `write` record) at the moment it
// happens, so a port's headless model can be answered in the same order. Harness code reads
// inside `quiet` so its own reads are not recorded.

let quietDepth = 0;

export function quiet<T>(fn: () => T): T {
  quietDepth++;
  try {
    return fn();
  } finally {
    quietDepth--;
  }
}

/** `tag.class.class:i` for each element from `<body>` down, `i` being its index among its
 * parent's element children. */
export function elementPath(el: Element): string {
  const parts: string[] = [];
  for (let e: Element | null = el; e !== null && e !== document.documentElement; e = e.parentElement) {
    const classes = Array.from(e.classList, (c) => "." + c).join("");
    const parent = e.parentElement;
    const index = parent === null ? 0 : Array.prototype.indexOf.call(parent.children, e);
    parts.push(`${e.tagName.toLowerCase()}${classes}:${index}`);
  }
  return parts.reverse().join(">");
}

const rectTree = (r: DOMRectReadOnly) => [r.x, r.y, r.width, r.height];

export function installRealGeometry(w: any, write: (record: object) => void) {
  const record = (kind: "read" | "write", name: string, el: Element, value: unknown) => {
    if (quietDepth === 0) write({ [kind]: name, el: elementPath(el), value: toTree(value) });
  };
  const elementProto = w.Element.prototype;
  const getBoundingClientRect = elementProto.getBoundingClientRect;
  elementProto.getBoundingClientRect = function (this: Element) {
    const rect = getBoundingClientRect.call(this);
    record("read", "getBoundingClientRect", this, rectTree(rect));
    return rect;
  };
  const getClientRects = elementProto.getClientRects;
  elementProto.getClientRects = function (this: Element) {
    const rects = getClientRects.call(this);
    record("read", "getClientRects", this, Array.from(rects as DOMRectList, rectTree));
    return rects;
  };
  const wrap = (proto: object, names: string[]) => {
    for (const name of names) {
      const original = Object.getOwnPropertyDescriptor(proto, name)!;
      Object.defineProperty(proto, name, {
        configurable: true,
        enumerable: original.enumerable,
        get(this: Element) {
          const value = original.get!.call(this);
          record("read", name, this, value);
          return value;
        },
        set:
          original.set === undefined
            ? undefined
            : function (this: Element, value: unknown) {
                record("write", name, this, value);
                original.set!.call(this, value);
              }
      });
    }
  };
  wrap(elementProto, ["clientWidth", "clientHeight", "clientLeft", "clientTop"]);
  wrap(elementProto, ["scrollWidth", "scrollHeight", "scrollLeft", "scrollTop"]);
  wrap(w.HTMLElement.prototype, ["offsetWidth", "offsetHeight", "offsetLeft", "offsetTop"]);
  for (const name of ["scrollTo", "scrollBy", "scrollIntoView"]) {
    const original = elementProto[name];
    elementProto[name] = function (this: Element, ...args: unknown[]) {
      record("write", name, this, args);
      return original.apply(this, args);
    };
  }
}
