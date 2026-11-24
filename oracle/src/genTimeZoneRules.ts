import "./shims";
import names from "./timeZoneNames.json";

// Emits every UTC offset change V8's `Date` has from START to END in each of tzdata 2025b's zones:
// {"zone","changes":[[epoch ms, offset ms from it on]...]}, the first at START.
//
// V8 guesses the offset of an instant less than 19 days after one it has cached, as if an offset
// lasted at least that long, so offsets that did not (America/Boa_Vista in October 2000) come out
// differently by the queries before. Setting TZ resets the cache, so every query here is exact.

const START = Date.UTC(1830, 0, 1);
const END = Date.UTC(2100, 0, 1);
const DAY = 86_400_000;

function offset(zone: string, ms: number): number {
  process.env.TZ = zone;
  const local = new Date(ms);
  const utc = Date.UTC(2000, local.getMonth(), local.getDate(), local.getHours(), local.getMinutes(), local.getSeconds(), local.getMilliseconds());
  return new Date(utc).setUTCFullYear(local.getFullYear(), local.getMonth(), local.getDate()) - ms;
}

// The whole-second instants in (from, to] where the offset changes.
function changes(zone: string, from: number, before: number, to: number, after: number, out: [number, number][]) {
  if (before === after) return;
  if (to - from === 1000) {
    out.push([to, after]);
    return;
  }
  const middle = from + Math.floor((to - from) / 2000) * 1000;
  const between = offset(zone, middle);
  changes(zone, from, before, middle, between, out);
  changes(zone, middle, between, to, after, out);
}

for (const zone of names) {
  let before = offset(zone, START);
  const found: [number, number][] = [[START, before]];
  for (let to = START + DAY; to <= END; to += DAY) {
    const after = offset(zone, to);
    changes(zone, to - DAY, before, to, after, found);
    before = after;
  }
  process.stdout.write(JSON.stringify({ zone, changes: found }) + "\n");
}
