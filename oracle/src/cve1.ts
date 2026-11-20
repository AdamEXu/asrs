export type Cve1 = null | boolean | string | Cve1[] | { [tag: string]: unknown };

const scratch = new DataView(new ArrayBuffer(8));

export function numberToBits(value: number): string {
  scratch.setFloat64(0, value);
  return (
    scratch.getUint32(0).toString(16).padStart(8, "0") + scratch.getUint32(4).toString(16).padStart(8, "0")
  );
}

export function bitsToNumber(bits: string): number {
  if (!/^[0-9a-f]{16}$/.test(bits)) throw new Error(`CVE-1: bad number bits ${JSON.stringify(bits)}`);
  scratch.setUint32(0, parseInt(bits.slice(0, 8), 16));
  scratch.setUint32(4, parseInt(bits.slice(8), 16));
  return scratch.getFloat64(0);
}

function bytesToHex(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i++) out += bytes[i].toString(16).padStart(2, "0");
  return out;
}

function hexToBytes(hex: string): Uint8Array {
  if (!/^(?:[0-9a-f]{2})*$/.test(hex)) throw new Error("CVE-1: bad hex bytes");
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

const TYPED_ARRAYS: { [name: string]: any } = {
  Int8Array,
  Uint8ClampedArray,
  Int16Array,
  Uint16Array,
  Int32Array,
  Uint32Array,
  Float32Array,
  Float64Array,
  BigInt64Array,
  BigUint64Array
};

function constructorName(value: object): string {
  const proto = Object.getPrototypeOf(value);
  const ctor = proto?.constructor;
  return typeof ctor === "function" && typeof ctor.name === "string" ? ctor.name : "";
}

function isPlainObject(value: object): boolean {
  const proto = Object.getPrototypeOf(value);
  return proto === null || proto === Object.prototype;
}

export function toTree(value: unknown, stack: object[] = []): Cve1 {
  switch (typeof value) {
    case "undefined":
      return { u: null };
    case "boolean":
      return value;
    case "string":
      return value;
    case "number":
      return { n: numberToBits(value) };
    case "bigint":
      return { bi: value.toString() };
    case "symbol":
      return { sym: value.description ?? null };
    case "function":
      return { fn: value.name };
  }
  if (value === null) return null;
  const object = value as object;
  if (stack.includes(object)) throw new Error("CVE-1: cyclic value");
  stack.push(object);
  try {
    return objectToTree(object, stack);
  } finally {
    stack.pop();
  }
}

function pairs(object: object, stack: object[]): Cve1[] {
  return Object.keys(object).map((key) => [key, toTree((object as any)[key], stack)]);
}

function objectToTree(object: object, stack: object[]): Cve1 {
  if (Array.isArray(object)) {
    const out: Cve1[] = [];
    for (let i = 0; i < object.length; i++) {
      out.push(i in object ? toTree(object[i], stack) : { hole: null });
    }
    return out;
  }
  if (object instanceof Uint8Array) return { b: bytesToHex(object) };
  if (ArrayBuffer.isView(object)) {
    if (object instanceof DataView) {
      return { dv: bytesToHex(new Uint8Array(object.buffer, object.byteOffset, object.byteLength)) };
    }
    const typed = object as unknown as ArrayLike<number | bigint>;
    const name = Object.keys(TYPED_ARRAYS).find((n) => object instanceof TYPED_ARRAYS[n]) ?? constructorName(object);
    return { ta: [name, Array.from(typed, (v) => toTree(v, stack))] };
  }
  if (object instanceof ArrayBuffer || (typeof SharedArrayBuffer !== "undefined" && object instanceof SharedArrayBuffer)) {
    return { ab: bytesToHex(new Uint8Array(object)) };
  }
  if (object instanceof Date) return { d: numberToBits(object.getTime()) };
  if (object instanceof Map) {
    return { map: Array.from(object.entries(), ([k, v]) => [toTree(k, stack), toTree(v, stack)]) };
  }
  if (object instanceof Set) return { set: Array.from(object.values(), (v) => toTree(v, stack)) };
  if (object instanceof Error) {
    return { err: [String(object.name), String(object.message)] };
  }
  if (object instanceof Number || object instanceof String || object instanceof Boolean) {
    return { box: toTree(object.valueOf(), stack) };
  }
  if (isPlainObject(object)) return { o: pairs(object, stack) };
  return { c: [constructorName(object), pairs(object, stack)] };
}

export function encode(value: unknown): string {
  return JSON.stringify(toTree(value));
}

function expectArray(value: unknown, what: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`CVE-1: ${what} must be an array`);
  return value;
}

function expectString(value: unknown, what: string): string {
  if (typeof value !== "string") throw new Error(`CVE-1: ${what} must be a string`);
  return value;
}

export function fromTree(tree: unknown): unknown {
  if (tree === null || typeof tree === "boolean" || typeof tree === "string") return tree;
  if (typeof tree === "number") return tree;
  if (Array.isArray(tree)) {
    const out: unknown[] = new Array(tree.length);
    tree.forEach((item, i) => {
      if (item !== null && typeof item === "object" && !Array.isArray(item) && "hole" in item) return;
      out[i] = fromTree(item);
    });
    return out;
  }
  const keys = Object.keys(tree as object);
  if (keys.length !== 1) throw new Error(`CVE-1: tag object must have exactly one key, got ${JSON.stringify(keys)}`);
  const tag = keys[0];
  const body = (tree as any)[tag];
  switch (tag) {
    case "u":
      return undefined;
    case "n":
      return bitsToNumber(expectString(body, "n"));
    case "bi":
      return BigInt(expectString(body, "bi"));
    case "sym":
      return Symbol(body ?? undefined);
    case "o": {
      const out: { [key: string]: unknown } = {};
      for (const pair of expectArray(body, "o")) {
        const [key, value] = expectArray(pair, "o entry");
        Object.defineProperty(out, expectString(key, "o key"), {
          value: fromTree(value),
          enumerable: true,
          writable: true,
          configurable: true
        });
      }
      return out;
    }
    case "b":
      return hexToBytes(expectString(body, "b"));
    case "ab":
      return hexToBytes(expectString(body, "ab")).buffer;
    case "dv":
      return new DataView(hexToBytes(expectString(body, "dv")).buffer);
    case "ta": {
      const [name, items] = expectArray(body, "ta");
      const ctor = TYPED_ARRAYS[expectString(name, "ta name")];
      if (ctor === undefined) throw new Error(`CVE-1: unknown typed array ${name}`);
      return ctor.from(expectArray(items, "ta items").map(fromTree));
    }
    case "d":
      return new Date(bitsToNumber(expectString(body, "d")));
    case "map":
      return new Map(
        expectArray(body, "map").map((entry) => {
          const [k, v] = expectArray(entry, "map entry");
          return [fromTree(k), fromTree(v)] as [unknown, unknown];
        })
      );
    case "set":
      return new Set(expectArray(body, "set").map(fromTree));
    case "err": {
      const [name, message] = expectArray(body, "err");
      const ctor = (globalThis as any)[expectString(name, "err name")];
      const error = typeof ctor === "function" && ctor.prototype instanceof Error ? new ctor(message) : new Error(message);
      if (error.name !== name) error.name = name as string;
      return error;
    }
    case "box":
      return Object(fromTree(body));
    case "hole":
      throw new Error("CVE-1: hole outside an array");
    case "fn":
    case "c":
      throw new Error(`CVE-1: tag ${tag} cannot be decoded`);
  }
  throw new Error(`CVE-1: unknown tag ${JSON.stringify(tag)}`);
}

export function decode(line: string): unknown {
  return fromTree(JSON.parse(line));
}
