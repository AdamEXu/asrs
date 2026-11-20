import { ByteSource, concat, hex } from "./bytes";
import { Corpus } from "./corpus";
import { Rng } from "./rng";
import { LogSpec } from "./wpilog";

class JsonCase {
  readonly spec = new LogSpec();
  private ids = new Map<string, number>();
  private t = 1000;

  put(key: string, text: ByteSource) {
    if (!this.ids.has(key)) this.ids.set(key, this.spec.entry(key, "json"));
    this.t += 20000;
    this.spec.put(this.ids.get(key)!, this.t, text);
    return this;
  }

  all(key: string, texts: ByteSource[]) {
    texts.forEach((t) => this.put(key, t));
    return this;
  }
}

function randomJson(r: Rng, depth: number): string {
  switch (r.below(depth > 2 ? 6 : 9)) {
    case 0:
      return "null";
    case 1:
      return r.bool() ? "true" : "false";
    case 2:
      return r.pick(["0", "-0", "1e400", "9007199254740993", "0.1", "-1.5e-7", "123e-2", String(r.range(-999, 999))]);
    case 3:
      return JSON.stringify(r.double());
    case 4:
      return JSON.stringify(r.ascii(6));
    case 5:
      return r.pick(['"\\ud800"', '"\\u00e9\\n"', '"a/b"', '""']);
    case 6: {
      const kind = r.below(3);
      const items = Array.from({ length: r.below(5) }, () =>
        kind === 0 ? (r.bool() ? "true" : "false") : kind === 1 ? String(r.range(0, 9)) : randomJson(r, depth + 1)
      );
      return `[${items.join(",")}]`;
    }
    default: {
      const keys = Array.from({ length: r.below(5) }, () =>
        r.pick(["a", "b", "0", "10", "2", "__proto__", "x/y", r.ascii(3)])
      );
      return `{${keys.map((key) => `${JSON.stringify(key)}:${randomJson(r, depth + 1)}`).join(",")}}`;
    }
  }
}

export function jsonMatrix(corpus: Corpus) {
  {
    const c = new JsonCase();
    c.put("/Json/IntegerKeys", '{"10":1,"2":2,"a":3,"0":4,"-1":5,"01":6,"4294967295":7,"4294967294":8,"1.5":9,"":10}');
    c.put("/Json/Duplicates", '{"a":1,"b":2,"a":3,"b":{"c":4}}');
    c.put("/Json/Proto", '{"__proto__":{"x":1},"y":2,"constructor":3,"toString":"s"}');
    c.put("/Json/Nested", '{"a":{"b":{"c":[1,{"d":true},null,[]]}},"e":{},"f":[{"g":1},{"g":"two"}]}');
    c.put("/Json/Slashes", '{"a/b":1,"c":{"/":2,"d//e":3},"":{"":4}}');
    c.all("/Json/Changing", [
      '{"v":1}',
      '{"v":"str"}',
      '{"v":{"w":1}}',
      '{"v":[true]}',
      '{"v":[]}',
      '{"v":null}',
      "{}"
    ]);
    corpus.spec(
      "payload/json/objects",
      "integer-like keys, duplicate keys, __proto__, nesting, slashes in keys and changing value types",
      c.spec
    );
  }
  {
    const c = new JsonCase();
    const numbers = [
      "9007199254740993",
      "-9007199254740993",
      "1e400",
      "-1e400",
      "-0",
      "0e0",
      "-0.0",
      "1E5",
      "1.5e-7",
      "123456789012345678901234567890",
      "0.1",
      "5e-324",
      "2e-324",
      "1.7976931348623157e308",
      "1.7976931348623159e308",
      "0.30000000000000004",
      "1e21",
      "1e-7",
      "100"
    ];
    c.put("/Json/Numbers", `{${numbers.map((n, i) => `"n${i}":${n}`).join(",")}}`);
    c.put("/Json/NumberArray", `[${numbers.join(",")}]`).put("/Json/NumberArray", `{"arr":[${numbers.join(",")}]}`);
    corpus.spec(
      "payload/json/numbers",
      "JSON.parse number rounding: values past 2^53, overflow to Infinity, underflow, -0, exponents",
      c.spec
    );
  }
  {
    const c = new JsonCase();
    const strings = [
      '"\\ud800"',
      '"\\udc00\\ud800"',
      '"\\ud83d\\ude00"',
      '"\\u0000"',
      '"\\/"',
      '"\\b\\f\\n\\r\\t"',
      '"\\u2028\\u2029"',
      '"é中🤖"',
      `"${"long".repeat(300)}"`,
      '"\\uFFFF"',
      '"\\uD800\\uDBFF"'
    ];
    c.put("/Json/Strings", `{${strings.map((s, i) => `"s${i}":${s}`).join(",")}}`);
    c.put("/Json/StringArray", `[${strings.join(",")}]`);
    c.put("/Json/Raw", concat('{"bad":"', hex("ff c3 28 ed a0 80"), '"}'));
    corpus.spec(
      "payload/json/strings",
      "escaped lone surrogates, control escapes, line separators, raw invalid UTF-8 inside a string",
      c.spec
    );
  }
  {
    const c = new JsonCase();
    c.all("/Json/RootArrays", [
      "[true,false]",
      "[1,2,3]",
      '["a","b"]',
      '[1,"a",true]',
      "[]",
      "[[1],[2,3]]",
      '[{"a":1},{"b":2}]',
      "[null]",
      "[null,1]"
    ]);
    c.put("/Json/RootArrays", "[]").put("/Json/RootArrays", "[true]");
    c.all("/Json/NestedArrays", [
      '{"a":[true,false]}',
      '{"a":[1,2]}',
      '{"a":["x"]}',
      '{"a":[1,"x"]}',
      '{"a":[]}',
      '{"a":[[true],[]]}'
    ]);
    corpus.spec(
      "payload/json/arrays",
      "root and nested arrays that are all-boolean, all-number, all-string, mixed, empty and nested",
      c.spec
    );
  }
  {
    const c = new JsonCase();
    c.all("/Json/Roots", [
      "true",
      "5",
      '"s"',
      "null",
      "",
      "{",
      "{}",
      "NaN",
      "Infinity",
      "[1,]",
      '{"a":1,}',
      "01",
      "{'a':1}"
    ]);
    c.all("/Json/Whitespace", [' \t\n\r{"a" : 1 }\n ', " {}", '{"a":1}garbage', '{"a":1} {"b":2}']);
    c.all("/Json/Bom", [
      concat(hex("ef bb bf"), '{"bom":1}'),
      concat(hex("ef bb bf ef bb bf"), '{"bom":2}'),
      concat('{"bom":', hex("ef bb bf"), "3}")
    ]);
    c.put("/Json/Deep", "[".repeat(200) + "1" + "]".repeat(200));
    c.put("/Json/DeepObject", '{"a":'.repeat(64) + "1" + "}".repeat(64));
    corpus.spec(
      "payload/json/roots",
      "primitive roots, invalid JSON, surrounding whitespace, BOMs and deep nesting",
      c.spec
    );
  }
  for (let n = 0; n < 2; n++) {
    const id = `payload/json/random-${n}`;
    const r = corpus.rng(id);
    const c = new JsonCase();
    for (let i = 0; i < 40; i++) {
      const keys = Array.from({ length: r.range(1, 5) }, () => r.pick(["a", "b", "0", "10", "c/d"]));
      c.put(
        `/Json/Random/${r.below(3)}`,
        `{${keys.map((key) => `${JSON.stringify(key)}:${randomJson(r, 0)}`).join(",")}}`
      );
    }
    corpus.spec(id, "seeded random JSON objects mixing every value kind, duplicate and integer-like keys", c.spec);
  }
}
