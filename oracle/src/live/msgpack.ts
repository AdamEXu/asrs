// The mock servers' own MessagePack writer and reader, mirrored by `asrs_parity::live::msgpack`.
// Numbers follow @msgpack/msgpack's rules: a safe integer takes the smallest integer form, any
// other number is a float64 unless the caller asks for float32.

export type Packed =
  | { nil: null }
  | { bool: boolean }
  | { int: number }
  | { f32: number }
  | { f64: number }
  | { str: string }
  | { bin: Uint8Array }
  | { arr: Packed[] }
  | { raw: Uint8Array };

export function number(n: number): Packed {
  return Number.isSafeInteger(n) ? { int: n } : { f64: n };
}

class Writer {
  bytes: number[] = [];

  u8(...values: number[]): void {
    this.bytes.push(...values);
  }

  be(value: number, width: number): void {
    for (let i = width - 1; i >= 0; i--) this.bytes.push(Math.floor(value / 2 ** (8 * i)) % 256);
  }

  float(value: number, width: 4 | 8): void {
    const view = new DataView(new ArrayBuffer(width));
    if (width === 4) view.setFloat32(0, value);
    else view.setFloat64(0, value);
    this.bytes.push(...new Uint8Array(view.buffer));
  }
}

function writeInt(w: Writer, n: number): void {
  if (n >= 0) {
    if (n < 0x80) w.u8(n);
    else if (n < 0x100) w.u8(0xcc, n);
    else if (n < 0x10000) (w.u8(0xcd), w.be(n, 2));
    else if (n < 0x100000000) (w.u8(0xce), w.be(n, 4));
    else (w.u8(0xcf), w.be(n, 8));
  } else if (n >= -0x20) {
    w.u8(n & 0xff);
  } else if (n >= -0x80) {
    w.u8(0xd0, n & 0xff);
  } else if (n >= -0x8000) {
    w.u8(0xd1);
    w.be(n + 0x10000, 2);
  } else if (n >= -0x80000000) {
    w.u8(0xd2);
    w.be(n + 0x100000000, 4);
  } else {
    w.u8(0xd3);
    const bits = BigInt.asUintN(64, BigInt(n));
    w.be(Number(bits >> 32n), 4);
    w.be(Number(bits & 0xffffffffn), 4);
  }
}

function writeLength(w: Writer, length: number, small: number | null, smallLimit: number, codes: number[]): void {
  if (small !== null && length < smallLimit) w.u8(small | length);
  else if (codes[0] !== -1 && length < 0x100) w.u8(codes[0], length);
  else if (length < 0x10000) (w.u8(codes[1]), w.be(length, 2));
  else (w.u8(codes[2]), w.be(length, 4));
}

function write(w: Writer, value: Packed): void {
  if ("nil" in value) w.u8(0xc0);
  else if ("bool" in value) w.u8(value.bool ? 0xc3 : 0xc2);
  else if ("int" in value) {
    if (!Number.isSafeInteger(value.int)) throw new Error(`msgpack: ${value.int} is not a safe integer`);
    writeInt(w, value.int);
  } else if ("f32" in value) (w.u8(0xca), w.float(value.f32, 4));
  else if ("f64" in value) (w.u8(0xcb), w.float(value.f64, 8));
  else if ("str" in value) {
    const bytes = new TextEncoder().encode(value.str);
    writeLength(w, bytes.length, 0xa0, 32, [0xd9, 0xda, 0xdb]);
    w.u8(...bytes);
  } else if ("bin" in value) {
    writeLength(w, value.bin.length, null, 0, [0xc4, 0xc5, 0xc6]);
    w.u8(...value.bin);
  } else if ("arr" in value) {
    writeLength(w, value.arr.length, 0x90, 16, [-1, 0xdc, 0xdd]);
    value.arr.forEach((item) => write(w, item));
  } else w.u8(...value.raw);
}

export function encode(values: Packed[]): Uint8Array {
  const w = new Writer();
  values.forEach((value) => write(w, value));
  return new Uint8Array(w.bytes);
}

/** A decoded value: maps and extensions are read past and come back as `undefined`. */
export type Unpacked = null | undefined | boolean | number | string | Uint8Array | Unpacked[];

class Reader {
  pos = 0;
  constructor(private bytes: Uint8Array) {}

  u8(): number {
    if (this.pos >= this.bytes.length) throw new Error("msgpack: truncated");
    return this.bytes[this.pos++];
  }

  be(width: number): number {
    let v = 0;
    for (let i = 0; i < width; i++) v = v * 256 + this.u8();
    return v;
  }

  take(n: number): Uint8Array {
    if (this.pos + n > this.bytes.length) throw new Error("msgpack: truncated");
    const out = this.bytes.slice(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }

  float(width: 4 | 8): number {
    const view = new DataView(this.take(width).buffer);
    return width === 4 ? view.getFloat32(0) : view.getFloat64(0);
  }

  signed(width: number): number {
    const v = this.be(width);
    return v >= 2 ** (8 * width - 1) ? v - 2 ** (8 * width) : v;
  }

  int64(): number {
    const hi = this.be(4);
    const lo = this.be(4);
    return (hi >= 0x80000000 ? hi - 0x100000000 : hi) * 0x100000000 + lo;
  }

  value(): Unpacked {
    const b = this.u8();
    if (b < 0x80) return b;
    if (b >= 0xe0) return b - 0x100;
    if (b >= 0xa0 && b < 0xc0) return new TextDecoder().decode(this.take(b & 0x1f));
    if (b >= 0x90 && b < 0xa0) return this.array(b & 0x0f);
    if (b >= 0x80 && b < 0x90) return this.skipMap(b & 0x0f);
    switch (b) {
      case 0xc0:
        return null;
      case 0xc2:
        return false;
      case 0xc3:
        return true;
      case 0xc4:
      case 0xc5:
      case 0xc6:
        return this.take(this.be(1 << (b - 0xc4)));
      case 0xc7:
      case 0xc8:
      case 0xc9: {
        const n = this.be(1 << (b - 0xc7));
        this.take(1 + n);
        return undefined;
      }
      case 0xca:
        return this.float(4);
      case 0xcb:
        return this.float(8);
      case 0xcc:
      case 0xcd:
      case 0xce:
      case 0xcf:
        return this.be(1 << (b - 0xcc));
      case 0xd0:
      case 0xd1:
      case 0xd2:
        return this.signed(1 << (b - 0xd0));
      case 0xd3:
        return this.int64();
      case 0xd4:
      case 0xd5:
      case 0xd6:
      case 0xd7:
      case 0xd8:
        this.take(1 + (1 << (b - 0xd4)));
        return undefined;
      case 0xd9:
      case 0xda:
      case 0xdb:
        return new TextDecoder().decode(this.take(this.be(1 << (b - 0xd9))));
      case 0xdc:
      case 0xdd:
        return this.array(this.be(b === 0xdc ? 2 : 4));
      case 0xde:
      case 0xdf:
        return this.skipMap(this.be(b === 0xde ? 2 : 4));
    }
    throw new Error(`msgpack: bad byte ${b}`);
  }

  array(n: number): Unpacked[] {
    const out: Unpacked[] = [];
    for (let i = 0; i < n; i++) out.push(this.value());
    return out;
  }

  skipMap(n: number): undefined {
    for (let i = 0; i < 2 * n; i++) this.value();
    return undefined;
  }
}

/** Every value in `bytes` up to the first one that does not decode. */
export function decodeAll(bytes: Uint8Array): Unpacked[] {
  const reader = new Reader(bytes);
  const out: Unpacked[] = [];
  while (reader.pos < bytes.length) {
    try {
      out.push(reader.value());
    } catch {
      break;
    }
  }
  return out;
}
