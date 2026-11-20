import "./shims";

// Emits the case-mapping ground truth for the jstring vectors:
//   {"cp":N,"l":"..","u":".."}  per-codepoint mappings (absent key = identity)
//   {"s":"..","l":"..","u":".."} multi-char context cases (final sigma, expansions)
//   {"cased":[[lo,hi],...]}, {"caseIgnorable":[[lo,hi],...]} property ranges
// BMP code points are emitted exhaustively; astral code points are emitted when
// either mapping differs from identity, plus a seeded sample of the rest.

const out = (value: object) => process.stdout.write(JSON.stringify(value) + "\n");

let seed = 0x9e3779b9;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};

const BMP_END = 0xffff;

for (let cp = 0; cp <= 0x10ffff; cp++) {
  const astral = cp > BMP_END;
  const s = String.fromCodePoint(cp);
  const l = s.toLowerCase();
  const u = s.toUpperCase();
  const rec: Record<string, unknown> = { cp };
  if (l !== s) rec.l = l;
  if (u !== s) rec.u = u;
  const interesting = rec.l !== undefined || rec.u !== undefined;
  if (!astral || interesting || (cp >= 0x10000 && rng() < 0.02)) {
    out(rec);
  }
}

const contexts = [
  "ΣΣ",
  "AΣ",
  "ΣA",
  "AΣA",
  "AΣ.",
  "A.Σ",
  "A Σ",
  "ΟΔΟΣ",
  "ΟΔΟΣ.",
  "Α1Σ",
  "1Σ",
  "Α'Σ",
  "Α’Σ",
  "ΑΣ",
  "A­Σ",
  "ÁΣ",
  "́Σ",
  "ΘΣΦΣ",
  "İ",
  "İ",
  "ß",
  "ẞ",
  "ﬀ ﬁ ﬂ ﬃ ﬄ ﬅ ﬆ ﬓ ﬔ ﬕ ﬖ ﬗ",
  "և",
  "ŉ",
  "ǰ",
  "ΐ ΰ",
  "ᾀ ᾈ ᾳ ᾼ ῃ ῌ ῳ ῼ ᾴ ᾷ",
  "Ǆ ǅ ǆ Ǉ ǈ ǉ Ǌ ǋ ǌ Ǳ ǲ ǳ",
  "ᏸᏀ ᏰᏨ",
  "ſıKÅ",
  "𝐀𝐚 𝔄𝔞 𝕬𝖆 𝓐𝓪 𝐀",
  "𐐀𐐨 𐲀𐳀 𖹠𖹿 𑢠𑣀 𞤀𞤢",
  "mixed Σurrogates: \ud83d\ude00 and lone \ud800\udfff Σ",
  "\ud800",
  "\udfff",
  "A\ud800",
  "\ud800Σ",
  "a̓Σ",
  "ὈΔΌΣ",
];
for (const s of contexts) {
  out({ s, l: s.toLowerCase(), u: s.toUpperCase() });
}

const ranges = (prop: string) => {
  const re = new RegExp(`\\p{${prop}}`, "u");
  const ranges: [number, number][] = [];
  let start = -1;
  for (let cp = 0; cp <= 0x10ffff; cp++) {
    const hit = re.test(String.fromCodePoint(cp));
    if (hit && start < 0) start = cp;
    if (!hit && start >= 0) {
      ranges.push([start, cp - 1]);
      start = -1;
    }
  }
  if (start >= 0) ranges.push([start, 0x10ffff]);
  return ranges;
};

out({ cased: ranges("Cased") });
out({ caseIgnorable: ranges("Case_Ignorable") });
