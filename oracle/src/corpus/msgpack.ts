import { Bytes, ByteSource, concat, hex, toBytes } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";
import { LogSpec } from "./wpilog";

type Form = "fix" | "8" | "16" | "32";
type IntForm = "fix" | "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64";

const INT_CODES: { [form: string]: [number, number] } = {
  u8: [0xcc, 1],
  u16: [0xcd, 2],
  u32: [0xce, 4],
  u64: [0xcf, 8],
  i8: [0xd0, 1],
  i16: [0xd1, 2],
  i32: [0xd2, 4],
  i64: [0xd3, 8]
};

function sized(fix: number | null, codes: [number, number, number], length: number, form: Form): Bytes {
  const b = new Bytes();
  if (form === "fix" && fix !== null) return b.u8(fix | length);
  const [code, width] = form === "8" ? [codes[0], 1] : form === "16" ? [codes[1], 2] : [codes[2], 4];
  return b.u8(code).be(length, width);
}

export const mp = {
  nil: () => Uint8Array.of(0xc0),
  bool: (v: boolean) => Uint8Array.of(v ? 0xc3 : 0xc2),
  int: (v: bigint | number, form: IntForm = "fix") => {
    if (form === "fix") return Uint8Array.of(Number(BigInt.asUintN(8, BigInt(v))));
    const [code, width] = INT_CODES[form];
    return new Bytes().u8(code).be(v, width).finish();
  },
  f32: (x: number) => new Bytes().u8(0xca).f32(x, false).finish(),
  f64: (x: number) => new Bytes().u8(0xcb).f64(x, false).finish(),
  f64Bits: (bits: bigint) => new Bytes().u8(0xcb).be(bits, 8).finish(),
  str: (s: ByteSource, form?: Form) => {
    const bytes = toBytes(s);
    const f = form ?? (bytes.length < 32 ? "fix" : bytes.length < 256 ? "8" : bytes.length < 65536 ? "16" : "32");
    return sized(0xa0, [0xd9, 0xda, 0xdb], bytes.length, f).raw(bytes).finish();
  },
  bin: (data: ByteSource, form: Form = "8") => {
    const bytes = toBytes(data);
    return sized(null, [0xc4, 0xc5, 0xc6], bytes.length, form).raw(bytes).finish();
  },
  arr: (items: Uint8Array[], form: Form = items.length < 16 ? "fix" : "16") =>
    concat(sized(0x90, [0, 0xdc, 0xdd], items.length, form).finish(), ...items),
  map: (pairs: [Uint8Array, Uint8Array][], form: Form = pairs.length < 16 ? "fix" : "16") =>
    concat(sized(0x80, [0, 0xde, 0xdf], pairs.length, form).finish(), ...pairs.flat()),
  ext: (type: number, data: ByteSource, form?: Form) => {
    const bytes = toBytes(data);
    const fixCodes: { [n: number]: number } = { 1: 0xd4, 2: 0xd5, 4: 0xd6, 8: 0xd7, 16: 0xd8 };
    const f = form ?? (bytes.length in fixCodes ? "fix" : "8");
    const b = new Bytes();
    if (f === "fix") b.u8(fixCodes[bytes.length]);
    else {
      const width = f === "8" ? 1 : f === "16" ? 2 : 4;
      b.u8(width === 1 ? 0xc7 : width === 2 ? 0xc8 : 0xc9).be(bytes.length, width);
    }
    return b.u8(type).raw(bytes).finish();
  }
};

const k = (s: string) => mp.str(s);

class MsgpackCase {
  readonly spec = new LogSpec();
  private ids = new Map<string, number>();
  private t = 1000;

  put(key: string, bytes: ByteSource) {
    if (!this.ids.has(key)) this.ids.set(key, this.spec.entry(key, "msgpack"));
    this.t += 20000;
    this.spec.put(this.ids.get(key)!, this.t, bytes);
    return this;
  }
}

function randomValue(r: Rng, depth: number): Uint8Array {
  const roll = r.below(depth > 2 ? 7 : 10);
  switch (roll) {
    case 0:
      return mp.nil();
    case 1:
      return mp.bool(r.bool());
    case 2:
      return mp.int(
        r.u64() >> BigInt(r.below(64)),
        r.pick(["u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64"] as IntForm[])
      );
    case 3:
      return r.bool() ? mp.f32(r.double()) : mp.f64(r.double());
    case 4:
      return mp.str(r.ascii(8));
    case 5:
      return mp.bin(r.bytes(r.below(5)));
    case 6:
      return mp.int(r.range(-32, 127));
    case 7:
      return mp.arr(Array.from({ length: r.below(5) }, () => randomValue(r, depth + 1)));
    case 8:
      return mp.map(
        Array.from({ length: r.below(5) }, () => [
          mp.str(r.pick(["a", "b", "1", "10", r.ascii(3)])),
          randomValue(r, depth + 1)
        ])
      );
    default:
      return mp.ext(r.range(-3, 5), r.bytes(r.pick([1, 2, 3, 4, 8, 16])));
  }
}

export function msgpackMatrix(corpus: Corpus) {
  {
    const c = new MsgpackCase();
    const ints: [string, Uint8Array][] = [
      ["fix0", mp.int(0)],
      ["fix127", mp.int(127)],
      ["negfix1", mp.int(-1)],
      ["negfix32", mp.int(-32)],
      ["u8", mp.int(255, "u8")],
      ["u16", mp.int(65535, "u16")],
      ["u32", mp.int(2 ** 32 - 1, "u32")],
      ["u64small", mp.int(5, "u64")],
      ["u64p53", mp.int(2n ** 53n + 1n, "u64")],
      ["u64max", mp.int(2n ** 64n - 1n, "u64")],
      ["i8", mp.int(-128, "i8")],
      ["i16", mp.int(-32768, "i16")],
      ["i32", mp.int(-(2 ** 31), "i32")],
      ["i64min", mp.int(-(2n ** 63n), "i64")],
      ["i64neg53", mp.int(-(2n ** 53n) - 1n, "i64")],
      ["i64one", mp.int(1, "i64")],
      ["f32nan", mp.f32(NaN)],
      ["f32negzero", mp.f32(-0)],
      ["f32tenth", mp.f32(0.1)],
      ["f32inf", mp.f32(-Infinity)],
      ["f64nanpayload", mp.f64Bits(0x7ff8000000000001n)],
      ["f64negzero", mp.f64(-0)],
      ["f64min", mp.f64(Number.MIN_VALUE)],
      ["true", mp.bool(true)],
      ["false", mp.bool(false)],
      ["nil", mp.nil()]
    ];
    c.put(
      "/Msgpack/Numbers",
      mp.map(
        ints.map(([name, v]) => [k(name), v]),
        "16"
      )
    );
    ints.forEach(([, v]) => c.put("/Msgpack/Root", v));
    c.put(
      "/Msgpack/NumberArray",
      mp.arr(
        ints.slice(0, 16).map(([, v]) => v),
        "16"
      )
    );
    c.put("/Msgpack/BoolArray", mp.arr([mp.bool(true), mp.bool(false)]));
    c.put("/Msgpack/StringArray", mp.arr([mp.str("a"), mp.str("b")]));
    corpus.spec(
      "payload/msgpack/numbers",
      "every integer and float width, values past 2^53, NaN and signed zero; root primitives and arrays",
      c.spec
    );
  }
  {
    const c = new MsgpackCase();
    const long = (fill: ByteSource) => concat(...Array.from({ length: 60 }, () => toBytes(fill)));
    const strs: [string, Uint8Array][] = [
      ["empty", mp.str("")],
      ["fix", mp.str("hello")],
      ["str8", mp.str("x".repeat(40), "8")],
      ["str16", mp.str("y", "16")],
      ["str32", mp.str("z", "32")],
      ["unicode", mp.str("é中\u{1f916}")],
      ["badShort", mp.str(hex("c3 28 ff ed a0 80"))],
      ["badLong", mp.str(long(hex("61 c3 28 ff")))],
      ["bomShort", mp.str(hex("ef bb bf 61"))],
      ["bomLong", mp.str(long(hex("ef bb bf 62")))],
      ["surrogateLong", mp.str(long(hex("ed a0 80 63")))],
      ["nul", mp.str(hex("00 61 00"))],
      ["bin8", mp.bin(hex("00 01 02"))],
      ["bin16", mp.bin(hex("ff"), "16")],
      ["bin32", mp.bin(new Uint8Array(0), "32")]
    ];
    c.put(
      "/Msgpack/Strings",
      mp.map(
        strs.map(([name, v]) => [k(name), v]),
        "16"
      )
    );
    c.put(
      "/Msgpack/StringKeys",
      mp.map([
        [mp.str(hex("ff")), mp.int(1)],
        [mp.str(hex("ef bb bf 6b")), mp.int(2)]
      ])
    );
    corpus.spec(
      "payload/msgpack/strings",
      "str and bin of every width; invalid UTF-8 and BOMs on both sides of the 200-byte decoder threshold",
      c.spec
    );
  }
  {
    const c = new MsgpackCase();
    const keys = ["10", "2", "a", "0", "01", "-1", "4294967295", "4294967294", "1.5", ""];
    c.put("/Msgpack/Keys", mp.map(keys.map((s, i): [Uint8Array, Uint8Array] => [k(s), mp.int(i)])));
    c.put(
      "/Msgpack/NumericKeys",
      mp.map([
        [mp.int(5), mp.str("five")],
        [mp.int(-1), mp.str("neg")],
        [mp.f64(1.5), mp.str("half")],
        [k("b"), mp.int(2)],
        [mp.int(0, "u64"), mp.nil()]
      ])
    );
    c.put(
      "/Msgpack/Duplicates",
      mp.map([
        [k("a"), mp.int(1)],
        [k("b"), mp.int(2)],
        [k("a"), mp.int(3)]
      ])
    );
    c.put(
      "/Msgpack/Proto",
      mp.map([
        [k("__proto__"), mp.int(1)],
        [k("x"), mp.int(2)]
      ])
    );
    c.put("/Msgpack/BadKey", mp.map([[mp.nil(), mp.int(1)]]));
    c.put("/Msgpack/ArrayKey", mp.map([[mp.arr([]), mp.int(1)]]));
    c.put(
      "/Msgpack/Nested",
      mp.map([
        [k("a"), mp.map([[k("b"), mp.arr([mp.int(1), mp.map([[k("c"), mp.bool(true)]]), mp.nil(), mp.arr([])])]])],
        [k("e"), mp.map([], "32")]
      ])
    );
    c.put("/Msgpack/Mixed", mp.arr([mp.int(1), mp.str("s"), mp.bool(false), mp.nil(), mp.bin(hex("aa"))], "32"));
    c.put("/Msgpack/EmptyArray", mp.arr([])).put("/Msgpack/EmptyMap", mp.map([], "16"));
    c.put("/Msgpack/Changing", mp.map([[k("v"), mp.int(1)]])).put(
      "/Msgpack/Changing",
      mp.map([[k("v"), mp.str("now a string")]])
    );
    c.put("/Msgpack/Changing", mp.map([[k("v"), mp.arr([mp.bool(true)])]])).put(
      "/Msgpack/Changing",
      mp.map([[k("v"), mp.arr([])]])
    );
    corpus.spec(
      "payload/msgpack/maps",
      "integer-like and numeric keys, duplicates, __proto__, invalid key types, nesting, type changes",
      c.spec
    );
  }
  {
    const c = new MsgpackCase();
    const exts: [string, Uint8Array][] = [
      ["fix1", mp.ext(1, hex("aa"))],
      ["fix2", mp.ext(2, hex("aa bb"))],
      ["fix4", mp.ext(127, hex("01 02 03 04"))],
      ["fix8", mp.ext(-2, new Uint8Array(8))],
      ["fix16", mp.ext(5, new Uint8Array(16).fill(7))],
      ["ext8", mp.ext(3, hex("01 02 03"))],
      ["ext16", mp.ext(4, hex("ff"), "16")],
      ["ext32", mp.ext(-128, new Uint8Array(0), "32")],
      ["ts32", mp.ext(-1, hex("5f 5e 10 00"))],
      ["ts64", mp.ext(-1, new Bytes().be((123456789n << 34n) | 1700000000n, 8).finish())],
      ["ts96", mp.ext(-1, new Bytes().be(999999999, 4).be(-62135596800n, 8).finish())]
    ];
    c.put(
      "/Msgpack/Ext",
      mp.map(
        exts.map(([name, v]) => [k(name), v]),
        "16"
      )
    );
    exts.forEach(([, v]) => c.put("/Msgpack/RootExt", v));
    c.put("/Msgpack/BadTimestamp", mp.map([[k("t"), mp.ext(-1, hex("01 02 03"))]]));
    c.put("/Msgpack/ExtArray", mp.arr([mp.ext(1, hex("01")), mp.ext(-1, hex("00 00 00 01"))]));
    corpus.spec(
      "payload/msgpack/ext",
      "fixext and ext of every width, positive and negative types, timestamp ext in 32/64/96-bit forms",
      c.spec
    );
  }
  {
    const c = new MsgpackCase();
    const valid = mp.map([[k("ok"), mp.int(1)]]);
    [
      concat(valid, hex("00")),
      valid.slice(0, 3),
      hex("c1"),
      new Uint8Array(0),
      mp.nil(),
      concat(mp.str("a"), mp.str("b")),
      hex("dc 00 05 01"),
      hex("cf 00 00"),
      hex("df ff ff ff ff"),
      hex("a5 61 62"),
      concat(hex("81"), mp.str("k"))
    ].forEach((p) => c.put("/Msgpack/Malformed", p));
    c.put("/Msgpack/Malformed", valid);
    corpus.spec(
      "payload/msgpack/malformed",
      "trailing bytes, truncation, the reserved 0xc1 byte, empty payloads and oversized counts",
      c.spec
    );
  }
  for (let n = 0; n < 2; n++) {
    const id = `payload/msgpack/random-${n}`;
    const r = corpus.rng(id);
    const c = new MsgpackCase();
    for (let i = 0; i < 30; i++) {
      const pairs: [Uint8Array, Uint8Array][] = Array.from({ length: r.range(1, 6) }, () => [
        mp.str(r.pick(["x", "y", "0", "7", "n"])),
        randomValue(r, 0)
      ]);
      c.put(`/Msgpack/Random/${r.below(3)}`, mp.map(pairs));
    }
    corpus.spec(id, "seeded random msgpack maps with nested arrays, maps, ext, bin and every number width", c.spec);
  }
}
