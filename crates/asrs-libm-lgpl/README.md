# asrs-libm-lgpl

`Math.sin` and `Math.cos` as V8 14.2 (Electron 39) computes them: a Rust port of the IBM
Accurate Mathematical Library from the GNU C Library (`sysdeps/ieee754/dbl-64/s_sin.c`,
`branred.c` and `sincostab.c`), which V8 bundles under `third_party/glibc`.

This crate is licensed under the GNU Lesser General Public License, version 2.1 or (at your
option) any later version; the full text is in [`LICENSE`](LICENSE). It is the only
LGPL-licensed code in asrs. Everything else is under the BSD 3-Clause license of the main
project.

## How asrs uses it

The crate builds a shared library (`libasrs_libm_lgpl.dylib` on macOS,
`asrs_libm_lgpl.dll` on Windows, `libasrs_libm_lgpl.so` on Linux) that asrs loads at runtime
and never links statically. asrs looks for it, first match wins:

1. at the path in the `ASRS_LIBM_LGPL` environment variable, if set (and nowhere else);
2. in the directory of the running executable, where releases ship it;
3. in development builds only, the copy the `asrs-js` build script compiles from this crate.

asrs stops with an error naming the paths it tried when the library is missing.

## C ABI

A replacement must export these symbols with the C calling convention:

| Symbol | Signature | Meaning |
|---|---|---|
| `asrs_libm_lgpl_abi_version` | `uint32_t (void)` | must return `1` |
| `asrs_libm_lgpl_sin` | `double (double)` | `Math.sin` |
| `asrs_libm_lgpl_cos` | `double (double)` | `Math.cos` |

The library may be written in any language. asrs reproduces upstream AdvantageScope
bit for bit, so a replacement that rounds differently from V8 changes what asrs displays and
exports.

## Replacing or relinking it

From a source checkout of asrs:

```sh
# edit crates/asrs-libm-lgpl/src, then:
cargo xtask libm --release --out <directory containing the advantagescope executable>
```

From a release, the bundle contains this crate's source in `asrs-libm-lgpl/` with a
standalone `Cargo.toml`:

```sh
cd asrs-libm-lgpl
cargo build --release
cp target/release/libasrs_libm_lgpl.dylib <directory containing advantagescope>   # .dll / .so elsewhere
```

Instead of copying, `ASRS_LIBM_LGPL=/path/to/library advantagescope` runs asrs with the
library at that path. `cargo xtask libm plant` builds a copy with one table entry changed
and checks that asrs's math tests catch it, which shows the tests run whichever library is
loaded.
