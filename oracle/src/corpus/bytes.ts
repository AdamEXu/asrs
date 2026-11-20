const ENCODER = new TextEncoder();

export type ByteSource = Uint8Array | number[] | string;

export function utf8(text: string): Uint8Array {
  return ENCODER.encode(text);
}

export function hex(text: string): Uint8Array {
  const clean = text.replace(/[\s_-]/g, "");
  if (clean.length % 2 !== 0 || /[^0-9a-f]/i.test(clean)) throw new Error(`bad hex ${text}`);
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function toBytes(source: ByteSource): Uint8Array {
  if (typeof source === "string") return utf8(source);
  return source instanceof Uint8Array ? source : Uint8Array.from(source);
}

export function concat(...parts: ByteSource[]): Uint8Array {
  const arrays = parts.map(toBytes);
  const out = new Uint8Array(arrays.reduce((n, a) => n + a.length, 0));
  let offset = 0;
  for (const a of arrays) {
    out.set(a, offset);
    offset += a.length;
  }
  return out;
}

export function bitsToF64(bits: bigint): number {
  const view = new DataView(new ArrayBuffer(8));
  view.setBigUint64(0, BigInt.asUintN(64, bits));
  return view.getFloat64(0);
}

export class Bytes {
  private data: number[] = [];

  get length(): number {
    return this.data.length;
  }

  u8(value: number): this {
    this.data.push(value & 0xff);
    return this;
  }

  le(value: bigint | number, width: number): this {
    let v = BigInt.asUintN(width * 8, BigInt(value));
    for (let i = 0; i < width; i++) {
      this.data.push(Number(v & 0xffn));
      v >>= 8n;
    }
    return this;
  }

  be(value: bigint | number, width: number): this {
    const start = this.data.length;
    this.le(value, width);
    const tail = this.data.splice(start, width).reverse();
    this.data.push(...tail);
    return this;
  }

  f32(value: number, littleEndian = true): this {
    const view = new DataView(new ArrayBuffer(4));
    view.setFloat32(0, value, littleEndian);
    return this.raw(new Uint8Array(view.buffer));
  }

  f64(value: number, littleEndian = true): this {
    const view = new DataView(new ArrayBuffer(8));
    view.setFloat64(0, value, littleEndian);
    return this.raw(new Uint8Array(view.buffer));
  }

  f32Bits(bits: number): this {
    return this.le(bits >>> 0, 4);
  }

  f64Bits(bits: bigint): this {
    return this.le(bits, 8);
  }

  raw(source: ByteSource): this {
    for (const b of toBytes(source)) this.data.push(b);
    return this;
  }

  lenPrefixed(source: ByteSource): this {
    const bytes = toBytes(source);
    return this.le(bytes.length, 4).raw(bytes);
  }

  finish(): Uint8Array {
    return Uint8Array.from(this.data);
  }
}

export function minimalWidth(value: bigint | number, max = 8): number {
  let v = BigInt.asUintN(64, BigInt(value));
  let width = 1;
  while (v > 0xffn && width < max) {
    v >>= 8n;
    width++;
  }
  return width;
}
