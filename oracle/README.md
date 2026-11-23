# Parity oracle

Runs upstream AdvantageScope TypeScript, unmodified, inside Electron 39.2.7's own runtime
(`ELECTRON_RUN_AS_NODE=1`), so V8 math and ICU match the shipped app. The Rust side is
compared against what this produces.

Nothing here touches `reference/`. Setup copies it to `target/oracle/upstream` and builds there.
Setup refuses to run unless `reference/AdvantageScope` is a clean checkout of the pinned commit
`8fc5b89`.

## Setup

```sh
cargo xtask oracle setup          # from a clean checkout; reruns skip unchanged steps
cargo xtask oracle setup --force  # redo every step
```

Needs `node`/`npm` (only used to run npm and build scripts), `git`, `bash` and network access.
Steps: sync upstream into `target/oracle/upstream`, `npm ci --ignore-scripts`, upstream's
`getLicenses.mjs` (the only postinstall step the bundles need), the Electron binary download,
upstream's `rolldown -c`, emsdk 4.0.12 plus upstream's `wasmCompile.mjs`, `npm ci` for this
package in `target/oracle/harness`, and the esbuild harness build into `target/oracle/dist`.
It finishes by running `selftest` and `smoke`.

Downloads go to `$ASRS_CACHE`: `npm/`, `electron/`, `emsdk/`, `public-logs/`. Without the
variable the cache is `<real target dir>/../cache` of the main checkout when its `target/` is a
symlink (on the dev machine `/Volumes/T9/asrs/cache`), otherwise its `target/cache`. A linked
worktree resolves the main checkout through its `.git` file, so worktrees share one cache
whatever their own `target/` points at. xtask passes the resolved directory to every oracle
process as `ASRS_CACHE`; Rust tests resolve it with `asrs_parity::cache`.

Setup ends by writing `target/oracle/install.json` (upstream commit, Electron version, whether
it was built or unpacked, and the host). `gen` and `corpus gen` refuse to run unless that
record matches the commit and Electron version xtask pins.

### Other hosts: packs

emsdk setup is scripted for macOS and Linux only, and the WASM indexer and JS bundles are
platform-neutral, so other hosts install from a pack built on the dev machine:

```sh
cargo xtask oracle pack                          # writes target/oracle/pack.tar.zst
cargo xtask oracle setup --from-pack <file>      # on the other host
```

The pack (about 36 MB) is `target/oracle/upstream` without `node_modules`: upstream sources,
the rolldown bundles, `wpilogIndexer.wasm`, `licenses.json` and the lockfile, plus
`asrs-oracle-pack.json` naming the commit and Electron version, which `--from-pack` checks
before unpacking. `--from-pack` then runs only `npm ci --ignore-scripts`, the Electron download
for that OS, the harness `npm ci` and esbuild build, `selftest` and `smoke`. It needs no
`reference/` checkout, no emsdk and never runs rolldown. The harness `dist` is not in the pack:
it embeds absolute paths, so each host builds it from its own checkout of `oracle/`. On
Windows the symlinks under upstream's `lite/` are skipped.

## Commands

| Command | What it does |
|---|---|
| `cargo xtask oracle fn [--zone <zone>]` | JSONL requests on stdin, CVE-1 responses on stdout (protocol below) |
| `cargo xtask oracle fn --list` | upstream modules, npm packages and helpers callable by `fn` |
| `cargo xtask oracle run [--zone <zone>] <entry> [args]` | run `target/oracle/dist/<entry>.mjs` under the oracle runtime |
| `cargo xtask oracle build` | rebuild the harness bundles only, after editing `oracle/src` |
| `cargo xtask oracle gen [<id>...] [--check \| --stale]` | regenerate committed vectors (below); `--check` fails if they are stale, `--stale` regenerates only stale ones |
| `cargo xtask oracle pack` | write the pack for `setup --from-pack` (above) |
| `cargo xtask oracle selftest` | CVE-1 encoder/decoder self test |
| `cargo xtask oracle smoke` | `smoke/fn-requests.jsonl` against `smoke/fn-expected.jsonl` |
| `cargo xtask oracle env` | electron binary, upstream, dist and cache paths and the environment policy as JSON |
| `cargo xtask oracle decode <file>` | canonical `Log` dump of one log file (below) |
| `cargo xtask oracle export <file> --options <json> [--out <file>]` | exact export bytes for one `ExportOptions` |
| `cargo xtask oracle historical [<corpus>...] [--check] [--jobs N]` | decode dumps and the export matrix over a synthetic corpus |
| `cargo xtask oracle dom [--backend jsdom\|blink] <spec or fixture>` | one DOM fixture's records (see "Blink backend") |
| `cargo xtask oracle dom-ab [<id prefix>...] [--jobs N] [--out <file>]` | every `dom/*` and `form/*` spec on jsdom and on Blink, differing records per family |

## Environment policy

xtask runs every oracle process with `TZ=<zone>`, `LANG=en_US.UTF-8`, `LC_ALL=en_US.UTF-8`
and `ASRS_ORACLE_ZONE=<zone>`, overwriting whatever the caller has set, and clears
`LANGUAGE`, `NODE_OPTIONS`, `NODE_ICU_DATA`, `ICU_DATA` and `ICU_TIMEZONE_FILES_DIR` (Electron's
ICU loads time zone rules from that directory, which would change offsets without changing the
zone name the startup check sees). The zone is `UTC` unless a spec or `--zone`
names one of `America/Los_Angeles`, `Asia/Kolkata` or `Pacific/Auckland`; no other zone is
accepted. `src/environment.ts`, imported first by `src/shims.ts`, checks at startup that the
variables are set that way, that the default `Intl` zone resolves to the expected one, and
that `Intl.DateTimeFormat`, `Intl.Collator` and `Intl.NumberFormat` resolve `en-US`; otherwise
it prints what differs and exits with status 78. Running a bundle by hand therefore needs those
variables too. The Electron app driver adds `--lang=en-US` (`appSwitches` in the policy).

The policy is written into `MANIFEST.json`, so changing it makes every vector stale.

## `fn` protocol

One request per line: `{"fn": "<name>", "args": [<CVE-1>, ...]}`. One response per request,
in order:

- `{"ok":<CVE-1>}`: the call returned; promises are awaited.
- `{"throw":<CVE-1>}`: the call threw that value.
- `{"error":"<message>"}`: the request was malformed or the name did not resolve.

Names:

| Form | Example | Call |
|---|---|---|
| global path | `Math.sin`, `JSON.stringify` | `this` is the parent object |
| prototype method | `Number.prototype.toFixed` | `args[0]` is the receiver |
| constructor | `new Date` | `new` with the args |
| upstream module | `shared/util:cleanFloat`, `shared/units:Units.convert`, `shared/log/Log:default` | module id is the path under `src/` without `.ts` |
| npm package | `npm:color-convert:hex.hsl` | resolved from upstream's `node_modules` |
| helper | `oracle:numberFormat` | see `src/helpers.ts` |
| lazy helper | `oracle:robotStateRun` | the `run` export of `src/lazyHelpers/robotStateRun.ts` |

New helpers that drive a whole upstream subsystem (the `*Run` modules behind an entry
generator) go in `src/lazyHelpers/<name>.ts`, not in `src/helpers.ts`. `build.mjs` bundles each
one separately to `dist/lazyHelpers/<name>.mjs`, and the registry imports it through a computed
URL the first time `oracle:<name>` is called, so it is in no other bundle. Adding or editing one
therefore leaves the generator hash of `fn`, and of every generator that imports
`src/helpers.ts`, unchanged; only its own generator and any `fn` spec whose requests name
`"oracle:<name>"` go stale (xtask adds those helpers' inputs to that spec's hash). A lazy
helper must have no load-time side effects and must not rely on module state shared with the
`fn` bundle: it gets its own copy of every module it imports, upstream ones included.
`geometryRun` and `logFieldRun` predate this and are still in `src/helpers.ts`.

Every `.ts` file under `src/shared` and `src/hub` is bundled except `hub/hub.ts` and worker
entry points; modules load lazily on first use, so one with load-time side effects does not
affect the others. `oracle:call` takes a function expression as a string and applies it to the
remaining args, for compositions that have no upstream function (it is how the smoke test
drives a real worker bundle).

## CVE-1 canonical encoding

Each value is one JSON text, written compactly with no whitespace. Canonical output is
byte-exact; the Rust implementation (`asrs-parity`) must produce the same bytes.

| JS value | Encoding |
|---|---|
| `null`, `true`, `false` | `null`, `true`, `false` |
| string | JSON string, escaped exactly like `JSON.stringify`: `\"`, `\\`, `\b \f \n \r \t`, other code units below U+0020 as `\u00xx`, lone surrogates as `\udxxx` (lowercase hex), everything else raw UTF-8 |
| number | `{"n":"<16 lowercase hex digits>"}`: the IEEE-754 bit pattern, so `-0`, infinities and NaN payloads are exact |
| `undefined` | `{"u":null}` |
| bigint | `{"bi":"<decimal>"}` |
| array | JSON array; a hole is `{"hole":null}`; non-index properties are dropped |
| plain object (prototype `Object.prototype` or `null`) | `{"o":[[key, value], ...]}` in `Object.keys` order (integer-like keys ascending, then insertion order) |
| other object | `{"c":["<constructor name>",[[key, value], ...]]}` |
| `Uint8Array` (and `Buffer`) | `{"b":"<lowercase hex>"}` |
| other typed array | `{"ta":["Float64Array",[<element>, ...]]}` |
| `ArrayBuffer` / `DataView` | `{"ab":"<hex>"}` / `{"dv":"<hex>"}` |
| `Date` | `{"d":"<16 hex digits of getTime()>"}` |
| `Map` / `Set` | `{"map":[[k, v], ...]}` / `{"set":[v, ...]}` in iteration order |
| `Error` | `{"err":["<name>","<message>"]}` |
| boxed primitive | `{"box":<primitive>}` |
| function / symbol | `{"fn":"<name>"}` / `{"sym":"<description>"}` or `{"sym":null}` |

Only own enumerable string keys are encoded; symbol keys are dropped. Cycles are an error.
Every JSON object in CVE-1 is a tag with exactly one key, so the encoding is unambiguous.
Decoders accept everything except `c` and `fn`; as a convenience they also accept a bare JSON
number as that number, which canonical output never contains.

Decoding normalizes the way JS does, and the Rust decoder (`asrs-parity`) mirrors each step
so both sides re-encode identically: `o` and `c` keys are deduplicated (first position, last
value) and put in property order; `map` and `set` drop SameValueZero duplicates and turn a
`-0` key into `0`; `d` applies TimeClip; `bi` drops leading zeros; `ta` elements are
converted to the element type; `box` of a bigint, symbol, null or object becomes what
`Object(x)` returns; `AggregateError` and `SuppressedError` lose their message. Where the two
deliberately differ: Rust also decodes `c` and `fn`, and rejects some malformed bodies the TS
decoder coerces (non-numeric `ta` elements, `bi` that is not `-?[0-9]+`, extra tuple items).
Signaling NaN bit patterns are outside the contract: V8 may quiet them.
`parity/vectors/cve1/torture` pins all of this, and `cargo xtask parity cve1` checks it live.

## Vectors

Specs live in `parity/specs`, vectors in `parity/vectors/<id>.jsonl.zst`, and
`parity/vectors/MANIFEST.json` records the upstream commit, Electron version and environment
policy, and for each vector its case count, content hash, generator hash, spec, zone and the
`<os>-<arch>` it was generated on, plus the DOM `backend` of each `dom` spec's vector. The header is pretty-printed and each vector is one
compact line, sorted by id, so branches that add or regenerate different vectors touch
different lines.

A generator hash covers only what that vector's generator uses: the harness files its entry
point bundles (esbuild's metafile, recorded by `build.mjs` in `target/oracle/dist/inputs.json`
as paths under `oracle/`; `gen` keeps a committed copy in `oracle/inputs.json`), the bundles
it starts at runtime (`runtimeLoads` in `build.mjs`: `protoRenderer` spawns the renderer check,
`canvasMain` loads the canvas page, `dom` evaluates the hub page bundle; a Blink `dom` vector
adds the `dom` and `domBlinkMain` entries' inputs), `build.mjs` and `package-lock.json`, then the spec files and the zone. `fn` specs use the `fn` entry.
Files are hashed with CRLF as LF. Upstream sources, upstream's bundles and workers, the
Electron version and the environment policy are not in the per-vector hash: the manifest
header pins them (upstream commit, Electron, policy) and a change there makes every vector
stale. So editing `src/genRegex.ts` makes only the `regex/*` vectors stale, while editing a
file every entry imports (such as `src/shims.ts`) or `build.mjs` makes all of them stale.

- `<id>.jsonl`: `fn` requests, run under UTC. Each vector line is the request with the
  oracle's `ok` or `throw` key appended. An `error` response fails generation.
- `<id>.entry.json`: `{"entry": "<name>", "args": [...], "zones": [...], "inputs": [...]}`. The
  entry's stdout lines are the vector, so an entry can build values JSON cannot express. The
  entry runs with `ASRS_ORACLE_SPEC` set to the spec's path, so it may read data from the spec
  itself (the `dom` entry does). `inputs` lists other files the entry reads, relative to the
  spec; they join the generator hash. An entry that reads a synthetic corpus names it in an
  arg (`target/parity/corpus/synthetic/<name>`) or an input
  (`../../corpus/synthetic/<name>.manifest.json`); see Corpus preconditions. A spec with
  `"entry": "dom"` also names its `"backend"`, `"blink"` (the default) or `"jsdom"`; `gen`
  runs the `domBlink` entry for Blink, and `gen --check` calls a vector stale when the backend
  recorded in the manifest is not its spec's, so a jsdom-generated vector cannot stand in a
  family flipped to Blink.
- `<id>.fn.json`: `{"requests": "<name>.requests.jsonl", "zones": [...]}`, `fn` requests from
  a sibling file (`*.requests.jsonl` files are never specs themselves).
- `<id>.session.json`: a live session (see Live sessions), run by the `liveSession` entry
  under UTC. The session file is the spec, so adding one needs no other edit.

`zones` is optional and lists policy zones. Without it a spec makes one UTC vector `<id>`;
with it, one oracle process per zone makes vector `<id>/<zone>`, for example
`oracle/zones/Asia/Kolkata`. Only local-time behaviour (`formatDate`, `new Date(y, m, d)`,
`toLocaleTimeString`) needs zones; everything else stays UTC. `parity/specs/oracle/` holds
the policy probes: `environment` (resolved zone and locales, `localeCompare`) and `zones` (the
local-time sites under all four zones).

`platforms` (in `.entry.json` or `.fn.json`, not combined with `zones`) marks a spec whose
results V8 itself computes differently per platform, such as `js/math-rem` (the NaN that `%`
returns comes from the C library's `fmod`). It lists `<os>-<arch>` tags and makes one vector
`<id>/<tag>` per platform, which only a host of that platform generates and checks for
content; elsewhere `gen` skips it and warns when it is stale, and `gen --check` checks only
its manifest entry. Generate one from the dev machine with
`cargo xtask host <name> gen <id>`, which syncs, runs `oracle gen` on the host and copies the
vector and its manifest entry back. Rust tests load the current platform's vector with
`asrs_parity::vectors::platform_vector("<id>")`.

`gen` with no ids regenerates every vector and drops manifest entries and files no spec
produces. `gen <id>...` takes spec or vector ids and refuses to run if the manifest's commit,
Electron version or policy differs from the current one, since that needs a full run.
`gen --stale [<id>...]` regenerates only the vectors (of those ids, or all) whose manifest
entry is stale, and with no ids also drops orphans; it is the usual command after editing a
generator or merging main. Stale platform vectors of other hosts are listed for
`cargo xtask host <name> gen <id>`.

`gen --check` works in two stages. It builds the harness (to learn each entry's inputs);
then, without running the oracle, it fails if the
manifest's upstream commit, Electron version or environment policy differs from the current
one, or if any vector's generator hash, spec, zone or content hash (against the committed
file) is stale, or a manifest entry has no spec. So an edit to a spec or to the oracle
sources without a regenerate fails even when it would not change the output. Only then does
it regenerate in memory and compare content with the differ. It also fails when
`oracle/inputs.json` differs from the graph the build just recorded.

`gen --check-hashes` is the first stage alone, computed from `oracle/inputs.json` and
repository files, so it needs no oracle install, upstream checkout or Electron. CI runs it, so
an edit to `build.mjs`, `package-lock.json`, a harness source or a spec cannot merge without
regenerating the vectors it touches.

The committed graph cannot drift silently through an import edit: adding or removing an import
changes a file already in the graph, which makes its vectors stale, and regenerating rewrites
`oracle/inputs.json`. It can drift only when an unchanged specifier starts resolving to a
different file under esbuild's extension order, such as a new `x.ts` beside an imported
`x.js` or `x.tsx` beside `x.ts`. Then `--check-hashes` keeps hashing the old file and misses
the new one and later edits to it until the next `gen`; only `gen --check`, which rebuilds the
graph, notices.

Rust tests read vectors without the oracle: `asrs_parity::fn_vectors!(test, "<id>", |case| ...)`
checks each recorded response, `asrs_parity::record_vectors!` hands over raw records, and a
mismatch panics with the differ's report. `$ASRS_PARITY_VECTORS` overrides the directory.

`cargo xtask parity <suite>` runs both sides live (`cve1`, `fn <spec>`, `diff <a> <b>`; see
`cargo xtask parity`). The Rust side of `fn` is `asrs-cli fn`, which speaks the same protocol;
ported functions are registered in `crates/asrs-cli/src/functions.rs` under their oracle name.

## Shims (`src/shims.ts`)

Imported first by every entry point: `window` and `self` alias `globalThis`;
`requestAnimationFrame` queues callbacks until `flushAnimationFrames(time)`; a global
`postMessage` records messages in `postedMessages`; `Worker` runs the real worker bundle in a
`worker_threads` thread with a web-worker style `self`, `onmessage` and `postMessage`. Relative
worker URLs resolve against `target/oracle/upstream/www`, so upstream's
`new Worker("../bundles/csvWorker.js")` loads the bundle rolldown built.

As in a browser, an exception or rejection in a worker's message handler becomes an `error` event
on the `Worker` and the worker keeps running. After each handled message (once an async handler
settles) the thread posts an internal done marker, which `OracleWorker` consumes; `busy` is true
while a posted message is still being handled. `setWorkerRouter` lets a harness take over message
delivery (`src/historical/scheduler.ts` does).

## Build constants

`build.mjs` applies the same `__distribution__`, `__version__` and `__copyright__` replacements
as upstream's `rolldown.config.mjs` (FRC6328 distribution), so `buildConstants.ts` matches the
shipped app. `__build_date__` is pinned to `1970-01-01T00:00:00.000Z` for reproducible output.

## Adding an entry point

Add `src/<name>.ts` (starting with `import "./shims"`) and run it with
`cargo xtask oracle run <name>`. Every top-level `src/*.ts` file is bundled as an entry point,
so `build.mjs` needs no edit (and other vectors' hashes do not change). If the entry spawns or
loads another bundle at runtime, add it to `runtimeLoads` in `build.mjs`. Harness code can import upstream sources as
`@upstream/<path under src>`.

## Synthetic corpus

`cargo xtask corpus gen [<name>...] [--check]` runs the generator named by
`parity/corpus/synthetic/<name>.spec.json` (`{"entry", "seed"}`) under the oracle runtime. Files land in
`target/parity/corpus/synthetic/<name>/`, and the committed `parity/corpus/synthetic/<name>.manifest.json`
lists every case with its description and each file's size and FNV-1a hash. `--check` regenerates into a
scratch directory and fails unless every file is byte-identical. `cargo xtask corpus list` prints the cases.

Corpus A (`src/corpusA.ts`, `src/corpus/`):

- `wpilog/encoder/*`: files written by upstream's own `WPILOGEncoder`.
- `wpilog/*` (everything else): files from the raw record builder in `src/corpus/wpilog.ts`, which controls
  every width, length and header field: every type string, int64 and float bit edge cases, invalid UTF-8 and
  BOMs, duplicate and out-of-order timestamps, redeclaration, short-lived entries, set-metadata records,
  8-byte varints, bad headers, and truncation at every byte of three last-record shapes and of the header.
- `payload/{struct,photon,proto,msgpack,json}/*`: a WPILOG file plus `<case>.ops.jsonl`, the `Log` calls
  wpilogWorker's `parseField` makes for its data records, in file order, one `{"fn":"putStruct","args":[...]}`
  per line with CVE-1 arguments and timestamps in seconds. Log-level tests replay these without a WPILOG reader.

Each case draws from its own random stream (the corpus seed mixed with the case id), so adding a case leaves
the others unchanged.

Corpus B (`src/corpusB.ts`):

- `rlog/*`: R1 and R2 files from `src/corpus/rlog.ts`: every R1 tag and R2 type string, value-length errors,
  key redeclaration, the timestamp corruption limits and resync scan, bad headers, truncation at every byte of
  the last cycle, seeded random logs.
- `dslog/*`: `.dslog` and `.dsevents` files (pairs share a base name): REV and CTRE power blocks, every status
  mask, battery spikes, 15000-cycle timestamp drift, LabVIEW time edges, tag stripping (including the
  `slice(-1)` quirk), truncation, and the header-only events file whose worker never responds.
- `roadrunner/*`: revisions 0 and 1, every schema kind, op mode timestamps, concatenated logs, error and
  truncation cases.
- `csv/export*/*`: logs exported by upstream's real `exportWorker` bundle (`src/corpus/exporter.ts`) in both
  layouts and every sampling mode, one log that survives the round trip and one that does not;
  `csv/hand/*`: hand-written and broken files.
- `urcl/*`: WPILOG files with URCL r1, r2 and r3 data; r3 STATUS frames are re-encoded with upstream's
  `buildCanFramePayload` for the 2025 and 2026 specs.
- `child/*`: parents with `log:<ext>` fields, chunked, nested, rejected and failing children.
- `merge/<case>/{target,source}.*`: log pairs for `getMergeOffset`, one per branch of its alignment logic.
- `fuzz/<family>/*`: six mutants per format from the fuzzer in `src/corpus/fuzz.ts`, so `--check` covers it.

`cargo xtask corpus fuzz [--seed <u32>] [--count <n>] [--check]` runs the same mutator (`src/corpusFuzz.ts`)
over the generated A and B corpora for `--deep` runs. Output goes to `target/parity/corpus/fuzz/<seed>/` with a
`list.jsonl` naming each mutant's source, donor and mutation steps; nothing is committed.

### Corpus preconditions

The generated tree under `target/` is not committed, so a fresh build dir has none. Every command
that reads it first checks each corpus it needs against the committed manifest (every listed file
present with its hash, nothing unlisted) and, if the tree is missing or differs, logs
`corpus <name> is not current (...); regenerating it` and rebuilds it as `corpus gen <name>` would,
without touching the committed manifest. If the generator no longer reproduces that manifest the
command fails and says to run `cargo xtask corpus gen <name>` and commit the result. The commands:

- `oracle gen`, `gen --stale` and `gen --check`, for the vectors they will run whose spec names a
  corpus (`log/merge-offset` and `urcl/urcl` read corpus B);
- `parity decode` with no file arguments (corpora A and B);
- `oracle historical` and `corpus fuzz` (the corpora they run over).

Generators that read the tree also check it themselves: `src/corpus/generated.ts` lists the
manifest's files for a generator and exits non-zero naming the first missing one, so
`oracle run genUrcl ...` on a partial tree fails instead of writing a vector with fewer cases.
New corpus-reading generators should use it.

## Public corpus

`parity/corpus/public.toml` lists real logs by pinned URL, sha256 and size. `cargo xtask corpus fetch` downloads
them into `$ASRS_CACHE/public-logs/`, rejects any file whose size or hash differs, and keeps verified files;
`--offline` only verifies. The logs are never committed.

Each log also records `license` and `redistributable`. Only data derived from a log with
`redistributable = true` (the MIT-licensed DSLOG-Reader files) may be committed. Specs whose vectors
come from the others (the RAR1741 logs, which state no license) carry `"local": true`: `oracle gen`
writes them to `$ASRS_CACHE/vectors/` with their own `MANIFEST.json`, never to `parity/vectors`, and
`--check` skips them. `cargo xtask corpus fetch` followed by `cargo xtask oracle gen --local`
regenerates them; without the logs a full `oracle gen` skips them with a warning. The oracle enforces
the split: `src/corpus/publicLogs.ts` refuses to read a non-redistributable log unless xtask set
`ASRS_ORACLE_LOCAL=1`, which it does only for local specs. Rust tests load local vectors with
`asrs_parity::vectors::check_local_records` and `check_local_fn_vectors`, which skip with a notice
when the vector has not been generated on this machine. The local specs are `wpilog/public`,
`log/search-public` and `log/merge-offset-public`; `log/search` and `log/merge-offset` keep their
synthetic cases committed.

## Historical decode and export

`decode`, `export` and `historicalRun` (`src/historical/`) open a log the way the hub does: a real
`HistoricalDataSource` from `src/hub/dataSources` on a fresh `Log`, the real worker bundles under the
`Worker` shim, and `src/historical/mainProcess.ts` standing in for main's `historical-start` and
`historical-start-raw` (a `.dslog` is read with its `.dsevents` sibling; a child log with a
`hoot`, `revlog` or `wpilogxz` extension gets a failed conversion, since the oracle has no
converter; such top-level files are refused).

Nothing depends on thread timing:

- Page timers are fake (`timers.ts`). Timeouts run only when the scheduler runs them, earliest due
  first, then in scheduling order; the 50 ms field-request intervals fire only on an explicit tick.
- `Scheduler` delivers worker messages one at a time. After microtasks it runs the next timeout if
  there is one; otherwise it takes the oldest worker (creation order) that has an undelivered
  message or is still busy, waits for that worker's next message and delivers it. It stops when no
  timeout is queued and no worker is busy.

Decode order:

1. `openFile(log, path, "")`, then settle: the `initial` phase.
2. Fire every source's interval once (no active tab or sidebar fields) and settle; repeat until a
   round creates no worker and posts no message.
3. `loadAllFields()` on the root and settle; repeat from step 2 until a whole pass does nothing.
   Child sources (embedded `log:` fields) go through the same steps. This is the `full` phase.

`exportRuns` (`src/exportRuns.ts`) opens logs the same way and runs the real export worker on each
once per `ExportOptions`, recording every value it posts through `WorkerManager`'s progress
callback and the bytes main would write.

The dump is CVE-1 JSONL, written for `initial` and then for `full`:

```text
{"phase", "status", "loadingFields", "fieldCount"}         status: HistoricalDataSourceStatus name
{"key", "type", "timestamps", "values", "structuredType", "wpilibType", "metadataString",
 "stripingReference", "typeWarning"}                         one line per field, getFieldKeys() order
{"generatedParents"}
{"timestampRange", "timestampZero"}                          raw state; null before any data
{"structSchemaStrings", "structSchemas"}
{"protoDescriptors"}
{"queuedStructs", "queuedStructArrays", "queuedProtos"}
```

`decode --initial` stops after the `initial` phase.

`decodeWorker <file>` (`src/decodeWorker.ts`) runs the worker bundle for the file's extension
alone, with the files main would send, and prints every message it posts, one request at a time
(the next request is posted once the worker has finished the previous one):

```text
{"request": "start"}                                         then the messages start posts
{"request": "parseField", "key"}                             then the messages parseField posts
{"type": "progress", "value"}
{"type": "initial", "isPartial"}                             then the dump lines above for message.log
{"type": "failed"}
{"type": "fields", "count"}                                  then one line per field response:
{"key", "generatedParent", "type", "timestamps", "values", "structuredType", "wpilibType",
 "metadataString", "stripingReference", "typeWarning"}
{"type": "threw"}                                            the handler threw (the worker's error event)
```

When the initial log is partial, the requests after `start` are every key of the initial log in
order, then every key `HistoricalDataSource` always requests (the robot-state key lists in
`updateFieldRequest`'s order), `"/"` plus the first key, `/asrs-oracle/missing`, and the first key
again. Lines are written synchronously, so a worker that never answers leaves the lines before it.

`export` decodes like `decode` (through `full`), then runs the real `exportWorker` bundle through
upstream's `WorkerManager` on `log.toSerialized()`, as `prepare-export` does, and writes what main's
`write-export` would: a string as UTF-8 (lone surrogates become U+FFFD), bytes as they are. Exit
status 3 means the worker rejected (the hub's "Failed to export data"); 4 means the run was
skipped. Two option sets are skipped because upstream would never finish:

- `fixed` sampling when `(range[1] - range[0]) / (samplingPeriod / 1000)` is not at most the
  sample limit (100000 by default), or when adding the step to either end of the range leaves it
  unchanged;
- `mcap` with `fixed` or `akit` sampling when this bound exceeds 1048576 (the writer's chunk size):
  2048, plus for every field whose type is Boolean, Number, String, BooleanArray or NumberArray,
  64 plus the key's UTF-8 length, plus for each sample timestamp with a value at or before it,
  31 plus the UTF-8 length of `JSON.stringify({ value })`. The sampled MCAP branch calls
  `addMessage` without awaiting it, so once a chunk fills, every later message re-finalizes an
  ever larger chunk.

The port's `asrs-cli decode <file>` must print the same dump and `asrs-cli export <file> --options
<json>` the same bytes, with the same skip rules, so `cargo xtask parity decode` and `export` can
compare the two with `cmp`.

`historicalSession <sessions.jsonl>` (`src/historicalSession.ts`) runs scripted hub sessions on
one `Log`: each `open` step is a new `HistoricalDataSource` as `startHistorical` makes it, and
main replies go to every source still in the hub's list. Timeouts and worker messages follow the
`Scheduler` rules above; intervals and animation frames run only from `tick` and `frame` steps.
The page `Date` starts at 1700000000000 and moves only with `frame`; each worker realm's `Date` is
pinned to 1000000000000 as in `decodeWorker`. A throw out of a page task prints
`{"event": "threw"}` and the session goes on, as a browser does. A worker that sends nothing for
`--stall-ms` (default 30000) while busy is killed and never heard from again, which is what a
worker that loops forever or dies looks like to the page. The trace, CVE-1 JSONL:

```text
{"session": name}
{"step", "op"}                                               before each step
{"event": "status", "source", "status"}                      plus "customError" on Error
{"event": "progress", "source", "value"}
{"event": "refresh", "source", "hasNewFields"}
{"event": "loaded", "source"}                                a loadAllFields() promise resolved
{"event": "main", "name", "path"}                            historical-start
{"event": "main", "name", "extension", "bytes"}              historical-start-raw
{"event": "worker", "worker", "script"}                      worker index in creation order
{"event": "post", "worker", "type": "start", "files"}        file lengths, null for a missing file
{"event": "post", "worker", "type": "parseField", "key"}
{"event": "terminate", "worker"}
{"event": "active", "fields"}                                after a pick step
{"event": "threw"}
dump lines as above                                          phase from the step, status is every
                                                             source's status joined by ","
{"end": name}
```

`asrs-cli historical-session` prints the same trace, and `cargo xtask parity historical` compares
the two over every decode-suite log (`cargo xtask parity historical --help` lists the scripts).

`cargo xtask oracle historical` runs every log file of a synthetic corpus (extensions with a
worker; a `.dsevents` only when it has no `.dslog` sibling) through `decode` and the export matrix
of `parity/corpus/historical.spec.json` (formats x sampling modes x generated on/off x prefix
sets), in parallel processes. Outputs land in `target/parity/oracle/<corpus>/<input>/` with an
`index.jsonl` of every output's hash; `parity/corpus/synthetic/<corpus>.historical.json` commits
one decode hash and one export-set hash per input. `--check` reruns into
`target/parity/oracle/<corpus>.check` and fails unless every output is byte-identical.

## Renderer check for protobuf int64

`protoRenderer` (vector `log/proto-renderer`) decodes corpus A's `payload/proto/*` files with the
real `bundles/wpilogWorker.js` twice: in a module web worker of a hidden Electron `BrowserWindow`
with the hub's default web preferences (no Node in the page or the worker), and under the oracle
shims. It fails unless both produce identical `initial` and `fields` messages, then records the
renderer's. The finding: protobufjs 8.3.0 bundles `long`, so the real renderer decodes 64-bit
fields as `Long` objects and `putUnknownStruct` expands them into `/low`, `/high` and `/unsigned`
child fields, exactly as the shim does.

## Canvas harness

`src/canvasMain.ts` runs as a real Electron app (not `ELECTRON_RUN_AS_NODE`) with a hidden
`BrowserWindow` loading `canvasPage.html`, whose script is `src/canvas/page.ts`. It is driven by
`cargo xtask canvas` (`cargo xtask oracle canvas` and `oracle canvas-lut` are aliases), which
starts Electron with `--force-color-profile=srgb --lang=en-US` and, for DOM scenes,
`--force-device-scale-factor=<1|2>`. These switches must be on the real command line: appended
from the ESM main they arrive too late and are ignored.

Modes (`electron canvasMain.mjs <mode> <out dir>`):

| Mode | Output |
|---|---|
| `canvas [<scene>...]` | `scenes/<scene>.json` and `ref/<scene>.{cpu,gpu}.png`: `getImageData` of the scene on a `willReadFrequently` (CPU) canvas, the reference, and on the default (GPU) canvas |
| `measure` | `measure.jsonl`: every `TextMetrics` field for each font, string, `textAlign` and `textBaseline` of the label corpus |
| `canvas-lut` | the calibration scenes `calib-0` to `calib-7` (grey text at each 3-bit luminance) on the CPU canvas |
| `dom <scale>` | `dom/<scene>@<scale>x.png` from `capturePage`, and `.json` with each row's box and per-character boxes; fails unless the colour patches come back exact |
| `list` | scene names |

PNGs are straight-alpha RGBA, written with filter 0 and zlib level 9 so equal pixels give equal
files. `info.json` records the Electron and Chrome versions, platform, architecture and GPU
feature status.

Scene JSON: `{name, group, gate, width, height, dpr, background, cmds}`. `gate` is `pixel`,
`text` or `calibration`; `background` is the opaque colour both sides are composited over before
comparing. `cmds` is a display list replayed call for call:

| Command | Meaning |
|---|---|
| `["=", prop, value]` | `ctx[prop] = value`; a value `{"linearGradient": [x0, y0, x1, y1], "stops": [[offset, colour], ...]}` or `{"radialGradient": [x0, y0, r0, x1, y1, r1], "stops": ...}` becomes a gradient |
| `[method, ...args]` | `ctx[method](...args)`; a string argument `"@id"` is the image source `id` |
| `["image", id, w, h, rgba]` | defines image source `id` from base64 straight RGBA |
| `["canvas", id, w, h, cmds]` | replays `cmds` on a new `w`x`h` canvas (same CPU or GPU mode) and keeps it as image source `id` |
| `["getImageData", sx, sy, sw, sh, "$reg"]` | stores the `ImageData` in register `$reg` |
| `["putImageData", "$reg" or {width, height, rgba}, dx, dy]` | puts a register or literal `ImageData` |

A port renderer for `cargo xtask canvas run` is called as
`<program> <args>... <scenes dir> <out dir> [<scene>...]` and writes `<out dir>/<scene>.png` in
the same straight-alpha form.

## Live sessions

`src/liveSession.ts` runs upstream's real live sources (`NT4Source` with `NT4_Client`,
`NT4Publisher`, `NT4Tuner`, `RLOGServerSource` with the main-process TCP code transcribed in
`src/live/rlogMain.ts`, `PhoenixDiagnosticsSource`, `FTCDashboardSource`) against a scripted
mock server, entirely in process on a fake clock, and prints a trace. Each session is one file,
`parity/specs/live/<protocol>/<name>.session.json`, which makes vector `live/<protocol>/<name>`.
`cargo xtask oracle run liveSession <file>` prints a trace without writing a vector.

Adding a session (the N units): write the file, run `cargo xtask oracle gen --stale`, commit
the session, its vector and the new `MANIFEST.json` line. No code or other spec changes: xtask
discovers session files, and the Rust test
`crates/asrs-parity/tests/live_sessions.rs` replays every one of them against the Rust mocks
(below). The manifest is the one shared file; a conflict there after merging main is resolved
by rerunning `oracle gen --stale`. A session that needs a mock or harness feature that does not
exist yet is a harness change, not a session.

### Session format

```json
{
  "protocol": "nt4",
  "seed": 6328,
  "epochMs": 1700000000000,
  "latencyMs": 2,
  "endMs": 3200,
  "client": { "source": "nt4", "mode": "default", "address": "127.0.0.1", "prefs": {}, "tabs": [], "sidebar": [] },
  "server": { "version": "4.1", "topics": [] },
  "steps": [
    { "t": 0, "client": ["connect"] },
    { "t": 500, "server": ["set", "/SmartDashboard/speed", 2.75] }
  ]
}
```

| Field | Meaning |
|---|---|
| `protocol` | `nt4`, `rlog`, `phoenix` or `ftc`: which mock serves the session |
| `seed` | at least 1 (V8 treats 0 as no seed); `Math.random` is V8's own generator under `--random-seed=<seed>` (`src/live/random.ts`; the port uses `asrs_js::time::V8MathRandom::from_seed`). It is installed before the source is built, so the source UUID, NT4 uids and the msgpack key-cache evictions all draw from it |
| `epochMs` | `Date.now()` at session time 0 (default 1700000000000) |
| `latencyMs` | one-way network delay, an integer of at least 1 (default 1) |
| `endMs` | the session stops after this time; a final `dump` is recorded then |
| `client.source` | `nt4` (needs `mode`: `default`, `akit`, `systemcore`, `ds`), `nt4-publisher` (`mode`: `sim` or `robot`), `nt4-client` (upstream's `NT4_Client` on its own; optional `ports` and `appName`), `rlog`, `phoenix`, `ftc` |
| `client.address` | what `connect` passes to the source (default `127.0.0.1`); the publisher derives its own |
| `client.prefs` | merged over upstream's `DEFAULT_PREFS` into `window.preferences`; `null` makes it `null` |
| `client.tabs`, `client.sidebar` | what `window.tabs.getActiveFields()` and `window.sidebar.getActiveFields()` return |
| `client.platform` | `window.platform` (default `darwin`) |
| `server` | the mock's configuration (per protocol, below) |
| `steps` | `{"t": <ms>, "client": [op, ...]}` or `{"t": <ms>, "server": [op, ...]}`, in non-decreasing time order |

Client ops, applied the way `hub.ts` does:

| Op | Effect |
|---|---|
| `["connect"]` | `startLive`: stops the previous source and publisher, builds the source and calls `connect(address, status, output)`; the output callback sets `window.log`. For `nt4-publisher` it builds a new `NT4Publisher` |
| `["stop"]` | `stop()` on the source and the publisher |
| `["prefs", {...} \| null]` | merges into `window.preferences` (from the defaults when it is `null`), or sets it to `null` |
| `["fields", tabs, sidebar]` | replaces the active field lists |
| `["log", method, ...args]` | calls `window.log[method](...args)`, args in CVE-1 (bare numbers allowed); used to build the publisher's log |
| `["select", selected, hovered]` | `window.selection.getSelectedTime()` and `getHoveredTime()` (numbers or `null`) |
| `["tune", key, value]`, `["untune", key]` | `getTuner().publish` and `unpublish` |
| `["dump"]` | records the canonical dump of `window.log` |
| `["nt4", method, ...args]` | `nt4-client` only (`connect` builds the client once, then calls `connect()`; `stop` calls `disconnect()`). Methods: `subscribe`, `subscribeTopicsOnly`, `unsubscribe` (an argument `k` that indexes the subscription ids returned so far means that id; anything else is passed as is), `clearAllSubscriptions`, `publish`, `unpublish`, `setProperties`, `setPersistent`, `setRetained`, `sample` (`addSample`), `sampleAt` (`addTimestampedSample`), `connect`, `disconnect`, and `time` (records `nt4` `["time", client µs, server µs, latency µs]`). Args in CVE-1 (bare numbers allowed) |

Server ops shared by every mock: `["up"]`; `["down", "refuse" \| "hang"]` (new connections and
requests are refused or never answered, open ones stay); `["mute", bool]` (pushes on open
connections are dropped; handshakes and HTTP replies still go out); `["drop"]` (the server closes every open
connection, websockets first, each in opening order). Per protocol (each server class documents
its own behaviour in detail):

| Protocol | Config | Steps |
|---|---|---|
| `nt4` (`src/live/nt4Server.ts`) | `version` (`4.1` or `4.0`), `ports` ([5810]), `offsetUs` (server time is `(epochMs + t) * 1000 + offsetUs`), `topics` ([{name, type, properties?, value?}]) | `announce`, `unannounce`, `properties`, `set`, `batch`, `setraw`, `text`, `textraw` (a string sent verbatim), `binary` (hex, or a list of hex pieces and `[hex, count]` repeats), `httpStatus` (the alive check's status from then on) |
| `rlog` (`RLOGServer`) | `port` (5800) | `["send", hex]` (one length-prefixed frame), `["sendraw", hex]` |
| `phoenix` (`PhoenixServer`) | `devices` ([{device, signals, values}]), `busUtil`, `startUs`, `points`, `stepUs` | `["devices", [...]]`, `["error", action, code]` |
| `ftc` (`FTCServer`) | `status` | `["status", {...}]`, `["send", json]`, `["sendtext", text]` |

NT4 values are packed by topic type (`double` is always float64, `float` float32, `int` the
smallest integer form, arrays element-wise, any other type takes a hex string as `bin`). Every
JSON value a mock writes must be safe integers or non-integers with magnitude in [1e-6, 1e21),
because JS and Rust print other numbers differently; the mocks reject anything else, and both
order object keys as `JSON.parse` does.

### Timing and network model

One loop (`src/live/loop.ts`) runs everything, in integer milliseconds. At each time it runs,
in order: the server (every client message arriving now in send order, then the server steps
for now in file order); the client steps for now (file order); then client tasks until none is
due, where a delivery (server message, IPC message, socket event) due now goes before any
timer, oldest first, and timers follow Blink's rules (see `src/live/clock.ts` and
`asrs_live::timers`). The emulated main process shares that one timer queue and those rules, so its heartbeat
interval and socket timers order against the renderer's by creation. Every task is followed by a
full microtask drain. Timers carry the HTML nesting level: one created in a timer task (or its
microtasks) at level n gets n + 1, an interval rises a level per repeat, and a delay under 4 ms
is raised to 4 ms when the creating level is above 5, so a `setTimeout(0)` in a fresh
`setInterval` callback waits 4 ms from the sixth repeat on (measured in Electron 39).
`requestIdleCallback` fires at its `timeout`, `requestAnimationFrame` 16 ms later and
`AbortSignal.timeout` at its delay, all as level-0 delayed tasks in the timers' due-time order;
Chromium's ordering of those against short DOM timers at an equal due time differs (#204). IPC
between the renderer and the emulated main process is a delivery with no delay.

Client traffic reaches the server `latencyMs` after it is sent and server traffic reaches the
client `latencyMs` later. Connection and request ids count up from 1 across websockets, HTTP
requests and TCP sockets. Websockets behave like the browser's: `send` while connecting
throws, a refused handshake fires `error` then `close`, a server drop fires `close` (1006),
`close()` fires `close` a round trip later, and messages arriving when not open are dropped.
`fetch` is GET only, and an abort reaches the server as `http-abort`. A URL that `new URL`
rejects (a robot address of `""`) makes `fetch` reject at once, as Chromium's does, without
reaching the server; it still takes an id. TCP sockets follow the Node behaviour main.ts
relies on: writes while connecting go out on connect, `setTimeout` is an idle timer refreshed
by connecting, reads and writes, a refusal emits `error` then `close`; `error` is emitted at
once and `close` as a later delivery, as Node emits it from the handle's close callback.

### Trace

Each line is `{"t":<ms>,"ev":<event>,"id":<connection>,"v":<CVE-1>}`, with `id` and `v` left out
where an event has none, in the order things happen.

| From | Events (`v`) |
|---|---|
| client | `ws-open` ([url, [protocols]]), `ws-send` (string or bytes), `ws-close`, `http-get` (url), `http-abort`, `tcp-open` ([host, port]), `tcp-send` (bytes), `tcp-close` |
| server | `ws-accept` (protocol, `""` for none), `ws-reject`, `ws-push` (string or bytes), `ws-drop`, `http-reply` ([status, body]), `http-refuse`, `tcp-accept`, `tcp-refuse`, `tcp-push` (bytes), `tcp-drop` |
| app | `status` (the status callback's value), `output` ([log index, the time supplier's value]), `main` ([name, data] for main messages other than RLOG's), `dump` ([log index, canonical dump]), `throw` (a client op threw, as a string), `nt4` (an `nt4-client` callback: `["announce" \| "unannounce" \| "properties", uid, name, type, properties]`, `["data", uid, name, timestamp, value]`, `["connect"]`, `["disconnect"]`) |

Log indexes number the distinct `Log` objects in the order the harness first sees them; 0 is
the hub's initial log. TCP chunk boundaries are recorded as written, but only the byte stream
is protocol-relevant.

### Rust side

`asrs_parity::live` mirrors the mocks: `Session::load` parses a session (objects in
`JSON.parse` order), `ServerHost` is the sans-IO server on the same clock (`arrive(sent_at,
event)`, `next_time()`, `process(t)` returning server events), `ClientEvent::line` and
`ServerEvent::line` write trace lines, and `replay` checks a recorded trace's server events
against the Rust mock line for line. A port's live test feeds its client's traffic to
`ServerHost`, delivers the returned events `latencyMs` later under the loop rules above, and
compares its own trace with the vector. `crates/asrs-live/tests/support` is that client side for
the N units: the loop, browser `WebSocket` and `fetch` over `ServerHost`, and a `Driver` trait a
port implements to run a session (`tests/nt4_sessions.rs` uses it for every NT4 session).

## DOM harness

`src/dom.ts` (entry `dom`) is the oracle for the hub's models, controllers and renderers: it
loads upstream's `www/hub.html` in jsdom 27 and evaluates `domPage.js` inside that window.
`domPage.js` is a browser bundle of `src/dom/page.ts`, which installs `src/dom/env.ts` and then
imports upstream's real `src/hub/hub.ts`, so `Tabs`, `Sidebar`, `SelectionImpl`, `Timeline`,
every controller with its `SourceList`, and every renderer run unmodified. The one substitute
is `Field3dRendererImpl` (WebGL), replaced by `src/dom/field3dStub.ts`; the 3D command is still
recorded. The driver plays main's part over the hub's `MessagePort`: it sends the same start-up
messages `main.ts` sends on `dom-ready`, then the fixture's steps.

```sh
cargo xtask oracle dom <spec or fixture file>         # vector lines on stdout, the spec's backend
cargo xtask oracle gen dom/tabs/line-graph            # regenerate one committed vector
```

Blink (Electron 39.2.7's Chromium) is the DOM of record: new fixtures run there, and jsdom is a
local pre-check that families leave once they are flipped (`"backend": "jsdom"` in a spec marks
one not flipped yet). The jsdom specifics below (`innerText`, text codecs, `jsdomError`) apply
to that backend only.

### What is deterministic

- Time: `Date`, `Date.now()` and `performance.now()` read a fake clock that starts at
  `startTime` (default `1767225600000`, 2026-01-01T00:00:00Z) and moves only in `advance` and
  `frames` steps. `setTimeout`, `setInterval` and `requestAnimationFrame` are queued and run in
  time order by those steps: a timer due at or before the next frame runs first, frames come
  every `framePeriod` ms (default 16). Microtasks settle after each callback and each step.
- `Math.random` is mulberry32 seeded with `seed` (default 1), returning `u32 / 2^32`, so tab
  UUIDs and everything else drawn from it repeat exactly. `crypto.getRandomValues` and
  `crypto.randomUUID` draw from the same stream (one byte per draw).
- Layout: nothing reads CSS. `getBoundingClientRect`, `client*`, `offset*` and `scroll*` sizes
  come from layout rules, fixture rules first, then `src/dom/layout.json` (a 1100x650 window
  with the hub's main regions). The first rule whose selector matches an element decides:
  `"rect": [x, y, w, h]`, `"parent": true` (the parent's rect), or `"text": <px>` (width from
  the shared advance table at that size, height `round(1.25 * px)`, at the parent's origin).
  Unmatched, hidden (`hidden` attribute or inline `display: none` on it or an ancestor; CSS
  classes are not consulted) and detached elements are `0x0`. `innerWidth`, `innerHeight` and
  `devicePixelRatio` come from `window`; the default rules assume 1100x650, so a fixture with
  another window size overrides the region rules too.
- `matchMedia` answers `prefers-color-scheme` from `window.theme`; everything else is false.
- `innerText` (jsdom has none) sets text nodes with `<br>` for line breaks and reads them back.
- Images never load. An `<img>` whose `src` is a key of `images` reports that `[w, h]` as its
  natural size and fires `load` on the next timer turn; any other image stays incomplete at 0x0.
- Every 2D context is a recording context (`src/dom/recordingCanvas.ts`); WebGL contexts are
  `null`.

### The recording canvas

Each call and assignment is recorded in the form `asrs_canvas::trace` decodes
(`asrs_canvas::trace::call_from_value`): `[method, ...args]` and `["=", property, value]`.
Gradients assigned to a style are recorded as `{"linearGradient" | "radialGradient" |
"conicGradient": [...], "stops": [[offset, colour], ...]}` with the stops present at assignment
(the port's trace has no conic gradients yet), patterns as `{"pattern": "@<src>", "repetition"}`, images as `"@<src>"`,
canvases as `"@canvas:<id>"`, `getImageData` as `[..., "r<n>"]` naming a register that a later
`putImageData` refers to. Trailing `undefined` arguments are dropped, as WebIDL does for
optional arguments. Queries (`measureText`, `isPointInPath`, `getLineDash`, ...) and factories
(`create*Gradient`, `createPattern`, `createImageData`) are not recorded. Calls the port's
`Call` enum does not know (`setTransform`, shadows, ...) are recorded all the same, so a painter
the port cannot express yet shows up in its trace. Property reads return the last assigned
value, not Chromium's normalized one.

Setting the canvas `width` or `height`, or calling `reset()`, resets the context and starts a
new frame, so a trace is
the display list since the last reset, and `frame` counts resets.

`measureText` uses the advance table in `src/canvas/fixedAdvance.json`, which
`asrs_canvas::text::FixedAdvance` embeds too: per UTF-16 code unit (tabs, newlines and NBSP
first become spaces), the first `[first, last, em]` range or `defaultAdvance`; code units in
`blank` have no ink; others ink `[0, inkTop, advance, inkBottom]` em; ascent and descent are
`round(ascent * size)` and `round(descent * size)`. All arithmetic is f32, step for step with
`asrs_canvas::text::measure_text`, and the font size follows `FontSpec::parse` (an invalid
`font` is ignored). Vector `dom/canvas/fixed-advance` pins the two sides together
(`crates/asrs-canvas/tests/dom_harness.rs`).

### Fixtures

A fixture is a spec `parity/specs/dom/<area>/<case>.entry.json`. Adding one never touches a
shared file: the spec holds the fixture, and its vector is `dom/<area>/<case>`.

```json
{
  "entry": "dom",
  "inputs": ["../logs/robot.json", "../assets/minimal.json"],
  "fixture": {
    "window": { "width": 1100, "height": 650, "devicePixelRatio": 1, "theme": "light", "framePeriod": 16 },
    "seed": 1,
    "startTime": 1767225600000,
    "assets": "../assets/minimal.json",
    "preferences": { "theme": "dark" },
    "images": { "bundledAssets/Field2d_2026FRCFieldV1/image.png": [3900, 1800] },
    "layout": [{ "selector": "div.renderer-content", "rect": [300, 92, 800, 358] }],
    "steps": [
      { "log": "../logs/robot.json" },
      { "main": "new-tab", "data": 1 },
      { "eval": "window.tabs.tabList[window.tabs.selectedTab].controller.leftSourceList.addField('/Drive/LeftVelocity')" },
      { "advance": 100 },
      { "capture": ["commands", "state", "canvas"], "label": "after-add" }
    ]
  }
}
```

Fixture keys, all optional:

| Key | Meaning |
|---|---|
| `window` | viewport size, device pixel ratio, colour scheme and frame period (defaults above) |
| `seed`, `startTime` | `Math.random` seed and the clock's start |
| `version` | the `set-version` data (default darwin `25.0.0` arm64, upstream's version) |
| `preferences` | merged onto `DEFAULT_PREFS` with upstream's `mergePreferences` before `set-preferences` |
| `assets` | the `set-assets` data (default `null`); `parity/specs/dom/assets/minimal.json` has one 2D field, one 3D field and one robot |
| `typeMemory`, `state` | sent as `restore-type-memory` and `restore-state` at start-up when present |
| `boot` | replaces the whole start-up message list, as `[[name, data], ...]` |
| `images` | natural sizes by `src` |
| `layout` | layout rules tried before the defaults |
| `ignoreMain` | hub-to-main message names not to record (default `save-state`, `save-type-memory`, `update-touch-bar-slider`, which repeat on timers; capture `state` instead) |
| `sourceLists` | `true` records every call into the hub's `SourceList` instances (see "Source lists" below) |
| `geometry` | `"scripted"` (default) or `"real"`, Blink only (see "Blink backend") |
| `steps` | run in order after start-up |

`assets`, `preferences`, `state`, `typeMemory` and a `log` step may be a path to a JSON file,
relative to the spec, instead of the value; the file must be listed in `inputs` or the run
fails, so the generator hash always covers it. Shared files live in `parity/specs/dom/logs/`
and `parity/specs/dom/assets/` (plain `.json`, never taken for specs).

Steps:

| Step | Effect |
|---|---|
| `{"log": [[method, ...args], ...]}` | calls `window.log[method](...args)` with CVE-1 arguments (bare JSON numbers, strings, booleans and arrays are fine), then `sidebar.refresh()` and `tabs.refresh()` as the hub does after a decode (`"newFields": false` skips the tabs) |
| `{"main": name, "data": value}` | delivers a main-to-hub message (`"cve1": value` instead of `data` for values JSON cannot express) |
| `{"advance": ms}`, `{"frames": n}` | runs timers and frames for `ms`, or `n * framePeriod`, milliseconds |
| `{"eval": "expression"}` | evaluates JavaScript in the page (`window` is the hub's) |
| `{"event": {"target", "index", "kind", "type", "init", "props"}}` | dispatches `new window[kind](type, {bubbles: true, cancelable: true, ...init})` on the `index`th match of the selector `target` (or `"window"`, `"document"`); `kind` defaults to `Event`; `props` assigns own properties (like an input's `value` or `scrollTop`) before dispatch |
| `{"selection": [method, ...args]}` | calls `window.selection[method](...args)` with CVE-1 arguments; `setLiveConnected` connects a scripted supplier that returns the last `liveTime` step's value |
| `{"liveTime": <cve1>}` | sets the scripted live timestamp the `setLiveConnected` supplier returns |
| `{"tuner": [keys...]}` | installs a stub `window.tuner` tunable for the listed keys and writes `{"tuner": ["publish", key, value]}` or `["unpublish", key]` records; `null` removes it |
| `{"form": {...}}` | a form-control probe (see "Form vectors") |
| `{"capture": [...], "label": ...}` | writes a `{"capture": label, "time": ms}` line, then each capture below |

Captures (a string or `{"kind": ..., ...}`):

| Capture | Lines written |
|---|---|
| `state` | `{"state": {sidebar, tabs}}`, the hub's `saveState()` |
| `commands` | `{"command": {index, type, title, uuid}, "value": <command>}` for every tab whose renderer was called since the last `commands` capture, with the command it was last given (snapshotted when `render` was called) |
| `canvas` | `{"canvas": "<class>#<n>", "frame", "width", "height", "calls": [...]}` for every 2D canvas drawn since the last `canvas` capture; `selector` limits it to matching canvases |
| `dom` | `{"dom": selector, "nodes": [...]}`: for the first match (`"all": true` for every match), each element in document order as `[depth, tag, class, hidden, text, extra]`, where `text` joins the element's own non-blank text nodes and `extra` holds the `attributes` asked for (default `title`) plus `value` of form controls and `checked` of checkboxes and radios |
| `selection` | `{"selection": {mode, hoveredTime, selectedTime, renderTime, currentLiveTime, grabZoomRange, timelineRange, timelineIsMaxZoom, buttons}}` where `buttons` records each control button's `hidden` flag and the inline `--show-lock-buttons` value |
| `eval` | `{"eval": expr, "value": <result>}` or `"throw"` |
| `measure` | `{"measure": {font, align, baseline, text}, "metrics": {...}}` per string of `texts`, from the recording canvas (`null` when the font is invalid) |
| `sourceLists` | `{"sl": id, "dump": {...}}` per recorded source list (title, state, `getState(true)`, config, hand icon, drag highlight style, buttons, and per item the icon data, `type-color`, type name, key, its decoration and title, hide icon and title, `hidden`/`child`/`parent-highlight` classes, preview visibility, text and item height), then `{"slMemory": window.typeMemory}` |

Also written as they happen: `{"main": name, "data": ...}` for every hub-to-main message not
ignored, `{"drag": <data>, "end", "x", "y"}` for every `drag-update` event the hub dispatches during a field
drag, `{"tuner": ...}` records from a `{"tuner"}` stub step, and
`{"exception": where, "error": "<name>: <message>"}` when upstream code throws in
a timer, frame, event listener (reported by jsdom, `where` is `event`), message handler,
`log` step's `sidebar.refresh()` or `eval`, or a promise rejection goes unhandled (the run goes on, as a page would). Every value
is CVE-1 inside a plain JSON object, so Rust reads it with `asrs_parity::record_vectors!` and
`Record::value`.

Baseline vectors: `dom/tabs/<tab>` (one per tab type, each with the robot log, the minimal
assets and a few fields: commands, state, canvas traces, selection and DOM text of the
renderer and controls at a selected time and again while hovering), `dom/hub/selection-sidebar`
(timeline hover and click, playback, step, zoom, sidebar expand, collapse and search) and
`dom/canvas/fixed-advance`.

### Tabs model

`dom/tabs-model/<case>` (`crates/asrs-hub/tests/tabs_dom.rs`) drives `Tabs` through main messages,
tab-bar and controls-handle events and restores of well-formed and malformed states. Until the
controllers and renderers are ported the port runs with stub halves, so each fixture's first
step is a `/*tabs:recorder*/` eval that wraps `window.tabs.addTab` to list every created tab
(with its creation-time UUID) and log its `restoreState` calls, plus each Table renderer's
`getTimestampRange` results and Table controller's `addRendererRange` calls, which
`/*tabs:ranges*/` drains along with the renderer UUIDs. Every capture then starts with
`/*tabs:info*/`, which reports per created tab what its controller and renderer save, its
`showTimeline()` and active fields; the stubs answer from it. The other evals compare what
`Tabs` owns: `window.tabs.saveState()`, `/*tabs:view*/` (tab bar classes and text, hidden flags,
CSS variables, drag highlight and item), `/*tabs:restores*/` and `[...window.tabs.getActiveFields()]`.
`/*tabs:tag*/` sets `data-i` on the title elements so layout rules can place them.

### Source lists

With `"sourceLists": true` (`src/dom/sourceLists.ts`) the run records what the port's
`asrs_model::source_list::SourceList` needs to replay every list: `{"sl": id, "create": {uuid,
title, config, editButton, numberPreview, supplemental}}` when a list is first seen (the lists
the hub builds at load are registered before start-up), then one `{"sl": id, "op": [...], "dark"}`
line per outermost call: the public methods, `handleItemDrag` and `handleFieldDrag` (with the
list's client rects and the supplemental states), each animation frame (`["frame", isDark,
renderTime]` with the `getNumberPreview` calls it made), the item and list mouse and click events,
and the type and clear prompt callbacks. A hooked call that throws carries `threw`. Also
recorded: `{"slTypeMemory": ...}` whenever `window.typeMemory` is replaced, `{"sl": id,
"dragStart": {...}}` for `startDrag` from a list, and `{"slLog": ops}` for each `log` step. The
fixture's `window.__oracleTheme` overrides `window.theme` for `matchMedia`, so a step can switch
the colour scheme. Source-list fixtures tag items with a `data-i` attribute before each event,
which event targets name.

`dom/source-list/*` run on Blink with `"geometry": "real"`, so the lists' layout reads are
`read` records (the rects in `itemDrag` and `fieldDrag` ops are a quiet snapshot the replay does
not use). Their first step defines `window.__slMouse(type, target, root, anchor, dx, dy)`, which
dispatches a `MouseEvent` on `target` (or `window`) at `dx` right of the list root `root` and `dy`
below the top of item `anchor`, scaled to the item's height and rounded (`"root"` measures from
the root's top; an anchor past the last item counts 30 px items below it). It reads the layout
inside `__oracleQuiet`. Every `sourceLists` capture also evaluates `/*sl:widths*/`: each list
root's class and, per item, its type name and `--type-width`. A capture evaluating `/*sl:scroll*/ [rootClass,
list.scrollTop = n]` scrolls a list mid-drag, as a user would to reach an item below the box; its
`scrollTop` write is a `write` record the replay consumes.

### LineGraph controller

`dom/line-graph/<case>` (`crates/asrs-tabs/tests/line_graph_dom.rs`) replays every LineGraph
controller call and every renderer frame. `node oracle/tools/lineGraphSpecs.mjs` writes the specs and their logs
(`parity/specs/dom/logs/line-graph-*.json`); edit the script, not the specs. Each fixture sets
`"sourceLists": true` and starts with a `/*lg:recorder*/` eval, which hooks every LineGraph
controller (the hub's default tab and any `addTab` creates) and writes inline records:

| Record | Written |
|---|---|
| `{"lg": "create", "id", "uuids"}` | when a controller is hooked; `uuids` are its left, right and discrete list UUIDs |
| `{"lg": "command" \| "preview" \| "save" \| "activeFields", "id", "dark", "ts", "args"?, "value" \| "threw", "reads"}` | after `getCommand`, `getPreview` (key and time only), `saveState` and `getActiveFields` return; `reads` lists each outermost `window.selection` getter called during it with its result, and `ts` is `preferences.timestamps` |
| `{"lg": <method>, "id", "phase": "begin", "args", "dark"}` and `{"lg": <method>, "id", "phase": "end", "threw"?}` | around `restoreState`, `editAxis`, `clearAxis`, `setRobotModeVisible` and `refresh`; the list ops and nested `getCommand` between them belong to the call |
| `{"lg": "edit", "id", "list", "rect"}` | before an axis's edit button handler runs, with the button's rect |
| `{"lg": "render", "id", "width", "height", "dpr", "dark", "prefs", "threw"?, "writes", "size"?, "calls"?, "style"}` | after the tab's `LineGraphRenderer.render` returns or throws; `width` and `height` are the canvas's client size, `prefs` is `[timestamps, hasScrolledLineGraph]` (or undefined before `set-preferences`), `size` and `calls` are the canvas's pixel size and its recorded calls when the render reset it, and `style` is the scroll overlay's `top`, `left`, `right`, the help overlay's `display` and whether it has `fading-out` |
| `{"lg": "event", "type", "x", "shift", "targets", "writes"}` | after a `mousemove`, `mousedown`, `mouseup`, `click`, `contextmenu` or `mouseleave` dispatch; `targets` holds, per hooked renderer, `[id, ROOT.hidden, on overlay, ROOT x, overlay x, overlay clientWidth]` |
| `{"lg": "scroll", "id", "args", "pointer", "prefs", "writes", "style"}` | after the renderer's scroll sensor callback (wheel input, which fixtures call directly) |

`writes` lists the outermost `window.selection` setters (`setHoveredTime`, `setSelectedTime`,
`setGrabZoomRange`, `finishGrabZoom`, `goIdle`, `applyTimelineScroll`) called during the
render, dispatch or callback. The harness answers the port's selection reads from `reads`, so
a read upstream did not make, or one it skipped, fails the case. Each `render` runs the port's
`LineGraphRenderer` on the command the port's controller built in the preceding `command`
record and compares the trace, the writes and the styles; `event` and `scroll` records drive
its pointer and wheel handlers.

### Field2d controller

`dom/field-2d/<case>` (`crates/asrs-tabs/tests/field_2d_dom.rs`) replays every Field2d
controller and renderer call. `node oracle/tools/field2dSpecs.mjs` writes the specs and their
logs (`parity/specs/dom/logs/field-2d-*.json`); edit the script, not the specs. Fixtures get the
three test fields as `assets` unless they say otherwise, so the renderer's camera math runs.
The recorder eval hooks every Field2d tab:

| Record | Written |
|---|---|
| `{"f2d": "create" \| "command" \| "save" \| "activeFields", "id", "dark", "prefs", "assets", "value" \| "threw", "reads"?}` | on construction and after `getCommand`, `saveState`, `getActiveFields`; `prefs` is `{cs, ftc, ts}` of `window.preferences` |
| `{"f2d": <method>, "phase": "begin" \| "end", ...}` | around `restoreState`, `refresh`, `newAssets` |
| `{"f2d": "select" \| "source" \| "orientation" \| "size", ...}` then `{"f2d": "controls", "controls"}` | before the control's handler runs, and the control DOM on the next timer turn |
| `{"f2dr": "render", "command", "dom", "camera", "threw"?, "size"?, "style"?, "calls"?, "heatmap"?}` | after `render`: the DOM it read and the private camera fields after it; when the render reset the canvas, its pixel size, the `width`, `height` and `transform` strings it assigned to `CANVAS.style` (captured by accessors on the style object, since Blink re-serializes lengths on read), its recorded calls with the heatmap canvas named `@heatmap`, and h337's `getData()` |
| `{"f2dr": "camera", "id", "camera"}` | when a fixture calls `window.__f2dCamera(renderer)`, after dispatching mouse events: the drag pan calls a callback the recorder cannot wrap |
| `{"f2dr": "scrollEvent"}` then `{"f2dr": "scroll", "args"}` | a `scroll` event on the sensor and the callback it dispatched, if any; the port takes the deltas through its wheel input and must produce the same cursor |
| `{"f2dr": "mouse", "type", "button", "shiftKey", "x", "y"}` | before the sensor's and the camera menu's listeners |
| `{"f2dr": "save" \| "aspectRatio" \| "restoreState" \| "set2DCamera", ...}` | renderer state calls |

Each `render` runs the port's `Field2dRenderer` on the recorded command and compares the
throw, the frame (or its absence), size, style strings, trace call for call and heatmap store;
a `requestAnimationFrame` exception must match what the port's render or list frame threw.
`node oracle/tools/field2dTraces.mjs` copies a few painted frames into
`oracle/src/canvas/field2dTraces.json`, which `scenes.ts` turns into the `field2d-trace-*`
canvas pixel scenes: each field image is a generated stand-in at its recorded natural size and
the heatmap canvas is transparent (its pixels are gated by `cargo xtask canvas heatmap`).

### Table, Console, Statistics and Metadata renderers

`dom/{table,console,statistics,metadata}-view/<case>` (`crates/asrs-tabs/tests/dom_tab_views.rs`)
record the DOM the four renderers leave after every `render` and every driven event.
`node oracle/tools/r11Specs.mjs` writes the specs; edit the script, not the specs. The table and
console fixtures use `"geometry": "real"`. Their recorder eval adds `window.__r11`, which
drives a renderer by id (`filter`, `highlight`, `row`, `jump`, `export`, `restore`, ...),
makes further renderers on the hub's or satellite.html's markup (`make`), and renders a command
on one directly (`render`) or the current one of the tab's controller (`share`).

| Record | Written |
|---|---|
| `{"r11": "create", "id", "kind", "hasController", "dom"}` | when a renderer is hooked |
| `{"r11": "begin", "id", "op", "dark", "timestamps"}` | before an operation: `op` is `["render", command]` or the event and its arguments, `timestamps` is `window.preferences.timestamps` |
| `{"r11": "selection", "call"}` | a `window.selection` call the operation made |
| `{"r11": "end", "id", "threw"?, "result"?, "range"?, "histogram"?, "dom"}` | after it, with `saveState()`'s result, `getTimestampRange()` or whether the Statistics renderer replaced its histogram's data, where they apply |

`dom` per kind: the Table's header cells, fillers (`style.height` as assigned) and pooled rows
`[classes, hidden, cell texts]`; the Console's field cell, buttons and rows `[classes,
timestamp, value nodes]` with nodes as `[localName, attributes, children]` or text; the
Statistics rows `[classes, [[text, colSpan, style.color as assigned], ...]]`; the Metadata
headers and rows `[[text, classes], ...]` with the alert's and table's `hidden`.

### Console layout

`dom/console-layout/*` (`crates/asrs-app/tests/console_layout.rs`) record what Blink lays the Console
tab's rows out as, with `"geometry": "real"`. `node oracle/tools/u13Specs.mjs` writes the specs and
their logs; edit the script, not the specs. `wrap`, `wrap-2x` and `wrap-narrow` capture an
`/*console-layout:rows*/` eval: per row its `clientHeight` and per cell its width and each text
node with its client rects, for 220 lines of random text, tabs, line breaks and ANSI renditions, then
with a filter typed. `anchor` scrolls (`/*console-layout:scroll N*/`), filters rows away and back,
and captures `scrollTop` (`/*console-layout:top*/`), which Chromium's scroll anchoring moves. The text is
macOS's Courier, so the test runs there only; the log leaves out CJK, whose fallback advance differs
from Blink's.

### Statistics and Metadata layout, line breaking

`dom/statistics-layout/*` and `dom/metadata-layout/*`
(`crates/asrs-app/tests/statistics_metadata_layout.rs`) record what Blink lays the two tabs' tables
out as, with `"geometry": "real"` at 1x, 1.5x, 2x and in a narrow window. `node
oracle/tools/u14Specs.mjs` writes the specs; edit the script, not the specs. Before each capture a
`/*layout:command*/` eval replaces the controller's `getCommand` and returns the command; the
capture's eval returns the container's boxes and every row as `[height, cells]`, a cell as its rect
and its text nodes with their line rects. The text is macOS's, so the test runs there only. CJK
advances in Courier differ (#273): only the vertical positions of those cells are compared, and the
1.5x fixture leaves CJK out.

`dom/line-break/*` (`crates/asrs-canvas/tests/line_break.rs`) record where Blink starts a line in a
box of width 0: `pairs` for every pair of printable ASCII characters between two letters, `strings`
for words, punctuation, other scripts and random mixtures of them.

### Mechanism and Points renderers

`dom/mechanism/<case>` and `dom/points/<case>` (`crates/asrs-tabs/tests/mechanism_points_dom.rs`)
record every `render` of every `MechanismRenderer` and `PointsRenderer`, both of which draw
with DOM elements rather than a canvas. `node oracle/tools/r08Specs.mjs` writes the specs,
`parity/specs/dom/logs/r08.json`, the `dom/css-style/*` specs and the hub-level
`dom/tabs/{mechanism,points}-r08` cases; edit the script, not the specs. The fixtures run on
Blink with `"geometry": "real"`, so container sizes are Blink's layout (fractional at
`devicePixelRatio` 1.5 and 2). Their recorder eval adds `window.__r08Render(type, index,
command)`, which renders a command directly on the `index`th tab of that type, and
`window.__r08Size(width, height)`, which resizes `div.renderer-content` inside
`__oracleQuiet`.

| Record | Written |
|---|---|
| `{"r08": "create", "kind", "id", "dom", "aspect"}` | when a renderer is hooked: its root element (`svg.mechanism-svg` or `div.points-background`) and `getAspectRatio()` |
| `{"r08": "render", "kind", "id", "command", "dark", "reads", "assigns", "threw"?, "created", "dom", "aspect"}` | after `render` returns or throws: the container reads it made (`clientWidth`/`clientHeight`, or `rect` for `getBoundingClientRect()`), each inline style assignment as `[target, camelCaseProperty, value as assigned]` with `target` `"root"` or the creation index of the element it made, how many elements it created or cloned, and the root's tree |

A tree node is `[localName, [[name, value], ...], [children...]]`, read with
`getAttributeNames()`, which first syncs the `style` attribute. Blink appends that attribute
at the first sync after the inline style is mutated, not when it is first set, and it parses
inline style on SVG elements with quirks behaviour (hashless hex colours on `background-color`,
unitless lengths). `dom/css-style/*` pin the serializations on the element types the renderers
use: each `{"eval"}` value lists `[property, namespace, tag, [[input, getPropertyValue, style
attribute or null], ...]]` per group, and `declarations` replays attribute and style scripts
with the attribute list after each `["sync"]` step and at the end.

### Swerve and Joysticks renderers

`dom/swerve/<case>` and `dom/joysticks/<case>` (`crates/asrs-tabs/tests/swerve_joysticks_dom.rs`)
record every `render` of every `SwerveRenderer` and `JoysticksRenderer`. `node
oracle/tools/r07Specs.mjs` writes the specs, `parity/specs/dom/assets/r07.json` and the hub-level
`dom/tabs/{swerve,joysticks}-r07` cases; edit the script, not the specs. The fixtures run on Blink
with `"geometry": "real"`. Their recorder eval adds `window.__r07Render(type, index, command)` and
`window.__r07Size(width, height)`, as the Mechanism and Points one does.

| Record | Written |
|---|---|
| `{"r07": "create", "kind", "id", "aspect", "state"}` | when a renderer is hooked: `getAspectRatio()` and `saveState()` |
| `{"r07": "assets", "assets"}` | before a Joysticks `render`, when `window.assets` is another object than at the last one |
| `{"r07": "load", "id", "index"}` | on each `load` event of `IMAGES[index]` |
| `{"r07": "render", "kind", "id", "command", "dark", "dpr", "reads", "style", "threw"?, "loads"?, "aspect", "size"?, "calls"?}` | after `render` returns or throws: the `clientWidth` and `clientHeight` reads it made, the `CANVAS.style` width and height strings as assigned, `imageLoadCount` before it (Joysticks) and, when it reset the canvas, the pixel size and recorded calls |

The recording canvas never throws, so each renderer's `arc` and `ellipse` calls first run on a
`CanvasRenderingContext2D` of an about:blank frame, whose `IndexSizeError` reaches upstream before
the call is recorded. NaN is written as the canonical NaN: V8's tiers give `x * -1` different NaN
signs, and which tier runs depends on when a background compile finishes.
`node oracle/tools/r07Traces.mjs` copies a few painted frames into
`oracle/src/canvas/r07Traces.json`, the `swerve-trace-*` and `joysticks-trace-*` canvas scenes.

Not modelled on jsdom: CSS (so no computed styles or real text layout), scrolling (`scrollTop`
stays 0). On either backend: workers and historical or live sources (logs are built with `log`
steps), drag sessions other than through dispatched events, the documentation iframe's
content, and 3D rendering.

### Histogram

`tools/histogramSpecs.mjs` writes `dom/histogram/*`: upstream's StatisticsRenderer with its real
Chart.js 4.4.0, on Blink with scripted geometry. Its first step installs a recorder that hooks
each chart as its tab opens and writes one record per outermost entry into it, with the canvas
calls that entry made (`reset` when it set the canvas size, which drops the calls before):
`create`, `resize` (the attach, or a throttled ResizeObserver or scale change), `render`, `event`
(a throttled mouse event) and `draw` (an animation frame). After a layout the record also holds
the chart area, both scales with their tick values and labels, every bar's
`[x, y, base, width, height]` and the legend. `observe`, `dpr` and `dom` are the inputs, and
`frame` marks an animation frame. Commands reach the renderer through the hub's tab loop by
replacing the controller's `getCommand`, except in `controller`, which uses a log.
`crates/asrs-tabs/tests/histogram_dom.rs` replays them, throws included.

`tools/histogramTraces.mjs` extracts painted frames from those vectors into
`src/canvas/histogramTraces.json`, the `histogram-*` canvas scenes.

### Blink backend

`src/domBlink.ts` (entry `domBlink`, chosen by `"backend": "blink"`) runs the same fixture and
the same `domPage.js` in Blink. It spawns Electron's main process on `domBlinkMain.mjs`
(`src/domBlinkMain.ts`, no upstream main process) with `--force-device-scale-factor=<window
devicePixelRatio>` and `--lang=en-US`, which opens an offscreen `BrowserWindow` with the
fixture's content size and `nativeTheme` set to its theme, loads upstream's `www/hub.html` and
serves, in place of `bundles/hub.js`, the fixture followed by `domPage.js`. The page's first
module, `src/dom/blinkBoot.ts`, provides the output sink and exception records `dom.ts` provides
for jsdom, then `env.ts` installs the same fake clock, timers, frames, seeded random, recording
canvas, `matchMedia`, `ResizeObserver` and image sizes. Before the first step the run fails
unless `innerWidth`, `innerHeight` and `devicePixelRatio` equal the fixture's, the zone and
`Intl` locales are the policy's and the page has no web fonts. Records are read back over the
DevTools protocol.

Determinism on Blink:

- Only `hub.html` and its stylesheets load. Every other request (images, `<object>` icons,
  iframes, workers, `fetch`, http and https) is answered with a body that never arrives, so no
  load or error event lands at a time the steps do not control. A request left unanswered would
  fail after Electron's 30 s timeout, and page load never completes. Image sizes still come from
  `images`, as on jsdom.
- Blink dispatches scroll events, runs layout and observers only in a real rendering frame, so
  every settle after a callback or step waits for one real frame and then one task. Native
  effects of a step (a scroll event after a `scrollTop` write, `selectionchange`) therefore run
  before the next step, at the same point on every run.
- CSS transitions and animations are off (`transition: none; animation: none`), because they
  run on the real clock; geometry reads see the end state.
- `requestIdleCallback`, `Element.animate` (Web Animations) and `WebSocket` are Blink's own and
  run on the real clock. jsdom lacks the first two, so upstream code that feature-tests for them
  (`NT4Source`'s idle loop, `VideoController`'s button animation) takes another path on each
  backend. No fixture on Blink reaches them yet; one that does needs them faked first (#145).
- `innerText`, `TextEncoder` and `TextDecoder` are Chromium's own. Exceptions in event
  listeners reach the page's `error` event and unhandled rejections its `unhandledrejection`
  event; both are written as `{"exception": "event" | "unhandledRejection", "error": "<name>:
  <message>"}`, the text jsdom's records use.

Geometry, fixture key `"geometry"`:

- `"scripted"` (default): the layout rules answer `getBoundingClientRect`, `getClientRects`,
  `client*`, `scroll{Width,Height}` and `offset*` exactly as on jsdom, and `scrollTo` and
  `scrollIntoView` do nothing. Geometry is a declared input; the families that keep it are those
  whose controllers only do pointer math inside one rectangle. Scroll positions, `MouseEvent`
  coordinates and everything else are Blink's.
- `"real"` (Blink only): Blink lays the page out. Every geometry read upstream makes is written
  where it happens as `{"read": name, "el": path, "value": <CVE-1>}` (`getBoundingClientRect`
  as `[x, y, width, height]`, `getClientRects` as a list of those, `client*`, `scroll*`,
  `offset*` as numbers), and every `scrollTop` or `scrollLeft` assignment and `scrollTo`,
  `scrollBy`, `scrollIntoView` or `scrollIntoViewIfNeeded` call as `{"write": name, "el",
  "value"}`. `el` is
  `tag.class.class:index` for each element from `<body>` down, joined by `>`. Harness code reads
  inside `window.__oracleQuiet(fn)`, which records nothing; fixtures use it too, for scrolls
  that are inputs rather than upstream behaviour. `asrs_parity::geometry::ScriptedReads`
  answers a port's reads from these records in order and checks its writes, failing on any read
  or write upstream did not make; `dom/tabs/table-real` replays the Table renderer this way
  (`crates/asrs-tabs/tests/table_real_dom.rs`), as do the other `dom/tabs/table*` fixtures the
  whole tab (`crates/asrs-hub/tests/dom_tabs.rs`). Both also check every vertical read against
  `asrs_tabs::table::layout::TableLayout`, the model of Blink's layout the app answers from.

`window.__oracleTree(value)` gives fixture evals the CVE-1 tree of a value.

`cargo xtask oracle dom-ab` runs every committed `dom/*` and `form/*` spec on jsdom and on Blink
with scripted geometry and writes a Markdown report: per family, the specs that are
byte-identical and, for the others, the records only one backend wrote, by kind, with the first
differing line. A family with no difference flips to Blink with unchanged content; one that
differs stays on jsdom until its unit regenerates it from Blink and the port follows.

### Form vectors

`form/number-input`, `form/range-input`, `form/select` and `form/css-value` record Chromium's
form-control semantics as pure string functions. `node oracle/tools/formSpecs.mjs` writes their
specs; edit the script, not the specs. Each is a list of `form` steps:

```json
{"form": {"markup": "<input type=\"range\" value=\"0\" min=\"1\" max=\"1\" step=\"1\">",
          "ops": [["set", "max", "300"], ["set", "value", "59.6"]],
          "read": ["value", "valueAsNumber"]}}
```

The markup is parsed into a connected container (so attributes are set in markup order, as the
parser sets upstream's), then each op runs: `["set", path, value]` assigns a property,
`["setAttribute", name, value]`, `["appendElement", tag, [[path, value], ...]]` creates an
element, assigns the pairs and appends it, `["remove", index]` removes an element child. After
the parse and after each op the step writes `{"form": markup, "op": <op or null>, "threw"?,
"values": [{"path", "value"} | {"path", "threw"}, ...]}`. Paths are dotted property paths from
the parsed element, with numeric segments indexing collections (`options.1.value`,
`style.width`).

### Select input vectors

`form/select-keys` (`src/selectKeys.ts`, with `src/selectKeysMain.ts` as its Electron main
process) records what trusted input does to a closed `<select>` in a real Chromium page. Each
step of the spec's fixture is `{"markup", "focus"?, "ops"}`: the markup is loaded into an
offscreen window, the first `<select>` is focused unless `focus` is `false`, and each op runs:
`["char", text, ms, modifiers?]` types a character and `["key", name, ms, modifiers?]` presses a
named key (`Enter`, `Escape`, `Home`, `End`, `PageUp`, `PageDown`, `ArrowLeft`, `ArrowRight`,
`Backspace`), both through the DevTools protocol's `Input.dispatchKeyEvent` with the event
timestamp `ms` after a fixed origin, since type-ahead compares event times; `["press", null, ms]`
presses the mouse at the select's centre; `["set", property, value]`, `["blur"]` and
`["focus"]` are script. After the markup and after each op it writes `{"step", "op", "value",
"selectedIndex", "focused", "events"}`, where `events` lists the DOM events the select received
since the previous record. Keys and presses that open the native popup on macOS (up, down, F4,
space outside a type-ahead session, a press on an enabled select) would block in AppKit's menu
loop, so fixtures leave them out. Modifiers are `ctrl`, `alt`, `meta` and `shift`; `meta` combinations
can reach Electron's menu, so fixtures avoid them. The entry's generator hash also covers
`selectKeysMain` (`BORROWED_INPUTS` in `xtask/src/vectors.rs`).

`form/number-keys` is the same entry on the number inputs of the Statistics tab: the element may
be an `<input>`, for which `ArrowUp` and `ArrowDown` are keys too. Its fixture is written by
`oracle/tools/u14Specs.mjs` and covers `step` attributes, start values and modifiers.

### Number insertion vectors

`form/number-insert` (`src/genNumberInsert.ts`) records what Chromium lets the user insert into an
en-US `<input type="number">` in an offscreen window: each record is the shown text, the selection
(UTF-16 offsets), the inserted text, the text shown afterwards and the `value` then read. The
insertions are `execCommand("insertText")`, which takes typing's and pasting's route through
`BeforeTextInsertedEvent`. Fixed starting texts with every caret position come first, then seeded
random ones.

## Hub sessions

`hubSession` (`src/hubSession.ts`) runs a `dom` fixture on the same page bundle (`domPage.js`:
`src/dom/env.ts`, then the real `src/hub/hub.ts` in hub.html under jsdom) and adds what the hub
lifecycle needs:

- Workers. The page's `Worker` is a real worker thread, with the worker realm's `Date` pinned to
  1,000,000,000,000 as in `historicalSession`; messages are copied into the page realm. After
  every page task, worker messages go to the page one at a time from the oldest worker that has
  one or is still busy, until no worker is busy, so worker work takes no page time. Decode
  workers and `exportWorker.js` both run this way.
- Main's `historical-start`: the file named in the fixture (relative to it, and listed in
  `inputs` under a spec) is read, and `historical-data` arrives as a zero-delay page timeout.
- Live sources and the NT4 publisher are not connected; fixtures only reach their refusals.

Besides the `dom` records, a session writes `{"worker", "script"}`, `{"post", "type", "files" |
"key"}` and `{"terminate"}` for worker traffic, `{"console": "warn", "text"}` for the
`set-assets` load-failure warning and `{"exception": "worker", "error"}` for a throw out of a
worker message. Its generator hash also covers the `dom` entry's inputs
(`BORROWED_INPUTS` in `xtask/src/vectors.rs`).

`crates/asrs-hub/tests/hub_lifecycle.rs` replays `parity/specs/hub/lifecycle-*.entry.json`
through `asrs_hub::hub::HubModel` with the same scheduling and compares every record.

A `lifecycle-*` fixture may also watch the loading glow: its first capture installs a
`MutationObserver` on the document element's style, and `window.__glow.splice(0)` reads every
value the glow took since the last read, one per page task.

## Log routing and the export flow

`genFiles routing 0 0 dev <scenario>` (`src/files/routing.ts`) runs the real `main.ts` against the
`files` fakes: `isDsLog` and `openFilesSmart` through every caller (dropped files, the Open Log(s)
dialog, `open-file`, the AdvantageKit poll, startup), the windows they open with their `dom-ready`
sends, and the export flow from `prompt-export` to `write-export` and the closing warning. The
scenarios are `running`, `startup-plain`, `startup-argv`, `startup-ds` and `startup-ds-saved`; the
records are described in the file. `crates/asrs-app/tests/routing_vectors.rs` replays them.

## Electron app driver

`src/appMain.ts` runs the real upstream app: it sets an isolated userData directory, seeds it,
requires upstream's `bundles/main.js`, and drives the hub through `webContents.executeJavaScript`
and `webContents.sendInputEvent` only. `cargo xtask app` (`run`, `compare`, `normalize`, `check`)
builds the harness, launches one Electron process per scenario variant with the pinned
environment and the switches below, and compares runs. Scenarios live in `parity/scenarios/`.

Switches, all on the real command line because Chromium reads several of them before the main
script runs (the driver exits naming the expected set when one is missing):
`--force-device-scale-factor=<1|2>`, `--lang=en-US`, `--disable-accelerated-2d-canvas`,
`--js-flags=--random-seed=1` (V8's `Math.random`, so controller UUIDs repeat;
`asrs_js::time::V8MathRandom::from_seed(1)` is the same stream), `--force-prefers-reduced-motion`
(upstream's 3D camera otherwise eases to its default position over wall-clock frame deltas),
`--disable-partial-raster` (partial re-raster left one-pixel seams that differed between runs),
`--force-color-profile=srgb` (see below), and for `"gl": true` scenarios on Linux and Windows
`--enable-unsafe-swiftshader`, plus `--use-angle=d3d11-warp` with `--gl d3d11-warp`.

What the driver pins besides switches: windows are shown inactive (`show` becomes
`showInactive`, except for modal prompts, whose `show` begins a sheet) at the primary display's work-area origin plus 16 px with the scenario's content
size, ignore the real pointer, and never open DevTools; `fetch` and `net.fetch` in the main
process and non-loopback HTTP and WebSocket requests from pages are refused (the refused URLs
are in `run.json`); every `dialog` call is answered from the scenario's `dialog` actions, and an
unscripted one fails the run; `prefs.json` is the scenario's `prefs` plus its theme, and
`beta-2027.json` marks the beta welcome and survey done.

A session preload installs a tap in each page's main world before page scripts run. It records
every canvas context, and for 3D scenarios reads each WebGL drawing buffer with `gl.readPixels`
right after the animation-frame callback that drew into it (the buffer is only defined until the
compositor takes the frame), flipped to top-down rows. It also keeps the data of the last
`save-state` message the page posts on its message port, which is the HubState main saves.

Scenario file:

| Key | Default | Meaning |
|---|---|---|
| `page` | `"app"` | `app` runs upstream; `colour-patch` shows 512 known sRGB patches instead |
| `window` | `{"width":1100,"height":650}` | content size in CSS px |
| `scales`, `themes` | `[1,2]`, `["light","dark"]` | variants; one launch each |
| `gl` | `false` | 3D scenario: GL switches and drawing-buffer dumps |
| `prefs`, `userData` | `{}` | prefs merged by upstream over its defaults; extra JSON files seeded into userData |
| `actions` | | in order, below |

Actions: `{"open": ["app:drive" or a path], "merge": false}` (the hub's `open-files` message;
`app:<name>` is a log generated by `src/app/logs.ts` with upstream's `WPILOGEncoder` into
`inputs/`), `{"main": name, "data": ...}` (any main-to-hub message, dispatched on the hub's
`messagePort`, e.g. `new-tab`, `restore-state`), `{"eval": js}`,
`{"selection": [method, ...args]}`, `{"mouse": "move"|"down"|"up"|"click", "x", "y"}`,
`{"wheel": {"x", "y", "deltaX", "deltaY", "wheelTicksX", "wheelTicksY", "precise"}}`,
`{"key": keyCode, "modifiers": [...]}`, `{"dialog": button index | file name | null}` (queued
answer for the next dialog), `{"menu": [label, ...]}` (clicks the item at that label path of the
context menu that popped up; neither side shows menus natively), `{"appMenu": [label, ...]}`
(clicks, for the hub window, the one application menu item that has one of these labels, which
differ per platform), `{"prompt": page | null}`
(`editRange`, `editFov`, `renameTab`, `unitConversion`, `export`, `preferences`, `licenses`,
`sourceListHelp` or `betaWelcome`: later input and captures go to that window, whose content size
is its own; `null` waits until it has closed),
`{"wait": ms}`, `{"idle": true}` and
`{"capture": name, "probes": [selectors], "masks": [selectors]}`.

A capture waits until the page is idle (fonts loaded, no log or field loading, no playback) and
then until three consecutive window captures, DOM dumps and drawing-buffer read counts agree.
It writes `captures/<name>.png` (capturePage), `captures/<name>.json` (size, colour space,
probed boxes, every visible text node's line boxes with font and colour, form controls, masks
for focused text inputs, webviews and the scenario's mask selectors, the canvas and GL dump
list, and a `timing` object that differs between runs), `<name>.canvas-<i>.png` (`getImageData`
on each visible 2D canvas) and `<name>.gl-<i>.png`. Each GL entry names its render mode (`mode`:
`standard`, `low-power` or `cinematic`, read from the context's `powerPreference`, which
`Field3dRendererImpl` sets from the mode alone), and gate 4 picks its rule from it. On the `app`
page the capture also waits for the next `save-state` and writes its data as
`<name>.hub-state.json` (`JSON.stringify`, compared byte for byte) and records
`selection.getTimelineRange()` as `timelineRange` (compared within 1e-9 of the span: Chromium
animates a notched wheel tick in compositor-timed steps, so the zoom `ScrollSensor` compounds
differs in the last bits between runs). After the last action
the driver waits for upstream's periodic state saves and copies the top-level JSON files and
asset folders of userData into `files/`.

The port's side is `asrs --parity-scenario` (built with the `parity` feature; `cargo xtask pixel
port` runs it). It reads the same files and writes the same layout headless. Actions go through
the hub's model APIs (`main` and `open` as main messages, `selection` on the model's
`SelectionImpl`) and the window's input dispatch (`mouse`, `wheel`, `key`; a notched wheel event
becomes GPUI's line delta, so its `deltaY` must be 40 px a tick on macOS and Linux); a `dialog`
action answers the next dialog the port shows, and `run.json` lists those it showed, which `xtask
pixel compare` checks against the driver's together with the files under `files/dialog/`. `eval`
is refused because the port has no page script, and so are `userData` seeds other than
`beta-2027.json`. Before the window opens it seeds `prefs.json` and `beta-2027.json` as the driver does and runs main's own
startup (`asrs_services::lifecycle::MainState`) in the variant's `userData/`: the preferences
rewrite, `createAssetFolders()` and `loadAssets()` over upstream's `bundledAssets`
(`--bundled-assets`), and boots the hub with main's assets and preferences file. The hub's
`save-state` and `save-type-memory` go to main's `StateTracker` and type memory, with the window
where the driver puts its own: 16 px into the primary work area that `xtask pixel port` reads
from the upstream run (`--work-area`). What lands in `userData/` is copied to `files/` and listed
in `run.json` with the driver's filter, and `xtask pixel compare` requires every file the port
wrote to equal upstream's and every file upstream wrote to be written by the port. Its
page realm is `V8MathRandom::from_seed(1)` after the 77 draws upstream's hub bundle
makes while its imports evaluate, so restored and default controller UUIDs match. Idle means the
same busy reasons, read from the model. Probes and mask selectors resolve through
`parity/probes.json` to the parity ids of painted elements (`asrs_ui::parity`), text runs come
from every painted `Text`, canvas dumps from each presented canvas bitmap, and 3D dumps (with an
edge mask PNG and the mode) from views that record them. A capture is taken when two consecutive
frames give the same pixels and records. Windows the hub opens transparent on macOS clear to
alpha 0, as `capturePage` gives where the vibrancy shows.

Colour: with `--force-color-profile=srgb` the colour-patch capture is exact on the macOS dev
display (512 of 512). Without it capturePage returns the display's colour space (Display P3
here); `--colour display` keeps that, stores the display's ICC profile as `display.icc`
(NSScreen on macOS, the X root window's `_ICC_PROFILE` on Linux, `GetICMProfileW` on Windows) and
`cargo xtask app normalize` converts with lcms2. That route loses information: after
normalization 21 of 512 patches are 2 to 5 levels off, all saturated colours, because the 8-bit
P3 capture is too coarse to invert near the sRGB gamut edge. `cargo xtask app check` gates the
sRGB route and reports the display route.
