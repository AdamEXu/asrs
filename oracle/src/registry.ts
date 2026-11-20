import { NPM_MODULES } from "oracle:npm-modules";
import { UPSTREAM_MODULES } from "oracle:upstream-modules";
import { HELPERS } from "./helpers";

const METHOD_RECEIVER = Symbol("method receiver");

export type Callable = { fn: Function; receiver: unknown; construct: boolean };

function walk(root: any, segments: string[], name: string): { parent: any; value: any } {
  let parent: any = undefined;
  let value = root;
  for (const segment of segments) {
    if (value === null || value === undefined) throw new Error(`fn ${name}: ${segment} is not reachable`);
    parent = value;
    value = value[segment];
  }
  return { parent, value };
}

async function loadRoot(target: string, name: string): Promise<{ root: any; path: string }> {
  if (target.startsWith("oracle:")) return { root: HELPERS, path: target.slice("oracle:".length) };
  if (target.startsWith("npm:")) {
    const rest = target.slice("npm:".length);
    const colon = rest.indexOf(":");
    const pkg = colon < 0 ? rest : rest.slice(0, colon);
    const load = NPM_MODULES[pkg];
    if (load === undefined) throw new Error(`fn ${name}: npm package ${pkg} is not in the oracle bundle`);
    return { root: await load(), path: colon < 0 ? "" : rest.slice(colon + 1) };
  }
  const colon = target.indexOf(":");
  if (colon >= 0) {
    const id = target.slice(0, colon);
    const load = UPSTREAM_MODULES[id];
    if (load === undefined) throw new Error(`fn ${name}: upstream module ${id} is not in the oracle bundle`);
    return { root: await load(), path: target.slice(colon + 1) };
  }
  return { root: globalThis, path: target };
}

const cache = new Map<string, Callable>();

export async function resolve(name: string): Promise<Callable> {
  const cached = cache.get(name);
  if (cached !== undefined) return cached;
  const construct = name.startsWith("new ");
  const target = construct ? name.slice(4) : name;
  const { root, path } = await loadRoot(target, name);
  const segments = path === "" ? [] : path.split(".");
  const { parent, value } = walk(root, segments, name);
  if (typeof value !== "function") throw new Error(`fn ${name}: not a function`);
  const isMethod = segments.length >= 2 && segments[segments.length - 2] === "prototype";
  const callable: Callable = {
    fn: value,
    receiver: isMethod ? METHOD_RECEIVER : parent,
    construct
  };
  cache.set(name, callable);
  return callable;
}

export async function invoke(name: string, args: unknown[]): Promise<unknown> {
  const { fn, receiver, construct } = await resolve(name);
  if (construct) return Reflect.construct(fn, args);
  if (receiver === METHOD_RECEIVER) {
    if (args.length === 0) throw new Error(`fn ${name}: prototype methods take the receiver as the first argument`);
    return await Reflect.apply(fn, args[0], args.slice(1));
  }
  return await Reflect.apply(fn, receiver, args);
}

export function listModules(): { upstream: string[]; npm: string[]; helpers: string[] } {
  return {
    upstream: Object.keys(UPSTREAM_MODULES),
    npm: Object.keys(NPM_MODULES),
    helpers: Object.keys(HELPERS)
  };
}
