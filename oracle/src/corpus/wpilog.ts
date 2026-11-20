import { encode } from "../cve1";
import { ByteSource, Bytes, minimalWidth, toBytes } from "./bytes";

export interface RecordOptions {
  entryLen?: number;
  sizeLen?: number;
  tsLen?: number;
  /** Declared payload size, when it should disagree with the payload actually written. */
  size?: number | bigint;
  /** Sets the unused top bit of the length bitfield. */
  spareBit?: boolean;
}

export interface HeaderOptions {
  magic?: ByteSource;
  version?: number;
  extra?: ByteSource;
  /** Declared extra header length, when it should disagree with the bytes written. */
  extraLen?: number;
}

/**
 * Builds WPILOG bytes record by record with every width and length under the caller's control,
 * for files upstream's WPILOGEncoder cannot produce.
 */
export class WpilogBuilder {
  private out = new Bytes();
  readonly recordStarts: number[] = [];

  constructor(header: HeaderOptions | null = {}) {
    if (header === null) return;
    const extra = toBytes(header.extra ?? "");
    this.out
      .raw(header.magic ?? "WPILOG")
      .le(header.version ?? 0x0100, 2)
      .le(header.extraLen ?? extra.length, 4)
      .raw(extra);
  }

  record(entry: number | bigint, timestamp: number | bigint, payload: ByteSource, opts: RecordOptions = {}): this {
    const data = toBytes(payload);
    const size = opts.size ?? data.length;
    const ts = typeof timestamp === "number" ? BigInt(Math.floor(timestamp)) : timestamp;
    const entryLen = opts.entryLen ?? minimalWidth(entry, 4);
    const sizeLen = opts.sizeLen ?? minimalWidth(size, 4);
    const tsLen = opts.tsLen ?? minimalWidth(ts, 8);
    if (entryLen > 4 || sizeLen > 4 || tsLen > 8) throw new Error("record field too wide");
    this.recordStarts.push(this.out.length);
    this.out
      .u8((entryLen - 1) | ((sizeLen - 1) << 2) | ((tsLen - 1) << 4) | (opts.spareBit ? 0x80 : 0))
      .le(entry, entryLen)
      .le(size, sizeLen)
      .le(ts, tsLen)
      .raw(data);
    return this;
  }

  start(
    timestamp: number | bigint,
    entry: number,
    name: ByteSource,
    type: ByteSource,
    metadata: ByteSource = "",
    opts: RecordOptions = {}
  ): this {
    const payload = new Bytes().u8(0).le(entry, 4).lenPrefixed(name).lenPrefixed(type).lenPrefixed(metadata);
    return this.record(0, timestamp, payload.finish(), opts);
  }

  finish(timestamp: number | bigint, entry: number, opts: RecordOptions = {}): this {
    return this.record(0, timestamp, new Bytes().u8(1).le(entry, 4).finish(), opts);
  }

  setMetadata(timestamp: number | bigint, entry: number, metadata: ByteSource, opts: RecordOptions = {}): this {
    return this.record(0, timestamp, new Bytes().u8(2).le(entry, 4).lenPrefixed(metadata).finish(), opts);
  }

  raw(bytes: ByteSource): this {
    this.out.raw(bytes);
    return this;
  }

  bytes(): Uint8Array {
    return this.out.finish();
  }
}

export const payload = {
  boolean: (v: boolean) => Uint8Array.of(v ? 1 : 0),
  int64: (v: bigint | number) => new Bytes().le(v, 8).finish(),
  float: (v: number) => new Bytes().f32(v).finish(),
  double: (v: number) => new Bytes().f64(v).finish(),
  string: (v: ByteSource) => toBytes(v),
  booleanArray: (v: boolean[]) => Uint8Array.from(v.map((b) => (b ? 1 : 0))),
  int64Array: (v: (bigint | number)[]) => {
    const b = new Bytes();
    v.forEach((x) => b.le(x, 8));
    return b.finish();
  },
  floatArray: (v: number[]) => {
    const b = new Bytes();
    v.forEach((x) => b.f32(x));
    return b.finish();
  },
  doubleArray: (v: number[]) => {
    const b = new Bytes();
    v.forEach((x) => b.f64(x));
    return b.finish();
  },
  stringArray: (v: ByteSource[], count = v.length) => {
    const b = new Bytes().le(count, 4);
    v.forEach((s) => b.lenPrefixed(s));
    return b.finish();
  }
};

/**
 * One WPILOG file plus, for payload cases, the Log write sequence wpilogWorker's parseField
 * makes for each data record, in file order. Each ops line is `{"fn": <Log method>, "args": [CVE-1...]}`.
 */
export class LogSpec {
  readonly b: WpilogBuilder;
  readonly ops: string[] = [];
  private nextId = 1;
  private types = new Map<number, { name: string; type: string }>();

  constructor(header: HeaderOptions = {}) {
    this.b = new WpilogBuilder(header);
  }

  entry(name: string, type: string, metadata = "", timestamp = 0): number {
    const id = this.nextId++;
    this.b.start(timestamp, id, name, type, metadata);
    this.types.set(id, { name, type });
    return id;
  }

  put(id: number, timestampUs: number, data: ByteSource): this {
    const bytes = toBytes(data);
    this.b.record(id, timestampUs, bytes);
    const { name, type } = this.types.get(id)!;
    const t = Math.floor(timestampUs) / 1000000.0;
    this.ops.push(opFor(name, type, t, bytes));
    return this;
  }

  bytes(): Uint8Array {
    return this.b.bytes();
  }
}

function op(fn: string, ...args: unknown[]): string {
  return `{"fn":${JSON.stringify(fn)},"args":[${args.map((a) => encode(a)).join(",")}]}`;
}

function opFor(key: string, type: string, t: number, bytes: Uint8Array): string {
  if (type === "json") return op("putJSON", key, t, new TextDecoder().decode(bytes));
  if (type === "msgpack") return op("putMsgpack", key, t, bytes);
  if (type.startsWith("struct:")) {
    const schema = type.split("struct:")[1];
    return schema.endsWith("[]")
      ? op("putStruct", key, t, bytes, schema.slice(0, -2), true)
      : op("putStruct", key, t, bytes, schema, false);
  }
  if (type.startsWith("photonstruct:")) return op("putPhotonStruct", key, t, bytes, type.split("photonstruct:")[1]);
  if (type.startsWith("proto:")) return op("putProto", key, t, bytes, type.split("proto:")[1]);
  return op("putRaw", key, t, bytes);
}
