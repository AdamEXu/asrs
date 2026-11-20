import { WPILOGEncoder, WPILOGEncoderRecord } from "@upstream/hub/dataSources/wpilog/WPILOGEncoder";
import { Bytes, concat, hex, utf8 } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";
import { WpilogBuilder, payload } from "./wpilog";

const SECOND = 1000000;

type Gen = (rng: Rng) => Uint8Array;

const STANDARD_TYPES: [string, Gen][] = [
  ["boolean", (r) => payload.boolean(r.bool())],
  ["int64", (r) => payload.int64(r.below(4) === 0 ? r.u64() : BigInt(r.range(-100000, 100000)))],
  ["int", (r) => payload.int64(BigInt(r.range(-1000, 1000)))],
  ["float", (r) => payload.float(r.double())],
  ["double", (r) => payload.double(r.double())],
  ["string", (r) => payload.string(r.ascii(16))],
  ["json", (r) => payload.string(JSON.stringify({ a: r.range(0, 9), b: [r.bool(), r.ascii(4)] }))],
  ["msgpack", (r) => Uint8Array.of(0x82, 0xa1, 0x61, r.below(0x80), 0xa1, 0x62, r.bool() ? 0xc3 : 0xc2)],
  ["raw", (r) => r.bytes(r.below(12))],
  ["boolean[]", (r) => payload.booleanArray(Array.from({ length: r.below(6) }, () => r.bool()))],
  ["int64[]", (r) => payload.int64Array(Array.from({ length: r.below(6) }, () => BigInt(r.range(-50, 50))))],
  ["int[]", (r) => payload.int64Array(Array.from({ length: r.below(6) }, () => BigInt(r.range(-50, 50))))],
  ["float[]", (r) => payload.floatArray(Array.from({ length: r.below(6) }, () => r.double()))],
  ["double[]", (r) => payload.doubleArray(Array.from({ length: r.below(6) }, () => r.double()))],
  ["string[]", (r) => payload.stringArray(Array.from({ length: r.below(5) }, () => r.ascii(6)))]
];

const ODD_TYPES = [
  "",
  "Boolean",
  "double ",
  "unknown:type",
  "msgpack[]",
  "structschema",
  "struct:Missing",
  "struct:Missing[]",
  "struct:",
  "proto:Missing.Type",
  "proto:FileDescriptorProto",
  "photonstruct:Missing",
  "photonstructschema",
  "rawBytes",
  "JSON",
  "MessagePack",
  "log:Hoot"
];

const INT64_EXTREMES: bigint[] = [
  0n,
  1n,
  -1n,
  2n ** 53n - 1n,
  2n ** 53n,
  2n ** 53n + 1n,
  2n ** 53n + 3n,
  -(2n ** 53n) - 1n,
  2n ** 62n,
  2n ** 63n - 1n,
  -(2n ** 63n),
  -(2n ** 63n) + 1n,
  0x7ffffffffffffc00n,
  0x7ffffffffffffdffn
];

const FLOAT32_BITS = [
  0x00000000, 0x80000000, 0x00000001, 0x807fffff, 0x3dcccccd, 0x7f7fffff, 0xff7fffff, 0x7f800000, 0xff800000,
  0x7fc00000, 0xffc00000, 0x7fc00001, 0x7f800001, 0x7fbfffff
];

const FLOAT64_BITS = [
  0x0000000000000000n,
  0x8000000000000000n,
  0x0000000000000001n,
  0x000fffffffffffffn,
  0x3fb999999999999an,
  0x7fefffffffffffffn,
  0x7ff0000000000000n,
  0xfff0000000000000n,
  0x7ff8000000000000n,
  0xfff8000000000000n,
  0x7ff8000000000001n,
  0x7ff0000000000001n,
  0x7fffffffffffffffn,
  0xffffffffffffffffn
];

const BAD_UTF8: [string, Uint8Array][] = [
  ["empty", new Uint8Array(0)],
  ["ascii", utf8("plain")],
  ["nul", hex("61 00 62")],
  ["lone-continuation", hex("80")],
  ["overlong-nul", hex("c0 80")],
  ["overlong-3", hex("e0 80 80")],
  ["surrogate", hex("ed a0 80")],
  ["surrogate-pair-cesu", hex("ed a0 bd ed b8 80")],
  ["above-max", hex("f4 90 80 80")],
  ["truncated-4", hex("f0 9f 98")],
  ["truncated-then-ascii", hex("e2 82 41")],
  ["ff", hex("ff")],
  ["fe-ff", hex("fe ff")],
  ["emoji", hex("f0 9f 98 80")],
  ["bom-only", hex("ef bb bf")],
  ["bom-prefix", hex("ef bb bf 61 62")],
  ["bom-twice", hex("ef bb bf ef bb bf 61")],
  ["bom-middle", hex("61 ef bb bf 62")],
  ["bom-truncated", hex("ef bb")],
  ["utf16le-bom", hex("ff fe 61 00")],
  ["mixed", concat(utf8("é"), hex("c3"), utf8("中"), hex("80 80"), utf8("z"))]
];

function encoderCases(corpus: Corpus) {
  const fill = (rng: Rng, entries: number, records: number, timestampUs: (i: number) => number) => {
    const encoder = new WPILOGEncoder("AdvantageScope");
    const used = STANDARD_TYPES.filter(([type]) => !["int", "int[]"].includes(type)).slice(0, entries);
    used.forEach(([type], i) => {
      encoder.add(
        WPILOGEncoderRecord.makeControlStart(0, {
          entry: i + 1,
          name: `/Encoder/${type.replace("[]", "Array")}`,
          type,
          metadata: i % 3 === 0 ? JSON.stringify({ source: "corpus", index: i }) : ""
        })
      );
    });
    for (let n = 0; n < records; n++) {
      used.forEach(([type, gen], i) => {
        const entry = i + 1;
        const ts = timestampUs(n);
        const r = rng;
        switch (type) {
          case "boolean":
            encoder.add(WPILOGEncoderRecord.makeBoolean(entry, ts, r.bool()));
            break;
          case "int64":
            encoder.add(WPILOGEncoderRecord.makeInteger(entry, ts, r.range(-1e6, 1e6)));
            break;
          case "float":
            encoder.add(WPILOGEncoderRecord.makeFloat(entry, ts, r.double()));
            break;
          case "double":
            encoder.add(WPILOGEncoderRecord.makeDouble(entry, ts, r.double()));
            break;
          case "string":
          case "json":
            encoder.add(WPILOGEncoderRecord.makeString(entry, ts, new TextDecoder().decode(gen(r))));
            break;
          case "boolean[]":
            encoder.add(
              WPILOGEncoderRecord.makeBooleanArray(
                entry,
                ts,
                Array.from({ length: r.below(5) }, () => r.bool())
              )
            );
            break;
          case "int64[]":
            encoder.add(
              WPILOGEncoderRecord.makeIntegerArray(
                entry,
                ts,
                Array.from({ length: r.below(5) }, () => r.range(-99, 99))
              )
            );
            break;
          case "float[]":
            encoder.add(
              WPILOGEncoderRecord.makeFloatArray(
                entry,
                ts,
                Array.from({ length: r.below(5) }, () => r.double())
              )
            );
            break;
          case "double[]":
            encoder.add(
              WPILOGEncoderRecord.makeDoubleArray(
                entry,
                ts,
                Array.from({ length: r.below(5) }, () => r.double())
              )
            );
            break;
          case "string[]":
            encoder.add(
              WPILOGEncoderRecord.makeStringArray(
                entry,
                ts,
                Array.from({ length: r.below(4) }, () => r.ascii(5))
              )
            );
            break;
          default:
            encoder.add(WPILOGEncoderRecord.makeRaw(entry, ts, gen(r)));
        }
      });
    }
    return encoder;
  };

  {
    const rng = corpus.rng("wpilog/encoder/all-types");
    let t = 0;
    const enc = fill(rng, 13, 25, () => (t += rng.range(1, 40000)));
    corpus.wpilog(
      "wpilog/encoder/all-types",
      "upstream WPILOGEncoder, every type it writes, ascending timestamps",
      enc.getEncoded()
    );
  }
  {
    const rng = corpus.rng("wpilog/encoder/sorted");
    const enc = fill(rng, 13, 20, () => rng.below(8) * 250000);
    corpus.wpilog(
      "wpilog/encoder/sorted",
      "WPILOGEncoder getEncoded(true): stable sort over shuffled records with many equal timestamps",
      enc.getEncoded(true)
    );
  }
  {
    const rng = corpus.rng("wpilog/encoder/unsorted");
    const enc = fill(rng, 13, 20, () => rng.range(0, 3 * SECOND));
    corpus.wpilog("wpilog/encoder/unsorted", "WPILOGEncoder without sorting: random record order", enc.getEncoded());
  }
  {
    const rng = corpus.rng("wpilog/encoder/fractional-timestamps");
    const enc = new WPILOGEncoder("AdvantageScope");
    enc.add(
      WPILOGEncoderRecord.makeControlStart(-1.5, { entry: 1, name: "/Fractional", type: "double", metadata: "" })
    );
    [0.5, 1.9999999, 2.0000001, 1e6 * 0.1, 1e6 * 1.1, -0.5, -1e6, 2 ** 53 + 1, 1e6 * 3.3].forEach((ts) =>
      enc.add(WPILOGEncoderRecord.makeDouble(1, ts, rng.double()))
    );
    corpus.wpilog(
      "wpilog/encoder/fractional-timestamps",
      "export-style fractional and negative microsecond timestamps, floored by encodeInteger",
      enc.getEncoded()
    );
  }
  {
    const enc = new WPILOGEncoder("AdvantageScope \u{1f916} \ud800 é");
    enc.add(
      WPILOGEncoderRecord.makeControlStart(0, {
        entry: 1,
        name: "/Lone\udc00Surrogate",
        type: "string",
        metadata: "\ud83d"
      })
    );
    enc.add(WPILOGEncoderRecord.makeString(1, 1000, "a\ud800b"));
    enc.add(WPILOGEncoderRecord.makeStringArray(1, 2000, ["\udfff", "ok"]));
    corpus.wpilog(
      "wpilog/encoder/lone-surrogates",
      "TextEncoder turns lone surrogates in the header, names, metadata and values into U+FFFD",
      enc.getEncoded()
    );
  }
  {
    const enc = new WPILOGEncoder("");
    enc.add(WPILOGEncoderRecord.makeControlStart(0, { entry: 1, name: "/Meta", type: "double", metadata: "first" }));
    enc.add(WPILOGEncoderRecord.makeDouble(1, 10, 1));
    enc.add(WPILOGEncoderRecord.makeControlSetMetadata(20, { entry: 1, metadata: "second" }));
    enc.add(WPILOGEncoderRecord.makeControlSetMetadata(30, { entry: 1, metadata: "a much longer metadata string" }));
    enc.add(WPILOGEncoderRecord.makeControlFinish(40, 1));
    corpus.wpilog(
      "wpilog/encoder/set-metadata-bug",
      "upstream makeControlSetMetadata omits the length prefix; then a finish 40 us after start",
      enc.getEncoded()
    );
  }
  {
    const rng = corpus.rng("wpilog/encoder/large");
    let t = 0;
    const enc = fill(rng, 13, 600, () => (t += rng.range(0, 20000)));
    corpus.wpilog(
      "wpilog/encoder/large",
      "WPILOGEncoder, 13 entries x 600 records, includes zero time steps",
      enc.getEncoded()
    );
  }
}

function typeCases(corpus: Corpus) {
  {
    const rng = corpus.rng("wpilog/types/every-type");
    const b = new WpilogBuilder();
    const all: [string, Gen][] = [
      ...STANDARD_TYPES,
      ...ODD_TYPES.map((t): [string, Gen] => [t, (r: Rng) => r.bytes(r.below(10))])
    ];
    all.forEach(([type], i) => b.start(0, i + 1, `/Types/${i}/${type}`, type, i % 2 === 0 ? "" : `{"type":"${type}"}`));
    for (let n = 0; n < 30; n++) all.forEach(([, gen], i) => b.record(i + 1, 1000 + n * 20000 + i, gen(rng)));
    corpus.wpilog(
      "wpilog/types/every-type",
      "every standard type string plus odd and unknown ones, 30 seeded samples each",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    const wrong: [string, Uint8Array[]][] = [
      ["boolean", [hex(""), hex("01 01"), hex("02"), hex("ff")]],
      ["int64", [hex(""), hex("01 02 03 04 05 06 07"), hex("01 02 03 04 05 06 07 08 09")]],
      ["int", [hex("01 02 03 04")]],
      ["float", [hex(""), hex("00 00 80 3f 00"), hex("00 00 00 00 00 00 f0 3f")]],
      ["double", [hex(""), hex("00 00 80 3f"), hex("00 00 00 00 00 00 f0 3f 00")]],
      ["int64[]", [hex(""), hex("01 00 00 00 00 00 00 00 02 00 00 00")]],
      ["float[]", [hex(""), hex("00 00 80 3f 00 00")]],
      ["double[]", [hex(""), hex("00 00 00 00 00 00 f0 3f 00 00 00 00")]],
      ["boolean[]", [hex(""), hex("00 01 02 ff")]],
      [
        "string[]",
        [
          hex(""),
          hex("00 00 00"),
          hex("00 00 00 00"),
          hex("00 00 00 00 ff"),
          hex("01 00 00 00"),
          hex("02 00 00 00 01 00 00 00 61"),
          hex("01 00 00 00 05 00 00 00 61"),
          hex("01 00 00 00 ff ff ff ff 61"),
          hex("ff ff ff ff 00 00 00 00"),
          payload.stringArray(["a", "b"], 1)
        ]
      ],
      ["string", [hex("")]],
      ["json", [hex(""), utf8("{"), utf8("null"), utf8("5"), utf8('"s"')]]
    ];
    wrong.forEach(([type], i) => b.start(0, i + 1, `/Wrong/${type}`, type));
    wrong.forEach(([, values], i) => values.forEach((v, n) => b.record(i + 1, (n + 1) * 1000, v)));
    corpus.wpilog(
      "wpilog/types/wrong-size",
      "payloads whose length does not fit the declared type; decode errors drop the record",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Int64", "int64").start(0, 2, "/Int", "int").start(0, 3, "/Int64Array", "int64[]");
    INT64_EXTREMES.forEach((v, i) => {
      b.record(1, (i + 1) * 1000, payload.int64(v));
      b.record(2, (i + 1) * 1000, payload.int64(-v));
    });
    b.record(3, 1000, payload.int64Array(INT64_EXTREMES)).record(3, 2000, payload.int64Array([]));
    corpus.wpilog(
      "wpilog/types/int64-extremes",
      "int64 values around 2^53 and the int64 limits, Number(BigInt) rounding",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Float", "float").start(0, 2, "/FloatArray", "float[]");
    FLOAT32_BITS.forEach((bits, i) => b.record(1, (i + 1) * 1000, new Bytes().f32Bits(bits).finish()));
    const all = new Bytes();
    FLOAT32_BITS.forEach((bits) => all.f32Bits(bits));
    b.record(2, 1000, all.finish());
    corpus.wpilog(
      "wpilog/types/float32",
      "float32 bit patterns: signed zero, subnormals, extremes, NaN payloads",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Double", "double").start(0, 2, "/DoubleArray", "double[]");
    FLOAT64_BITS.forEach((bits, i) => b.record(1, (i + 1) * 1000, new Bytes().f64Bits(bits).finish()));
    FLOAT64_BITS.forEach((bits, i) => b.record(1, (i + 100) * 1000, new Bytes().f64Bits(bits).finish()));
    const all = new Bytes();
    FLOAT64_BITS.forEach((bits) => all.f64Bits(bits));
    b.record(2, 1000, all.finish()).record(2, 2000, all.finish());
    corpus.wpilog(
      "wpilog/types/float64",
      "float64 bit patterns including NaN payloads, repeated so NaN dedup is exercised",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Utf8/String", "string")
      .start(0, 2, "/Utf8/StringArray", "string[]")
      .start(0, 3, "/Utf8/Json", "json");
    BAD_UTF8.forEach(([, bytes], i) => {
      b.record(1, (i + 1) * 1000, bytes);
      b.record(3, (i + 1) * 1000, concat(bytes, "1"));
    });
    b.record(2, 1000, payload.stringArray(BAD_UTF8.map(([, bytes]) => bytes)));
    corpus.wpilog(
      "wpilog/utf8/values",
      "invalid UTF-8 and BOM variants in string, string[] and json values",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder({ extra: hex("ef bb bf 68 69 ff") });
    let id = 1;
    BAD_UTF8.forEach(([name, bytes]) => {
      b.start(0, id, concat("/Names/", bytes), "double", bytes);
      b.record(id, 1000, payload.double(id));
      id++;
      b.start(0, id, `/Types/${name}`, concat(bytes, "double"), "");
      b.record(id, 1000, payload.double(id));
      id++;
    });
    b.start(0, id, hex("ef bb bf 2f 42 6f 6d"), "double").record(id, 2000, payload.double(-1));
    b.start(0, id + 1, "/Bom", "double").record(id + 1, 2000, payload.double(-2));
    corpus.wpilog(
      "wpilog/utf8/names",
      "invalid UTF-8 and BOMs in names, type strings, metadata and the extra header; BOM-stripped names collide",
      b.bytes()
    );
  }
  {
    const rng = corpus.rng("wpilog/types/random");
    const b = new WpilogBuilder();
    STANDARD_TYPES.forEach(([type], i) => b.start(0, i + 1, `/Random/${type}`, type));
    for (let n = 0; n < 150; n++) {
      const i = rng.below(STANDARD_TYPES.length);
      b.record(i + 1, n * 5000, STANDARD_TYPES[i][1](rng));
    }
    corpus.wpilog("wpilog/types/random", "seeded random values across the standard types", b.bytes());
  }
}

function timestampCases(corpus: Corpus) {
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Dup/Same", "double").start(0, 2, "/Dup/Different", "double").start(0, 3, "/Dup/Array", "double[]");
    for (let i = 0; i < 4; i++) b.record(1, 5000, payload.double(1));
    [1, 2, 3, 2].forEach((v) => b.record(2, 5000, payload.double(v)));
    b.record(2, 6000, payload.double(2)).record(2, 6000, payload.double(9));
    [
      [1, 2],
      [1, 3],
      [1, 3, 4]
    ].forEach((v) => b.record(3, 7000, payload.doubleArray(v)));
    corpus.wpilog(
      "wpilog/timestamps/duplicates",
      "several records at the same timestamp: last write wins, array children follow",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Order/Values", "double").start(0, 2, "/Order/Strings", "string");
    const plan: [number, number][] = [
      [5000, 1],
      [3000, 2],
      [9000, 3],
      [1000, 4],
      [4000, 2],
      [6000, 3],
      [2000, 4],
      [8000, 3],
      [7000, 3],
      [0, 0]
    ];
    plan.forEach(([ts, v]) => {
      b.record(1, ts, payload.double(v));
      b.record(2, ts, payload.string(`v${v}`));
    });
    corpus.wpilog(
      "wpilog/timestamps/out-of-order",
      "out-of-order timestamps hitting the equal-previous drop and equal-next move rules of putData",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Runs/Number", "double").start(0, 2, "/Runs/Bool", "boolean").start(0, 3, "/Runs/Raw", "raw");
    b.start(0, 4, "/Runs/NumberArray", "double[]");
    [1, 1, 1, 2, 2, 1, 1, NaN, NaN, 0, -0, 0].forEach((v, i) => b.record(1, i * 1000, payload.double(v)));
    [true, true, false, false, true].forEach((v, i) => b.record(2, i * 1000, payload.boolean(v)));
    ["", "", "00", "00", "0001", ""].forEach((v, i) => b.record(3, i * 1000, hex(v)));
    [[1, 2], [1, 2], [1, 3], [], [], [NaN], [NaN]].forEach((v, i) => b.record(4, i * 1000, payload.doubleArray(v)));
    corpus.wpilog(
      "wpilog/timestamps/run-length",
      "repeated values deduplicated by putData, NaN and signed zero equality",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Wide/Timestamps", "double");
    const tsValues: bigint[] = [
      0n,
      1n,
      2n ** 32n,
      2n ** 40n,
      2n ** 48n,
      2n ** 53n + 1n,
      2n ** 56n,
      2n ** 63n - 1n,
      2n ** 63n,
      2n ** 64n - 1n,
      0x8000000000000001n
    ];
    tsValues.forEach((ts, i) => b.record(1, ts, payload.double(i), { tsLen: 8 }));
    b.record(1, 5n, payload.double(-1), { tsLen: 5 }).record(1, 6n, payload.double(-2), { tsLen: 7 });
    corpus.wpilog(
      "wpilog/timestamps/eight-byte",
      "8-byte timestamps: top bit set is negative in the TS decoder and wraps in the C indexer",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/OnlyStart", "double").start(5 * SECOND, 2, "/AlsoStart", "string");
    corpus.wpilog("wpilog/timestamps/control-only", "only start records: the indexer range stays 0 to 0", b.bytes());
  }
  {
    const b = new WpilogBuilder();
    b.start(2n ** 62n, 1, "/Range", "double");
    b.record(1, 3 * SECOND, payload.double(1)).record(1, 2 * SECOND, payload.double(2));
    b.finish(2n ** 63n - 1n, 7).setMetadata(1, 1, "late");
    corpus.wpilog(
      "wpilog/timestamps/control-extremes",
      "control records at extreme times do not move the timestamp range",
      b.bytes()
    );
  }
  {
    const rng = corpus.rng("wpilog/timestamps/division");
    const b = new WpilogBuilder();
    b.start(0, 1, "/Division", "double");
    const found: number[] = [];
    while (found.length < 40) {
      const ts = rng.u32() * 256 + rng.below(256);
      if (ts / 1000000.0 !== ts * 1.0e-6) found.push(ts);
    }
    found.sort((a, b) => a - b).forEach((ts, i) => b.record(1, ts, payload.double(i)));
    corpus.wpilog(
      "wpilog/timestamps/division",
      "timestamps where worker ts/1e6 and indexer ts*1e-6 differ in the last bit",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Widths", "double", "", { entryLen: 4, sizeLen: 4, tsLen: 8 });
    b.record(1, 1000, payload.double(1), { entryLen: 4 });
    b.record(1, 2000, payload.double(2), { sizeLen: 4 });
    b.record(1, 3000, payload.double(3), { tsLen: 8 });
    b.record(1, 4000, payload.double(4), { entryLen: 3, sizeLen: 3, tsLen: 6 });
    b.record(1, 5000, payload.double(5), { spareBit: true });
    b.record(1, 6000, payload.double(6), { entryLen: 2, sizeLen: 2, tsLen: 3, spareBit: true });
    corpus.wpilog(
      "wpilog/varint/non-minimal",
      "non-minimal entry, size and timestamp widths, and the unused bitfield bit",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Size", "double").record(1, 1000, payload.double(1));
    b.record(1, 2000, payload.double(2), { size: 0xffffffff, sizeLen: 4 });
    b.record(1, 3000, payload.double(3));
    corpus.wpilog(
      "wpilog/varint/size-overflow",
      "a declared size of 2^32-1 stops both readers; later records are unreachable",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 0xffffffff, "/MaxId", "double").record(0xffffffff, 1000, payload.double(1));
    b.start(0, 0x100, "/Id256", "double").record(0x100, 1000, payload.double(2));
    b.start(0, 0x1000000, "/Id2p24", "double").record(0x1000000, 1000, payload.double(3));
    b.record(0x100, 2000, payload.double(4), { entryLen: 4 });
    corpus.wpilog("wpilog/varint/entry-ids", "large entry ids up to 2^32-1", b.bytes());
  }
}

function entryCases(corpus: Corpus) {
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/A", "double").record(1, 1000, payload.double(1)).record(1, 2000, payload.double(2));
    b.start(3000, 2, "/A", "string").record(2, 4000, payload.string("s")).record(1, 4500, payload.double(3));
    b.start(5000, 3, "/A", "double").record(3, 6000, payload.double(4));
    b.start(0, 4, "/B", "double").record(4, 1000, payload.double(1));
    b.start(2000, 5, "/B", "double").record(5, 3000, payload.double(2)).record(4, 4000, payload.double(3));
    corpus.wpilog(
      "wpilog/entries/redeclare",
      "redeclaring a name with a new type drops earlier data; the same type keeps it and both ids feed the key",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/First", "double").record(1, 1000, payload.double(1));
    b.start(2000, 1, "/Second", "string").record(1, 3000, payload.string("x"));
    b.record(9, 3500, payload.double(9));
    b.record(2, 3600, payload.double(9)).start(4000, 2, "/Late", "double").record(2, 5000, payload.double(5));
    corpus.wpilog(
      "wpilog/entries/id-reuse",
      "an id restarted under a new name, data for unknown ids and data before its start",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(SECOND, 1, "/Short", "double")
      .record(1, SECOND + 10, payload.double(1))
      .finish(2 * SECOND - 1, 1);
    b.record(1, 2 * SECOND, payload.double(2));
    b.start(SECOND, 2, "/Exact", "double")
      .record(2, SECOND + 10, payload.double(1))
      .finish(2 * SECOND, 2);
    b.record(2, 3 * SECOND, payload.double(3));
    b.start(0, 3, "/Twice", "double").finish(10, 3).finish(20, 3);
    b.finish(30, 99);
    b.start(3 * SECOND, 4, "/Short", "double").record(4, 4 * SECOND, payload.double(4));
    b.start(5 * SECOND, 5, "/Restart", "string")
      .finish(5 * SECOND + 1, 5)
      .start(5 * SECOND + 2, 5, "/Restart", "string");
    b.record(5, 7 * SECOND, payload.string("after"));
    corpus.wpilog(
      "wpilog/entries/short-lived",
      "finish under 1 s deletes the entry, exactly 1 s keeps it; double finish, unknown id, restart after delete",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Meta", "double", "initial").record(1, 1000, payload.double(1));
    b.setMetadata(2000, 1, "replaced").setMetadata(3000, 1, "").setMetadata(4000, 77, "unknown entry");
    b.record(0, 5000, hex("02 01 00 00 00 00 00 00"));
    b.setMetadata(6000, 1, hex("ef bb bf 7b 7d"));
    corpus.wpilog(
      "wpilog/entries/set-metadata",
      "set-metadata records: replace, empty, unknown entry, too short, BOM",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Ok", "double").record(1, 1000, payload.double(1));
    b.record(0, 2000, hex("02 01 00 00 00 ff 00 00 00"));
    corpus.wpilog(
      "wpilog/entries/set-metadata-bad-length",
      "a set-metadata whose string length overruns the payload throws inside the index callback",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Ok", "double").record(1, 1000, payload.double(1));
    b.record(0, 2000, concat(hex("00 02 00 00 00 ff 00 00 00"), "/Bad", hex("00 00 00 00 00 00 00 00")));
    corpus.wpilog(
      "wpilog/entries/start-bad-length",
      "a start record whose name length overruns the payload throws inside the index callback",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/Ok", "double").record(1, 1000, payload.double(1));
    b.record(0, 1100, hex("00 02 00 00 00 00 00 00 00 00 00 00 00 00 00 00"));
    b.record(0, 1200, hex("03 01 00 00 00"));
    b.record(0, 1300, hex("01 01 00 00 00 00"));
    b.record(0, 1400, hex(""));
    b.record(0, 1500, hex("01 01 00 00"));
    b.record(1, 2000, payload.double(2));
    corpus.wpilog(
      "wpilog/entries/control-malformed",
      "control records that match no kind: start under 17 bytes, unknown type 3, finish not 5 bytes, empty",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    const keys = [
      "0",
      "10",
      "2",
      "/1",
      "",
      "/",
      "//a",
      "a/",
      "a//b",
      "NT:/x:y",
      "Phoenix6/Talon_1/Pos",
      "/Phoenix6/Pigeon_2/Yaw",
      "DS:/Dscomm/Console/ConsoleLine",
      "/Robot/Arm/.type",
      "/Log1/Merged",
      "Log2",
      "4294967294",
      "4294967295",
      "-1",
      "01",
      "/Real Outputs/With Spaces",
      "/中文/键",
      "/a.b/c"
    ];
    keys.forEach((key, i) => {
      const type = key.endsWith(".type") || key.includes("Console") ? "string" : "double";
      b.start(0, i + 1, key, type);
      b.record(i + 1, 1000 + i, type === "string" ? payload.string(`v${i}`) : payload.double(i));
    });
    corpus.wpilog(
      "wpilog/entries/key-shapes",
      "integer-like, empty, slashed, merge-prefixed, Phoenix, console and unicode keys",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/DriverStation/Enabled", "boolean");
    b.start(0, 2, "DS:enabled", "boolean");
    b.start(0, 3, "/Robot/ProgramStartTime", "int64");
    b.start(0, 4, "/Sensor", "double");
    for (let i = 0; i < 20; i++) {
      b.record(1, i * 250000, payload.boolean(i % 5 > 1));
      b.record(2, i * 250000 + 1, payload.boolean(i % 7 > 2));
      b.record(4, i * 250000 + 2, payload.double(i));
    }
    b.record(3, 100, payload.int64(1500000000n)).record(3, 200, payload.int64(2250000000n));
    corpus.wpilog(
      "wpilog/entries/program-start",
      "eager robot-state keys and /Robot/ProgramStartTime, which clears data before 2.25 s",
      b.bytes()
    );
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "NT:/FMSInfo/FMSControlData", "int64");
    b.start(0, 2, "NT:/Robot/ProgramStartTime", "double");
    [0, 1, 33, 35, -1, -3, 2 ** 31 + 1].forEach((w, i) => b.record(1, i * SECOND, payload.int64(BigInt(w))));
    b.record(2, 10, payload.double(NaN));
    corpus.wpilog(
      "wpilog/entries/control-word",
      "FMSControlData words including negative odd values and a NaN program start",
      b.bytes()
    );
  }
}

function headerCases(corpus: Corpus) {
  const body = (b: WpilogBuilder) =>
    b.start(0, 1, "/Value", "double").record(1, 1000, payload.double(1)).record(1, 2000, payload.double(2));
  const variants: [string, string, WpilogBuilder][] = [
    ["empty", "zero-byte file", new WpilogBuilder(null)],
    ["short", "11 bytes, one short of a header", new WpilogBuilder(null).raw(hex("57 50 49 4c 4f 47 00 01 00 00 00"))],
    ["header-only", "valid 12-byte header and nothing else", new WpilogBuilder()],
    ["magic", "WPILOX magic with valid records after it", body(new WpilogBuilder({ magic: "WPILOX" }))],
    ["magic-lowercase", "wpilog magic in lowercase", body(new WpilogBuilder({ magic: "wpilog" }))],
    ["version-0101", "version 1.1", body(new WpilogBuilder({ version: 0x0101 }))],
    ["version-0200", "version 2.0", body(new WpilogBuilder({ version: 0x0200 }))],
    ["version-0001", "byte-swapped version", body(new WpilogBuilder({ version: 0x0001 }))],
    ["extra-unicode", "UTF-8 extra header", body(new WpilogBuilder({ extra: "AdvantageKit \u{1f916} é" }))],
    [
      "extra-len-short",
      "extra header length 2 bytes shorter than written",
      body(new WpilogBuilder({ extra: "abcdef", extraLen: 4 }))
    ],
    [
      "extra-len-long",
      "extra header length 3 bytes longer than written",
      body(new WpilogBuilder({ extra: "abcdef", extraLen: 9 }))
    ],
    ["extra-len-past-end", "extra header length beyond the file", body(new WpilogBuilder({ extraLen: 1000 }))],
    [
      "extra-len-all-ones",
      "extra header length 2^32-1, an int32 of -1 in the indexer",
      body(new WpilogBuilder({ extraLen: 0xffffffff }))
    ],
    [
      "extra-len-negative",
      "extra header length 2^31, a negative int32 in the indexer",
      body(new WpilogBuilder({ extraLen: 0x80000000 }))
    ]
  ];
  variants.forEach(([name, about, b]) => corpus.wpilog(`wpilog/header/${name}`, about, b.bytes()));
}

function truncationCases(corpus: Corpus) {
  const bases: [string, string, WpilogBuilder][] = [];
  {
    const b = new WpilogBuilder({ extra: "trunc" });
    b.start(0, 1, "/T/Double", "double").start(0, 2, "/T/String", "string");
    b.record(1, 1000, payload.double(1))
      .record(2, 1500, payload.string("keep"))
      .record(1, 2000, payload.double(Math.PI));
    bases.push(["double", "last record is a minimal-width double", b]);
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/T/First", "double").record(1, 1000, payload.double(1));
    b.start(2000, 2, "/T/Second/Name", "struct:Pose2d", '{"source":"truncation"}');
    bases.push(["start", "last record is a start control record", b]);
  }
  {
    const b = new WpilogBuilder();
    b.start(0, 1, "/T/Strings", "string[]").record(1, 1000, payload.stringArray(["a", "bc"]));
    b.record(1, 2n ** 40n, payload.stringArray(["last", "ε"]), { entryLen: 2, sizeLen: 4, tsLen: 8 });
    bases.push(["wide", "last record is a string[] with 2-byte entry, 4-byte size and 8-byte timestamp", b]);
  }
  for (const [name, about, b] of bases) {
    const bytes = b.bytes();
    const last = b.recordStarts[b.recordStarts.length - 1];
    for (let cut = last; cut < bytes.length; cut++) {
      const at = String(cut).padStart(4, "0");
      corpus.wpilog(
        `wpilog/truncate/${name}/at-${at}`,
        `${about}, file cut to ${cut} of ${bytes.length} bytes`,
        bytes.slice(0, cut)
      );
    }
  }
  {
    const b = new WpilogBuilder({ extra: "xy" }).start(0, 1, "/H", "double").record(1, 1000, payload.double(1));
    const bytes = b.bytes();
    for (let cut = 1; cut < b.recordStarts[0]; cut++) {
      const at = String(cut).padStart(4, "0");
      corpus.wpilog(
        `wpilog/truncate/header/at-${at}`,
        `header with a 2-byte extra header cut to ${cut} bytes`,
        bytes.slice(0, cut)
      );
    }
  }
}

function randomCases(corpus: Corpus) {
  for (let n = 0; n < 4; n++) {
    const id = `wpilog/random/mixed-${n}`;
    const rng = corpus.rng(id);
    const b = new WpilogBuilder({ extra: rng.ascii(8) });
    const live: number[] = [];
    let nextId = 1;
    let t = 0;
    for (let i = 0; i < 400; i++) {
      t += rng.below(3) === 0 ? 0 : rng.range(-2000, 30000);
      const ts = Math.max(0, t);
      const widths =
        rng.below(4) === 0 ? { entryLen: rng.range(1, 4), sizeLen: rng.range(1, 4), tsLen: rng.range(4, 8) } : {};
      const roll = rng.below(100);
      if (roll < 6 || live.length === 0) {
        const [type] = rng.pick(STANDARD_TYPES);
        const id = rng.below(5) === 0 && live.length > 0 ? rng.pick(live) : nextId++;
        b.start(
          ts,
          id,
          `/Random/${rng.below(12)}/${type.replace("[]", "s")}`,
          type,
          rng.bool() ? "" : rng.ascii(6),
          widths
        );
        if (!live.includes(id)) live.push(id);
      } else if (roll < 8) {
        b.finish(ts, rng.pick(live), widths);
      } else if (roll < 10) {
        b.setMetadata(ts, rng.pick(live), rng.ascii(10), widths);
      } else {
        const [, gen] = rng.pick(STANDARD_TYPES);
        b.record(rng.pick(live), ts, rng.below(10) === 0 ? rng.bytes(rng.below(9)) : gen(rng), widths);
      }
    }
    corpus.wpilog(
      id,
      "seeded random mix of starts, finishes, metadata and data of mismatched types and widths",
      b.bytes()
    );
  }
}

export function wpilogMatrix(corpus: Corpus) {
  encoderCases(corpus);
  typeCases(corpus);
  timestampCases(corpus);
  entryCases(corpus);
  headerCases(corpus);
  truncationCases(corpus);
  randomCases(corpus);
}
