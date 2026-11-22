import { toTree } from "../cve1";

// The `form` step; oracle/README.md, "Form vectors", documents ops, paths and records.

export interface FormProbe {
  markup: string;
  ops?: unknown[][];
  read: string[];
}

function resolve(root: any, path: string): [any, string] {
  const parts = path.split(".");
  let target = root;
  for (const part of parts.slice(0, -1)) target = /^\d+$/.test(part) ? target[Number(part)] : target[part];
  return [target, parts[parts.length - 1]];
}

function errorText(error: unknown): string {
  return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

export function formProbe(probe: FormProbe, write: (record: object) => void) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  container.innerHTML = probe.markup;
  const el: any = container.firstElementChild;
  if (el === null) throw new Error(`form: ${JSON.stringify(probe.markup)} parses to no element`);
  const values = () =>
    probe.read.map((path) => {
      try {
        const [target, key] = resolve(el, path);
        return { path, value: toTree(target[key]) };
      } catch (error) {
        return { path, threw: toTree(errorText(error)) };
      }
    });
  write({ form: probe.markup, op: null, values: values() });
  for (const op of probe.ops ?? []) {
    const [kind, ...args] = op as [string, ...any[]];
    if (!["set", "setAttribute", "appendElement", "remove"].includes(kind)) {
      throw new Error(`form: unknown op ${JSON.stringify(op)}`);
    }
    let threw: unknown = undefined;
    try {
      if (kind === "set") {
        const [target, key] = resolve(el, args[0]);
        target[key] = args[1];
      } else if (kind === "setAttribute") el.setAttribute(args[0], args[1]);
      else if (kind === "appendElement") {
        const child: any = document.createElement(args[0]);
        for (const [path, value] of args[1] ?? []) {
          const [target, key] = resolve(child, path);
          target[key] = value;
        }
        el.appendChild(child);
      } else el.removeChild(el.children[args[0]]);
    } catch (error) {
      threw = errorText(error);
    }
    write({ form: probe.markup, op: toTree(op), ...(threw === undefined ? {} : { threw: toTree(threw) }), values: values() });
  }
  container.remove();
}
