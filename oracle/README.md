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
`<os>-<arch>` it was generated on. The header is pretty-printed and each vector is one
compact line, sorted by id, so branches that add or regenerate different vectors touch
different lines.

A generator hash covers only what that vector's generator uses: the harness files its entry
point bundles (esbuild's metafile, recorded by `build.mjs` in `target/oracle/dist/inputs.json`
as paths under `oracle/`), the bundles it starts at runtime (`runtimeLoads` in `build.mjs`:
`protoRenderer` spawns the renderer check, `canvasMain` loads the canvas page, `dom` evaluates
the hub page bundle), `build.mjs`
and `package-lock.json`, then the spec files and the zone. `fn` specs use the `fn` entry.
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
  spec; they join the generator hash.
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
it regenerate in memory and compare content with the differ.

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

## Public corpus

`parity/corpus/public.toml` lists real logs by pinned URL, sha256 and size. `cargo xtask corpus fetch` downloads
them into `$ASRS_CACHE/public-logs/`, rejects any file whose size or hash differs, and keeps verified files;
`--offline` only verifies. The logs are never committed.

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
| `client.source` | `nt4` (needs `mode`: `default`, `akit`, `systemcore`, `ds`), `nt4-publisher` (`mode`: `sim` or `robot`), `rlog`, `phoenix`, `ftc` |
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

Server ops shared by every mock: `["up"]`; `["down", "refuse" \| "hang"]` (new connections and
requests are refused or never answered, open ones stay); `["mute", bool]` (pushes on open
connections are dropped; handshakes and HTTP replies still go out); `["drop"]` (the server closes every open
connection, websockets first, each in opening order). Per protocol (each server class documents
its own behaviour in detail):

| Protocol | Config | Steps |
|---|---|---|
| `nt4` (`src/live/nt4Server.ts`) | `version` (`4.1` or `4.0`), `ports` ([5810]), `offsetUs` (server time is `(epochMs + t) * 1000 + offsetUs`), `topics` ([{name, type, properties?, value?}]) | `announce`, `unannounce`, `properties`, `set`, `batch`, `setraw`, `text`, `binary` |
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
full microtask drain. `requestIdleCallback`
fires at its `timeout`, `requestAnimationFrame` 16 ms later, both as ordinary timers. IPC
between the renderer and the emulated main process is a delivery with no delay.

Client traffic reaches the server `latencyMs` after it is sent and server traffic reaches the
client `latencyMs` later. Connection and request ids count up from 1 across websockets, HTTP
requests and TCP sockets. Websockets behave like the browser's: `send` while connecting
throws, a refused handshake fires `error` then `close`, a server drop fires `close` (1006),
`close()` fires `close` a round trip later, and messages arriving when not open are dropped.
`fetch` is GET only; `AbortSignal.timeout` is a timer, and an abort reaches the server as
`http-abort`. TCP sockets follow the Node behaviour main.ts relies on: writes while connecting
go out on connect, `setTimeout` is an idle timer refreshed by connecting, reads and writes, a
refusal emits `error` then `close`; `error` is emitted at once and `close` as a later
delivery, as Node emits it from the handle's close callback.

### Trace

Each line is `{"t":<ms>,"ev":<event>,"id":<connection>,"v":<CVE-1>}`, with `id` and `v` left out
where an event has none, in the order things happen.

| From | Events (`v`) |
|---|---|
| client | `ws-open` ([url, [protocols]]), `ws-send` (string or bytes), `ws-close`, `http-get` (url), `http-abort`, `tcp-open` ([host, port]), `tcp-send` (bytes), `tcp-close` |
| server | `ws-accept` (protocol, `""` for none), `ws-reject`, `ws-push` (string or bytes), `ws-drop`, `http-reply` ([status, body]), `http-refuse`, `tcp-accept`, `tcp-refuse`, `tcp-push` (bytes), `tcp-drop` |
| app | `status` (the status callback's value), `output` ([log index, the time supplier's value]), `main` ([name, data] for main messages other than RLOG's), `dump` ([log index, canonical dump]), `throw` (a client op threw, as a string) |

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
compares its own trace with the vector.

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
cargo xtask oracle run dom <spec or fixture file>     # vector lines on stdout
cargo xtask oracle gen dom/tabs/line-graph            # regenerate one committed vector
```

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
| `{"selection": [[method, ...args], ...]}` | calls `window.selection[method](...args)` with CVE-1 arguments (for example `["setHoveredTime", 1.5]`) |
| `{"liveTime": t}` | calls `window.selection.setLiveConnected(() => t)`; `null` calls `setLiveDisconnected` |
| `{"tuner": [keys...]}` | installs a stub `window.tuner` tunable for the listed keys and writes `{"tuner": ["publish", key, value]}` or `["unpublish", key]` records; `null` removes it |
| `{"capture": [...], "label": ...}` | writes a `{"capture": label, "time": ms}` line, then each capture below |

Captures (a string or `{"kind": ..., ...}`):

| Capture | Lines written |
|---|---|
| `state` | `{"state": {sidebar, tabs}}`, the hub's `saveState()` |
| `commands` | `{"command": {index, type, title, uuid}, "value": <command>}` for every tab whose renderer was called since the last `commands` capture, with the command it was last given (snapshotted when `render` was called) |
| `canvas` | `{"canvas": "<class>#<n>", "frame", "width", "height", "calls": [...]}` for every 2D canvas drawn since the last `canvas` capture; `selector` limits it to matching canvases |
| `dom` | `{"dom": selector, "nodes": [...]}`: for the first match (`"all": true` for every match), each element in document order as `[depth, tag, class, hidden, text, extra]`, where `text` joins the element's own non-blank text nodes and `extra` holds the `attributes` asked for (default `title`) plus `value` of form controls and `checked` of checkboxes and radios |
| `selection` | `{"selection": {mode, hoveredTime, selectedTime, renderTime, currentLiveTime, grabZoomRange, timelineRange, timelineIsMaxZoom}}` |
| `eval` | `{"eval": expr, "value": <result>}` or `"throw"` |
| `measure` | `{"measure": {font, align, baseline, text}, "metrics": {...}}` per string of `texts`, from the recording canvas (`null` when the font is invalid) |

Also written as they happen: `{"main": name, "data": ...}` for every hub-to-main message not
ignored, `{"drag": <payload>}` for every `drag-update` event the hub dispatches during a field
drag, `{"tuner": ...}` records from a `{"tuner"}` stub step, and
`{"exception": where, "error": "<name>: <message>"}` when upstream code throws in
a timer, frame, event listener (reported by jsdom, `where` is `event`), message handler or
`eval`, or a promise rejection goes unhandled (the run goes on, as a page would). Every value
is CVE-1 inside a plain JSON object, so Rust reads it with `asrs_parity::record_vectors!` and
`Record::value`.

Baseline vectors: `dom/tabs/<tab>` (one per tab type, each with the robot log, the minimal
assets and a few fields: commands, state, canvas traces, selection and DOM text of the
renderer and controls at a selected time and again while hovering), `dom/hub/selection-sidebar`
(timeline hover and click, playback, step, zoom, sidebar expand, collapse and search) and
`dom/canvas/fixed-advance`.

Not modelled: CSS (so no computed styles or real text layout), scrolling (`scrollTop` stays 0),
workers and historical or live sources (logs are built with `log` steps), drag sessions other
than through dispatched events, the documentation iframe's content, and 3D rendering.
