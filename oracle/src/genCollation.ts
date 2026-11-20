import "./shims";

// Emits the collation ground truth for asrs_js::collation: one {"k":key,"p":rank,"n":rank} line
// per key, where "p" is the key's rank under a.localeCompare(b) and "n" its rank under
// a.localeCompare(b, undefined, { numeric: true }); equal ranks mean the comparison returns 0.
// ICU's numeric comparison is not transitive (its identical-prefix skip can split a digit run),
// so ranks alone cannot hold every result: every ordered pair is compared, and the pairs whose
// result differs from what the ranks give follow as {"xp":[[i,j,result],...]} and
// {"xn":[...]} lines. Ranks plus exceptions encode the full pairwise matrix of both forms.

const KEY_COUNT = 5000;

const fail = (message: string): never => {
  process.stderr.write(`genCollation: ${message}\n`);
  process.exit(1);
};

const locale = new Intl.Collator().resolvedOptions().locale;
if (locale !== "en-US") fail(`default collation locale is ${locale}, expected en-US`);

let seed = 0x5eed_c011;
const rng = () => {
  seed = (seed + 0x6d2b79f5) >>> 0;
  let t = seed;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const below = (n: number) => Math.floor(rng() * n);
const pick = <T>(items: readonly T[]): T => items[below(items.length)];
const chars = (s: string) => Array.from(s);

const keys: string[] = [];
const seen = new Set<string>();
const add = (key: string) => {
  if (keys.length < KEY_COUNT && !seen.has(key)) {
    seen.add(key);
    keys.push(key);
  }
};

const range = (from: number, to: number) => {
  for (let cp = from; cp <= to; cp++) add(String.fromCodePoint(cp));
};

add("");
range(0x20, 0x7e);
range(0x00, 0x1f);
add("\x7f");
range(0xa0, 0xff);

const fixed = [
  "a", "A", "á", "Á", "à", "â", "ä", "ã", "å", "æ", "Æ", "ß", "ss", "SS", "Ss", "ẞ", "ﬁ", "fi", "FI", "ŉ", "ǆ", "Ǆ", "ǅ",
  "dž", "é", "é", "É", "É", "Å", "Å", "Å", "ö", "ö", "ø", "Ø", "œ", "Œ", "þ", "ð", "ł", "đ",
  "ı", "İ", "i̇", "I", "i", "ĳ", "ij", "ǈ", "ﬀ", "ﬃ", "ſ", "ʼn", "ǰ", "ȷ", "ꞵ", "ɑ", "ɐ",
  "ạ́", "ạ́", "ạ́", "ấ", "ậ", "́", "́a", "á", "à́", "á̀",
  "a­b", "ab", "a​b", "a‌b", "a‍b", "a﻿b", "a\u0000b", "a\u0001b", "a͏b", "a⁠b",
  "a b", "a\tb", "a\nb", "a\rb", "a b", "a b", "a　b", "a_b", "a-b", "a‐b", "a–b", "a—b",
  "a.b", "a/b", "a:b", "a,b", "a;b", "a'b", "a’b", "a\"b", "a“b", "a(b", "a[b", "a{b", "a@b", "a#b", "a$b",
  "a%b", "a&b", "a*b", "a+b", "a<b", "a=b", "a>b", "a^b", "a`b", "a|b", "a~b", "a\\b", "a!b", "a?b", "a¡b", "a¿b",
  "1", "01", "001", "0", "00", "000", "10", "2", "02", "1.5", "1.05", "1.50", "-1", "+1", "1e5", "9", "99", "100", "1000",
  "1,000", "1 000", "1_000", "0x10", "１", "１０", "２", "٣", "۳", "३", "๓", "০", "੧", "𝟏", "𝟙", "①", "²", "Ⅻ", "ⅻ",
  "〇", "一", "二", "十", "٠", "٠١", "߁", "᠑", "꘡", "꧑", "１２3", "1２3", "12３",
  "1".repeat(300), "0".repeat(300) + "1", "9".repeat(254), "9".repeat(254) + "1", "9".repeat(255), "9".repeat(253) + "8",
  "1".repeat(254) + "2", "1".repeat(255), "1".repeat(254) + "0", "0".repeat(254), "0".repeat(255), "x" + "0".repeat(260) + "5",
  "123456789012345678901234567890", "123456789012345678901234567891", "12345678901234567890123456789",
  "Module1", "Module10", "Module2", "Module01", "module1", "Module 1", "Module1a", "Module1b", "Module1.5",
  "Module0", "Module00", "Module-1", "Module_1", "Module١", "Module١٠", "Module１", "Module１0",
  "v1.2.10", "v1.10.2", "v1.2.9", "v1.2.09", "v01.2.9", "2023 Robot", "2024 Robot", "2025 KitBot", "KitBot 2025",
  "2025 KitBot v2", "2025 Kitbot", "2026 KitBot", "Robot_2025", "Robot2025", "Robot 2025", "Duck Bot", "Crab Bot",
  "Log_23-04-05_12-00-00", "akit_23-04-05_12-00-00", "Log_24-01-01_00-00-00", "FRC_20240101_000000.wpilog",
  "😀", "😀😀", "😁", "👍", "👍🏽", "👍🏿", "👨‍👩‍👧", "👨", "🇺🇸", "🇬🇧", "🏳️‍🌈", "🏳", "❤️", "❤", "️", "©", "®", "™",
  "€", "$", "£", "¥", "₹", "₿", "°", "µ", "μ", "Ω", "\u2126", "K", "\u212a",
  "中", "中文", "日本", "漢字", "汉字", "一二三", "百", "千", "万", "あ", "ア", "ｱ", "か", "が", "カ", "ガ", "ㇰ", "ゝ", "ー",
  "한", "한국어", "ᄒ", "ㅎ", "가", "각", "가", "각", "ㄱ", "ᆨ", "ㄱㅏ", "𠀀", "𪜀", "々",
  "\u{2ebf0}", "\u{31350}", "⿼", "㇯", "\u{30000}", "丽", "\u{2f800}", "豈", "﨎",
  "α", "Α", "ά", "ὰ", "ω", "Ω", "ς", "σ", "Σ", "а", "А", "ё", "е", "Ё", "ѐ", "і", "ї", "ъ", "ь",
  "ع", "عربي", "ﻋ", "ـ", "aـb", "א", "שלום", "ש", "שׁ", "ก", "เก", "กา", "เ", "ไก", "ກ", "ເກ", "क", "क्ष", "क़",
  "क़", "অ", "க", "ა", "Ա", "ա", "አ", "Ꭰ", "ꭰ", "𐐀", "𐐨", "ᚠ", "ᚁ", "ᠠ", "ཀ", "ཱི", "ཱི", "ཱྀ",
  "̈́", "̈́", "ཷ", "ჲ", "Ჲ", "ɡ", "ǥ", "ꝺ", "ꟑ", "Ꟁ", "Ɤ", "Ꟍ", "Ᲊ", "ᲊ",
  "�", "￾", "￿", "\ud800", "\udbff", "\udc00", "\udfff", "\ud800\ud800", "\udc00\ud800", "a\ud800",
  "a\udc00b", "\ud83d", "\ude00", "😀", "\ud83d!", "", "", "\u{f0000}", "\u{10ffff}", "͸",
  "\u{2ffff}", "\u{e0001}", "\u{e0100}", "﷐", "\u{1fffe}", "\u{10d40}", "\u{10d50}", "\u{1e5d0}", "\u{16d40}",
  "\u{11bc0}", "\u{1fa89}", "\u{1fadc}", "\u{1cc00}", "\u{1f8b0}", "\u{11f00}", "\u{1e4d0}", "\u{11b00}", "\u{10ec2}",
  "ࢗ", "\u{105c0}", "\u{1df25}", "\u{1e030}", "\u{1f774}", "\u{1fbcb}", "\u{1d2c0}",
  "\u0000", "\u0000\u0000", "\u0001", "­", "​", "﻿", "\u0000a", "a\u0000",
];
fixed.forEach(add);

const tokens = [
  "RealOutputs", "ReplayOutputs", "Drive", "Module0", "Module1", "Module2", "Module3", "Module10", "Module11", "SwerveStates",
  "Measured", "Setpoints", "SetpointsOptimized", "Odometry", "Robot", "Pose", "Pose2d", "Translation", "Rotation", "x", "y",
  "value", "NT:", "SmartDashboard", "Shuffleboard", "AdvantageKit", "DriverStation", "Joystick0", "Joystick1", "ButtonCount",
  "ButtonValues", "POVs", "AxisValues", "Timestamp", "LoggedRobot", "FullCycleMS", "UserCodeMS", "PowerDistribution",
  "ChannelCurrent", "Current", "Vision", "Camera0", "Camera1", "Photon", "photonvision", "tx", "ty", "ta", "Elevator", "Arm",
  "Intake", "Shooter", "RPM", "Volts", "Amps", "TempCelsius", "DS:enabled", "DS:autonomous", "systemTime", "Gyro", "YawPosition",
  "YawVelocityRadPerSec", "DrivePositionRad", "DriveVelocityRadPerSec", "TurnAbsolutePosition", "TurnPosition", "AutoSelector",
  "LoggedDashboardChooser", "Alerts", "errors", "warnings", "infos", ".type", ".schema", "struct:Pose2d", "proto:Pose3d",
  "SystemStats", "BatteryVoltage", "BrownedOut", "CANBus", "Utilization", "PhoenixDiagnostics", "Spark", "TalonFX-12",
  "URCL", "Raw", "Persistent", "Faults", "StickyFaults", "AppliedOutput", "Velocity", "Position", "kP", "kI", "kD", "kFF",
];
// ICU skips the identical prefix of two strings and, in numeric collation, can split a digit run
// there (see genCollationUnsafe.ts). These keys share digit prefixes and then continue with
// supplementary digits behind safe and unsafe lead surrogates, BMP digits, and characters on
// either side of ICU's unsafe-backward set.
const splitPrefixes = ["", "1", "12", "0", "00", "9", "x1", "Module1", "a", "𝟮", "1𝟮"];
const splitTails = [
  "𝟮", "𝟐", "🯲", "𐒢", "𑁨", "𞥒", "𖩢",
  "𞅂", "𝐀", "\ud835", "\ud83e", "2", "0", "٢", "२", "２", "x", "L", "l", "·", "L·", " ",
  "€", "字", "가", "é", "é", "́", "Ș", "Ș", "\u0000", "-", ".", " ", "",
];
for (const prefix of splitPrefixes) {
  for (const tail of splitTails) {
    add(prefix + tail);
    add(prefix + tail + pick(["5", "x", "𝟱", "", "05"]));
  }
}

const separators = ["/", "/", "/", "/", "_", "-", ".", " ", ":", ""];
const caseTweak = (s: string) => {
  switch (below(6)) {
    case 0:
      return s.toLowerCase();
    case 1:
      return s.toUpperCase();
    case 2:
      return s.charAt(0).toLowerCase() + s.slice(1);
    default:
      return s;
  }
};
for (let i = 0; i < 1500; i++) {
  let key = below(4) === 0 ? "" : "/";
  const depth = 1 + below(5);
  for (let d = 0; d < depth; d++) {
    if (d > 0) key += pick(separators);
    key += caseTweak(pick(tokens));
  }
  if (below(5) === 0) key += String(below(5) === 0 ? below(100000) : below(20));
  add(key);
}

const digitZeros = [0x30, 0xff10, 0x660, 0x6f0, 0x966, 0x9e6, 0xe50, 0x1d7ce, 0x1d7d8, 0x1040, 0xa8d0];
const digitRun = () => {
  const zero = below(5) === 0 ? pick(digitZeros) : 0x30;
  let s = "";
  const leading = below(4) === 0 ? below(4) : 0;
  for (let i = 0; i < leading; i++) s += String.fromCodePoint(zero);
  const len = below(8) === 0 ? 20 + below(300) : 1 + below(6);
  for (let i = 0; i < len; i++) s += String.fromCodePoint(zero + below(10));
  return s;
};
const words = ["Module", "module", "Robot", "KitBot", "v", "Log_", "Camera", "file", "item", "a", "A", "b", "", "", "x-", "Z"];
const glue = ["", "", " ", ".", "-", "_", "/", ",", ":"];
for (let i = 0; i < 1000; i++) {
  let key = pick(words);
  const parts = 1 + below(3);
  for (let p = 0; p < parts; p++) {
    key += digitRun();
    if (p + 1 < parts || below(2) === 0) key += pick(glue) + (below(2) === 0 ? pick(words) : "");
  }
  add(key);
}

for (let i = 0; i < 800; i++) {
  const len = below(7);
  let key = "";
  for (let j = 0; j < len; j++) key += String.fromCharCode(0x20 + below(95));
  add(key);
}

const pool = chars(
  "aAbBcCeEiIoOuUzZ01234567 ._-/:" +
    "áÁàÀâäÄåÅæçÇéÉèëñÑöÖøßþðłŁœ" +
    "̸̧̣̀́̂̃̈̊ͅ" +
    "­​‍﻿\u0000\u0007͏️" +
    "αΑωΩςσΣаАёЁжЖעבש" +
    "中文日本漢字一十あアｱかがカガ한국가각각" +
    "١٢३४１２๑" +
    "😀👍🏽🇺🇸❤©™€" +
    "�"
).concat(["\ud800", "\udc00", "\ud83d", "\ude00", "\u{20000}", "\u{1f600}", "\u{10ffff}", "\u{e0001}", "\u{10d40}", "\u{1e5d0}"]);
for (let i = 0; i < 1000; i++) {
  const len = 1 + below(6);
  let key = "";
  for (let j = 0; j < len; j++) key += pick(pool);
  add(key);
}

const mutations: ((units: string[]) => void)[] = [
  (u) => {
    const i = below(u.length);
    u[i] = u[i] === u[i].toLowerCase() ? u[i].toUpperCase() : u[i].toLowerCase();
  },
  (u) => u.splice(below(u.length + 1), 0, pick(["́", "̈", "̣", "̀"])),
  (u) => u.splice(below(u.length + 1), 0, pick(["­", "​", "\u0000", "﻿", "͏"])),
  (u) => u.splice(below(u.length + 1), 0, pick([" ", "-", "_", ".", "/"])),
  (u) => {
    const i = below(u.length);
    if (/[0-9]/.test(u[i])) u[i] = String(below(10));
    else u.splice(i, 0, "0");
  },
  (u) => u.splice(below(u.length), 1),
  (u) => {
    const i = below(u.length);
    u[i] = u[i].normalize(pick(["NFD", "NFC", "NFKD", "NFKC"]));
  },
];
while (keys.length < KEY_COUNT) {
  const base = chars(pick(keys));
  if (base.length === 0) continue;
  pick(mutations)(base);
  add(base.join(""));
}

const plain = (a: string, b: string) => a.localeCompare(b);
const numericCollator = new Intl.Collator(undefined, { numeric: true });
const numeric = (a: string, b: string) => numericCollator.compare(a, b);

for (let i = 0; i < 200000; i++) {
  const a = pick(keys);
  const b = pick(keys);
  const viaLocaleCompare = a.localeCompare(b, undefined, { numeric: true });
  if (viaLocaleCompare !== numeric(a, b)) {
    fail(`numeric localeCompare and Intl.Collator differ for ${JSON.stringify([a, b])}`);
  }
}

const MAX_EXCEPTIONS = 100000;

const ranks = (cmp: (a: string, b: string) => number) => {
  const order = keys.map((_, i) => i).sort((i, j) => cmp(keys[i], keys[j]));
  const rank = new Array<number>(keys.length);
  let current = 0;
  order.forEach((index, position) => {
    if (position > 0 && cmp(keys[order[position - 1]], keys[index]) !== 0) current = position;
    rank[index] = current;
  });
  const exceptions: [number, number, number][] = [];
  for (let i = 0; i < keys.length; i++) {
    for (let j = 0; j < keys.length; j++) {
      const got = cmp(keys[i], keys[j]);
      if (got !== Math.sign(rank[i] - rank[j])) exceptions.push([i, j, got]);
    }
  }
  if (exceptions.length > MAX_EXCEPTIONS) fail(`${exceptions.length} pairs disagree with the ranks`);
  return { rank, exceptions };
};

const plainRanks = ranks(plain);
const numericRanks = ranks(numeric);

keys.forEach((k, i) => {
  process.stdout.write(JSON.stringify({ k, p: plainRanks.rank[i], n: numericRanks.rank[i] }) + "\n");
});
process.stdout.write(JSON.stringify({ xp: plainRanks.exceptions }) + "\n");
process.stdout.write(JSON.stringify({ xn: numericRanks.exceptions }) + "\n");
