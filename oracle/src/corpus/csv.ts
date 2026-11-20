import Log from "@upstream/shared/log/Log";
import { bitsToF64, concat, hex, utf8 } from "./bytes";
import { Corpus } from "./corpus";
import { exportBytes } from "./exporter";
import { Rng } from "./rng";

/** A log touching every LoggableType, generated JSON children, and values that stress the CSV text form. */
export function csvSourceLog(rng: Rng, hostile: boolean): Log {
  const log = new Log();
  const strings = hostile
    ? ["plain", "comma, inside", 'quote " inside', "semicolon; space", "line\nbreak", "tab\tand\r", "\u00e9\u4e2d\ud83d\ude00", "\ud800", "", "(empty)", "ab-cd", "0a", "[1; 2]", "null"]
    : ["plain", 'quote " inside', "line\nbreak", "tab\tand\r", "\u00e9\u4e2d\ud83d\ude00", "\ud800", "", "ab-cd", "null"];
  const numbers = [0, -0, 1, -1.5, 0.1, 1e21, 1e-7, 123456789012345680000, 2 ** 53 + 2, NaN, Infinity, -Infinity, Number.MIN_VALUE, bitsToF64(0x7ff8000000000001n)];
  for (let i = 0; i < 40; i++) {
    const t = i * 0.02 + (i % 7 === 0 ? 0.001 : 0);
    if (i % 2 === 0) log.putNumber("/Drive/LeftVelocity", t, numbers[i % numbers.length]);
    if (i % 3 === 0) log.putBoolean("/Robot/Enabled", t, i % 6 === 0);
    if (i % 4 === 1) log.putString("/Robot/Status", t, strings[(i >> 2) % strings.length]);
    if (i % 5 === 0) log.putNumberArray("/Vision/Distances", t, Array.from({ length: i % 4 }, () => rng.double()));
    if (i % 5 === 2) log.putBooleanArray("/Vision/Valid", t, [true, false, i % 2 === 0]);
    if (i % 5 === 3) log.putStringArray("/Vision/Names", t, ["a", hostile ? "b, c" : "b", strings[i % strings.length]]);
    if (i % 6 === 0) log.putRaw("/Raw/Short", t, rng.bytes(i % 5));
    if (i % 10 === 0) log.putRaw("/Raw/Long", t, rng.bytes(520));
    if (i % 8 === 0) log.putNumberArray("/Arrays/Long", t, Array.from({ length: 503 }, (_, k) => k / 3));
    if (i % 7 === 0) log.putJSON("/Json", t, JSON.stringify({ "10": i, "2": [i, "x"], name: "n" + i, nested: { ok: i % 2 === 0 } }));
    log.putNumber("/Timestamp", t, t * 1e6);
  }
  log.putNumber("/Sorting/item10", 0.5, 10);
  log.putNumber("/Sorting/item2", 0.5, 2);
  log.putNumber("/Sorting/Item1", 0.5, 1);
  log.putNumber("/Sorting/_under", 0.5, 0);
  log.putNumber("NT:/Prefixed/Value", 0.25, 5);
  log.putString("/Single", 0.3, "only one sample");
  return log;
}

const EXPORTS: [string, string, Parameters<typeof exportBytes>[1]][] = [
  ["table-changes", "csv-table, all changes", { format: "csv-table", samplingMode: "changes" }],
  ["table-fixed-20", "csv-table, fixed 20 ms sampling", { format: "csv-table", samplingMode: "fixed", samplingPeriod: 20 }],
  ["table-fixed-7", "csv-table, fixed 7 ms sampling (accumulated f64 timestamps)", { format: "csv-table", samplingMode: "fixed", samplingPeriod: 7 }],
  ["table-akit", "csv-table, AdvantageKit cycles from /Timestamp", { format: "csv-table", samplingMode: "akit" }],
  ["table-original", "csv-table, generated fields excluded", { format: "csv-table", samplingMode: "changes", includeGenerated: false }],
  ["table-prefixes", "csv-table, prefixes /Vision,/Robot", { format: "csv-table", samplingMode: "changes", prefixes: "/Vision,/Robot" }],
  ["list-changes", "csv-list, all changes", { format: "csv-list", samplingMode: "changes" }],
  ["list-fixed-20", "csv-list, fixed 20 ms sampling", { format: "csv-list", samplingMode: "fixed", samplingPeriod: 20 }],
  ["list-akit", "csv-list, AdvantageKit cycles", { format: "csv-list", samplingMode: "akit" }]
];

async function roundTrips(corpus: Corpus) {
  for (const hostile of [false, true]) {
    const log = csvSourceLog(corpus.rng(hostile ? "csv/export-hostile" : "csv/export"), hostile);
    const dir = hostile ? "csv/export-hostile" : "csv/export";
    const what = hostile
      ? "a log whose strings hold commas and '; ' (the reader cannot split these arrays back)"
      : "a log of every type whose text survives the round trip";
    for (const [name, about, options] of EXPORTS) {
      corpus.add(`${dir}/${name}`, `upstream's exporter on ${what}: ${about}`, [{ path: `${dir}/${name}.csv`, bytes: await exportBytes(log, options) }]);
    }
  }
  const rng = corpus.rng("csv/export-random");
  for (let i = 0; i < 3; i++) {
    const random = new Log();
    for (let k = 0; k < 200; k++) {
      const key = `/R${rng.below(6)}/${rng.pick(["n", "b", "s", "a"])}`;
      const t = rng.below(500) / 100;
      if (key.endsWith("n")) random.putNumber(key, t, rng.double());
      else if (key.endsWith("b")) random.putBoolean(key, t, rng.bool());
      else if (key.endsWith("s")) random.putString(key, t, rng.ascii(8));
      else random.putNumberArray(key, t, [rng.double(), rng.double()]);
    }
    const format = i === 1 ? "csv-list" : "csv-table";
    corpus.add(`csv/export/random-${i}`, `upstream's exporter (${format}) on a seeded random log`, [
      { path: `csv/export/random-${i}.csv`, bytes: await exportBytes(random, { format }) }
    ]);
  }
}

const HAND: [string, string, string | Uint8Array][] = [
  ["minimal-table", "header and one row", "Timestamp,/A\n0,1"],
  ["minimal-list", "list header and one row", "Timestamp,Key,Value\n0,/A,1"],
  ["header-only-table", "a table header with no rows", "Timestamp,/A,/B"],
  ["header-only-list", "a list header with no rows", "Timestamp,Key,Value\n"],
  ["timestamp-only", "a table whose only column is Timestamp", "Timestamp\n0\n1"],
  ["crlf", "CRLF line endings: the \\r stays on the last cell", "Timestamp,/A,/S\r\n0,1,\"x\"\r\n1,2,\"y\"\r\n"],
  ["crlf-list", "a CRLF list header no longer matches exactly, is read as a table, and the key column fails to parse", "Timestamp,Key,Value\r\n0,/A,1\r\n"],
  ["blank-lines", "empty rows are skipped", "Timestamp,/A\n\n0,1\n\n\n1,2\n"],
  ["timestamps", "parseFloat timestamps: exponent, Infinity, -0, hex prefix, trailing text, leading space, empty", "Timestamp,/A\n1e3,1\nInfinity,2\n-0,3\n0x10,4\n12abc,5\n 7,6\n,7\n-Infinity,8\n.5,9"],
  ["raw-values", "raw cells: (empty), byte lists, a truncated list with -..., single bytes, a number that looks like a byte", "Timestamp,/R\n0,(empty)\n1,00-ff-10\n2,01-02-...\n3,0a\n4,ff\n5,10\n7,00-01-02-03-04"],
  ["raw-uppercase", "an uppercase byte list is not raw and not JSON: the load fails", "Timestamp,/R\n0,00-01\n1,AB-CD"],
  ["raw-dangling", "a byte list with a trailing dash: the load fails", "Timestamp,/R\n0,0a-"],
  ["arrays", "array cells split on '; ': empty, single empty element, trailing \"...\", strings, nested", 'Timestamp,/A\n0,[]\n1,[""]\n2,[1; 2; 3]\n3,[1; 2; "..."]\n4,["a"; "b"]\n5,[true; false]\n6,[[1]; [2]]'],
  ["array-no-space", "an array split only on ';' without the space: the elements do not parse and the load fails", "Timestamp,/A\n0,[1;2]"],
  ["json-values", "JSON cells: numbers, booleans, strings, null, single-key objects, big integers, -0", 'Timestamp,/V\n0,1\n1,true\n2,"text"\n3,null\n4,{"k":1}\n5,9007199254740993\n6,-0\n7,1e400\n8,"\\ud800"\n9,{"10":{"2":3}}'],
  ["mixed-types", "one column whose values change type: later writes are type warnings", 'Timestamp,/M\n0,1\n1,"s"\n2,true\n3,[1]\n4,00-01'],
  ["null-cells", "null cells write nothing, including the first row", "Timestamp,/A,/B\n0,null,1\n1,2,null"],
  ["duplicate-columns", "two columns with the same key", "Timestamp,/A,/A\n0,1,2\n1,3,4"],
  ["unordered", "rows out of time order and repeated timestamps", "Timestamp,Key,Value\n2,/A,2\n1,/A,1\n1,/A,5\n0,/B,true\n2,/B,false"],
  ["keys", "keys with spaces, colons, integer-like segments and no leading slash", "Timestamp,Key,Value\n0,NT:/a b/1,1\n0,10,2\n0,/x//y,3\n0,,4"],
  ["column-mismatch", "a row with too few columns: the load fails", "Timestamp,/A,/B\n0,1,2\n1,3"],
  ["list-column-mismatch", "a list row with a value containing a comma: the load fails", "Timestamp,Key,Value\n0,/A,1,2"],
  ["bad-header", "a header not starting with Timestamp: the load fails", "time,/A\n0,1"],
  ["bad-json", "a cell that is neither raw, array nor JSON: the load fails", "Timestamp,/A\n0,abc"],
  ["bad-array", "an array cell with an invalid element falls back to JSON.parse of the whole cell and fails", "Timestamp,/A\n0,[1; x]"],
  ["empty", "zero bytes: the header is empty and the load fails", ""],
  ["lowercase-header", "lowercase timestamp header fails", "timestamp,/A\n0,1"]
];

export async function csvMatrix(corpus: Corpus): Promise<void> {
  await roundTrips(corpus);
  for (const [id, about, text] of HAND) {
    corpus.add(`csv/hand/${id}`, about, [{ path: `csv/hand/${id}.csv`, bytes: typeof text === "string" ? utf8(text) : text }]);
  }
  corpus.add("csv/hand/bom", "a UTF-8 BOM before the header is stripped by TextDecoder", [
    { path: "csv/hand/bom.csv", bytes: concat(hex("ef bb bf"), "Timestamp,/A\n0,1") }
  ]);
  corpus.add("csv/hand/double-bom", "two BOMs: only the first is stripped, so the header no longer matches", [
    { path: "csv/hand/double-bom.csv", bytes: concat(hex("ef bb bf ef bb bf"), "Timestamp,/A\n0,1") }
  ]);
  corpus.add("csv/hand/invalid-utf8", "invalid UTF-8 inside a string cell decodes to U+FFFD", [
    { path: "csv/hand/invalid-utf8.csv", bytes: concat("Timestamp,/S\n0,\"a", hex("c3 28"), "\"") }
  ]);
}
