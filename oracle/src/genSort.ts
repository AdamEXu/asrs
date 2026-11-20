import "./shims";
import { toTree, type Cve1 } from "./cve1";

// Emits Array.prototype.sort / toSorted ground truth for asrs_js::sort. One line per case:
//   {"m":"sort"|"toSorted","c":<comparator kind>,"s":<seed>,"a":[<CVE-1 elements>],
//    "o":[<final layout>],"nc":<comparator calls>,"t":<trace hash>}
// Elements are wrapped as {v, id} (id = index in "a") before sorting, so "o" lists the ids in
// their final order, with {"u":null} for undefined and {"hole":null} for holes. "t" is FNV-1a
// over the (id, id) pair of every comparator call in call order; for the default comparator
// the pairs come from the two toString calls each comparison makes. Bare JSON numbers in "a"
// are integers (CVE-1 decoders accept them).

const CASES = 10000;

let seed = 0x50e7_1234;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const below = (n: number) => Math.floor(rng() * n);
const pick = <T>(items: readonly T[]): T => items[below(items.length)];

// The comparator stream of the "rand" and "randf" kinds, ported in the Rust test.
const caseRng = (caseSeed: number) => {
  let state = caseSeed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};

type Elem = { v: any; id: number; toString(): string };
type Cmp = ((a: Elem, b: Elem) => number) | undefined;

let trace = 0;
let calls = 0;
const record = (a: Elem, b: Elem) => {
  calls++;
  for (const x of [a.id, b.id]) {
    for (let shift = 0; shift < 32; shift += 8) {
      trace ^= (x >>> shift) & 0xff;
      trace = Math.imul(trace, 0x01000193) >>> 0;
    }
  }
};

const robotName = /^\d/;
const randfValues = [NaN, -0, 0, Infinity, -Infinity, 1e-300, -1e-300, 0.5, -0.5, 2 ** 53, -(2 ** 31)];

const comparators: { [kind: string]: (caseSeed: number) => Cmp } = {
  num: () => (a, b) => a.v - b.v,
  numNaN: () => (a, b) => a.v - b.v,
  rev: () => (a, b) => (a.v < b.v ? 1 : a.v > b.v ? -1 : 0),
  mod: () => (a, b) => (a.v % 3) - (b.v % 3),
  zero: () => () => 0,
  neg: () => () => -1,
  pos: () => () => 1,
  rand: (s) => {
    const r = caseRng(s);
    return () => {
      const x = r();
      return x < 1 / 3 ? -1 : x < 2 / 3 ? 0 : 1;
    };
  },
  randf: (s) => {
    const r = caseRng(s);
    return () => {
      const x = r();
      return x < 0.5 ? randfValues[Math.floor(r() * randfValues.length)] : x - 0.75;
    };
  },
  pairhash: () => (a, b) => ((a.id * 7919 + b.id * 104729) % 5) - 2,
  evergreen: () => (a, b) => {
    if (a.v === "Evergreen") return 1;
    if (b.v === "Evergreen") return -1;
    return a.v > b.v ? -1 : b.v > a.v ? 1 : 0;
  },
  robots: () => (a, b) => {
    if (robotName.test(a.v) && robotName.test(b.v)) {
      return -a.v.localeCompare(b.v, undefined, { numeric: true });
    } else {
      return a.v.localeCompare(b.v, undefined, { numeric: true });
    }
  },
  locale: () => (a, b) => a.v.localeCompare(b.v),
  default: () => undefined,
};

const kinds: [string, number][] = [
  ["num", 1800],
  ["numNaN", 600],
  ["rev", 600],
  ["mod", 600],
  ["zero", 300],
  ["neg", 300],
  ["pos", 300],
  ["rand", 1500],
  ["randf", 800],
  ["pairhash", 800],
  ["evergreen", 400],
  ["robots", 600],
  ["locale", 500],
  ["default", 900],
];
if (kinds.reduce((n, [, c]) => n + c, 0) !== CASES) throw new Error("case counts do not add up");

const length = () => {
  const r = rng();
  if (r < 0.04) return below(3);
  if (r < 0.2) return 2 + below(7);
  if (r < 0.7) return below(65);
  if (r < 0.95) return 64 + below(300);
  if (r < 0.995) return 300 + below(1700);
  return 2000 + below(8001);
};

const numbers = (n: number, withNaN: boolean): number[] => {
  const span = pick([1, 2, 3, 10, 100, 1000, 1e6]);
  const raw = Array.from({ length: n }, () => below(span));
  let out: number[];
  switch (below(8)) {
    case 0:
      out = raw.sort((a, b) => a - b);
      break;
    case 1:
      out = raw.sort((a, b) => b - a);
      break;
    case 2:
      out = raw.sort((a, b) => a - b).map((x) => (below(20) === 0 ? below(span) : x));
      break;
    case 3: {
      const run = 1 + below(40);
      out = Array.from({ length: n }, (_, i) => (i % run) + (below(4) === 0 ? below(3) : 0));
      break;
    }
    case 4: {
      out = Array.from({ length: n }, (_, i) => Math.min(i, n - i));
      break;
    }
    case 5: {
      const runs: number[] = [];
      while (runs.length < n) {
        const len = 1 + below(200);
        const up = below(2) === 0;
        const start = below(1000);
        for (let i = 0; i < len && runs.length < n; i++) runs.push(up ? start + i : start - i);
      }
      out = runs;
      break;
    }
    default:
      out = raw;
  }
  if (withNaN) out = out.map((x) => (below(6) === 0 ? NaN : x));
  if (below(10) === 0) out = out.map((x) => (below(4) === 0 ? x + 0.5 : x));
  return out;
};

const robotNames = [
  "2025 KitBot", "2024 KitBot", "2023 KitBot", "2026 KitBot", "KitBot 2025", "Crab Bot", "Duck Bot", "Evergreen", "Robot 10",
  "Robot 9", "Robot 1", "Robot 01", "10 Robot", "9 Robot", "1 Robot", "R2D2", "Mecanum", "mecanum", "Swerve", "Tank",
  "2025 kitbot", "2025 KitBot v2", "2025 KitBot v10", "Ｒobot", "Évolution", "evolution", "Zed", "zed", "_alpha", "-beta",
];
const fieldNames = [
  "Evergreen", "2025 Field", "2024 Field", "2023 Field", "2022 Field", "Reefscape", "Crescendo", "Charged Up", "FTC 2025",
  "evergreen", "FTC Into The Deep", "Rapid React", "Infinite Recharge", "2020 Field", "2019 Field", "Destination: Deep Space",
];
const anyNames = robotNames.concat(fieldNames, ["/Drive/Module0", "/Drive/Module10", "/Drive/Module2", "NT:/a", "a", "A", "b"]);
const strings = (n: number, kind: string): string[] => {
  const source = kind === "robots" ? robotNames : kind === "evergreen" ? fieldNames : anyNames;
  return Array.from({ length: n }, () => {
    const base = pick(source);
    return below(3) === 0 ? base + " " + below(30) : base;
  });
};

const values = (kind: string, n: number): unknown[] => {
  switch (kind) {
    case "numNaN":
      return numbers(n, true);
    case "evergreen":
    case "robots":
    case "locale":
      return strings(n, kind);
    case "default":
      return below(2) === 0 ? strings(n, kind) : numbers(n, false).map((x) => (below(8) === 0 ? x - 500 : x));
    default:
      return numbers(n, false);
  }
};

const compactTree = (tree: Cve1): Cve1 => {
  if (tree !== null && typeof tree === "object" && !Array.isArray(tree) && "n" in tree) {
    const bits = tree.n as string;
    const x = new Float64Array(new BigUint64Array([BigInt("0x" + bits)]).buffer)[0];
    if (Number.isSafeInteger(x) && !Object.is(x, -0)) return x as unknown as Cve1;
  }
  return Array.isArray(tree) ? tree.map(compactTree) : tree;
};

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

const plan: string[] = [];
for (const [kind, count] of kinds) for (let i = 0; i < count; i++) plan.push(kind);

for (let c = 0; c < plan.length; c++) {
  const kind = plan[c];
  const method = below(4) === 0 ? "toSorted" : "sort";
  const caseSeed = below(2 ** 32);
  const n = length();
  const vals = values(kind, n);

  const array: unknown[] = new Array(0);
  const markers: unknown[] = [];
  const special = below(7) === 0;
  for (let i = 0; i < vals.length; i++) {
    if (special && below(5) === 0) {
      const hole = below(2) === 0;
      if (hole) {
        array.length = i + 1;
        markers.push("hole");
      } else {
        array.push(undefined);
        markers.push(undefined);
      }
      continue;
    }
    const id = i;
    const elem: Elem = {
      v: vals[i],
      id,
      toString() {
        return String(this.v);
      },
    };
    array.push(elem);
    markers.push(vals[i]);
  }

  const encodedInput: Cve1[] = markers.map((m) => (m === "hole" ? { hole: null } : compactTree(toTree(m))));

  let cmp = comparators[kind](caseSeed);
  trace = 0x811c9dc5;
  calls = 0;
  let pending: Elem | undefined;
  if (cmp === undefined) {
    for (const e of array) {
      if (e !== undefined && e !== null) {
        const elem = e as Elem;
        elem.toString = function (this: Elem) {
          if (pending === undefined) {
            pending = this;
          } else {
            record(pending, this);
            pending = undefined;
          }
          return String(this.v);
        };
      }
    }
  } else {
    const inner = cmp;
    cmp = (a, b) => {
      record(a, b);
      return inner(a, b);
    };
  }

  const sorted = method === "sort" ? array.sort(cmp) : (array as any).toSorted(cmp);
  if (pending !== undefined) throw new Error("unpaired toString call");

  const layout: Cve1[] = [];
  for (let i = 0; i < sorted.length; i++) {
    if (!(i in sorted)) layout.push({ hole: null });
    else if (sorted[i] === undefined) layout.push({ u: null });
    else layout.push((sorted[i] as Elem).id);
  }
  out({ m: method, c: kind, s: caseSeed, a: encodedInput, o: layout, nc: calls, t: trace });
}
