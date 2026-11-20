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

Downloads go to `$ASRS_CACHE` (default: `<real target dir>/../cache` when `target/` is a
symlink, otherwise `target/cache`): `npm/`, `electron/`, `emsdk/`.

## Commands

| Command | What it does |
|---|---|
| `cargo xtask oracle fn` | JSONL requests on stdin, CVE-1 responses on stdout (protocol below) |
| `cargo xtask oracle fn --list` | upstream modules, npm packages and helpers callable by `fn` |
| `cargo xtask oracle run <entry> [args]` | run `target/oracle/dist/<entry>.mjs` under the oracle runtime |
| `cargo xtask oracle build` | rebuild the harness bundles only, after editing `oracle/src` |
| `cargo xtask oracle gen [<id>...] [--check]` | regenerate committed vectors (below); `--check` fails if a rerun differs |
| `cargo xtask oracle selftest` | CVE-1 encoder/decoder self test |
| `cargo xtask oracle smoke` | `smoke/fn-requests.jsonl` against `smoke/fn-expected.jsonl` |
| `cargo xtask oracle env` | electron binary, upstream, dist and cache paths as JSON |

The runtime is not pinned to a locale or time zone. The default locale on the dev machine is
`en-US`; set `LC_ALL`/`TZ` on the command when a vector depends on them.

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
`parity/vectors/MANIFEST.json` records the upstream commit, Electron version, case count,
content hash and generator hash (oracle sources plus spec) of each one.

- `<id>.jsonl`: `fn` requests. Each vector line is the request with the oracle's `ok` or
  `throw` key appended. An `error` response fails generation.
- `<id>.entry.json`: `{"entry": "<name>", "args": [...]}`. The entry's stdout lines are the
  vector, so an entry can build values JSON cannot express.

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

## Build constants

`build.mjs` applies the same `__distribution__`, `__version__` and `__copyright__` replacements
as upstream's `rolldown.config.mjs` (FRC6328 distribution), so `buildConstants.ts` matches the
shipped app. `__build_date__` is pinned to `1970-01-01T00:00:00.000Z` for reproducible output.

## Adding an entry point

Add `src/<name>.ts` (starting with `import "./shims"`), list it in `entries` in `build.mjs`,
and run it with `cargo xtask oracle run <name>`. Harness code can import upstream sources as
`@upstream/<path under src>`.
