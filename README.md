# asrs

asrs is a port of [AdvantageScope](https://github.com/Mechanical-Advantage/AdvantageScope),
the robot telemetry and log viewer, from Electron and TypeScript to Rust and
[GPUI](https://www.gpui.rs). It aims to match upstream's behaviour and rendering exactly,
checked against upstream itself by the parity harness under `oracle/` and `parity/`.

asrs is an unofficial port and an independent project. It is not affiliated with, endorsed
by or supported by Littleton Robotics or FRC Team 6328 (Mechanical Advantage), and it is not
AdvantageScope: the app is named `asrs`. Please report problems with asrs here, not to the
AdvantageScope project. The parity code keeps the identities upstream's protocols and file
formats use (the NetworkTables client name and the WPILOG exporter metadata both say
`AdvantageScope`), since robot code and other tools read them.

## Building

```sh
cargo xtask assets      # copy upstream assets from reference/AdvantageScope
cargo xtask docs        # build upstream's documentation for the Documentation tab (needs Node)
cargo run -p asrs-app   # the asrs binary
cargo xtask check       # layout rules, replay-harness lint, formatting, clippy and tests
cargo xtask licenses    # third-party license bundle for release builds
cargo xtask libm        # build the LGPL sin/cos library asrs loads at runtime
cargo xtask bundle      # release directory: app, sin/cos library, documentation, licenses
```

Building needs a checkout of upstream AdvantageScope at the commit pinned in
`xtask/src/oracle.rs` in `reference/AdvantageScope` (its icons and tables are read at build
time). On Linux, GPUI needs the X11, Wayland, xkbcommon, fontconfig, FreeType and Vulkan
development packages; `.github/workflows/ci.yml` lists them for Ubuntu. It also lists GTK 3 and
WebKitGTK 4.1, which only `asrs-webview` links: the process that shows the Documentation tab's
page. Without it (not built, no WebKitGTK, or a Wayland window) the tab links to the online
documentation instead.

## Public logs and local vectors

`parity/corpus/public.toml` lists the public logs the parity suites use, with each log's
license and whether it may be redistributed. asrs only commits vectors derived from logs whose
license allows it. Vectors derived from the others (the RAR1741 logs, which state no license)
are generated on your machine, next to the downloaded logs in `$ASRS_CACHE/vectors`:

```sh
cargo xtask corpus fetch        # download the public logs into $ASRS_CACHE/public-logs
cargo xtask oracle gen --local  # generate the local vectors (needs `cargo xtask oracle setup`)
```

Without them, the tests that use them (`wpilog_vectors::public` and `::search_key_sets`,
`search_vectors::search_fields_public_vectors`,
`merge_calculator_vectors::merge_calculator_public`) skip with a notice.

## Continuous integration

CI builds and tests natively on Windows (x64, arm64), macOS (arm64) and Linux (x64, arm64),
runs `cargo fmt`, clippy with and without the parity features, the committed-vector tests,
`cargo xtask docs` and `cargo xtask bundle`, and uploads each bundle without the documentation. While the repository is private only the
ubuntu-latest job runs. CI sets `ASRS_CI=1`, under which these tests skip with a printed
`note: skipping` line:

- the local vector tests above (downloaded public logs)
- `asrs-ui`: `headless_capture`, `icon_capture`, `pixel_snap`, `canvas_view`, `text_box` and
  `text_scroll` (GPU) and `platform_mac` (display)
- `asrs-field3d`: `render_gpu` (GPU)
- `asrs-app`: `documentation_window` (display)

Tests that need the Electron oracle (the `*_sweep` tests) are `#[ignore]`d everywhere, and no
default test reads the generated corpora under `target/parity`. On x86_64 the per-platform
vector tests (`math_vectors::math_remainder`, `util_vectors::util_platform_nan`,
`statistics_vectors::simple_statistics`, and `statistics_dom`, whose statistics use the C
library's `pow` as V8 does) skip until x86_64 vectors exist, and NaN sign and
payload bits are compared loosely, since x86_64 makes different NaNs from the aarch64 hosts
the vectors come from; every other bit is still compared exactly.

## License

asrs is distributed under the BSD 3-Clause license in [`LICENSE`](LICENSE), with copyright
held by Adam Xu and, as a derived work of AdvantageScope, by Littleton Robotics.

It includes code ported from other projects under their own licenses (MIT, ISC,
Apache-2.0, BSD and the Unicode license), listed in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) with full texts in
[`licenses/`](licenses/). Licenses of the Rust crates compiled into the app are generated
by `cargo xtask licenses`.

One component is not BSD-licensed: [`crates/asrs-libm-lgpl`](crates/asrs-libm-lgpl), the
port of glibc's `sin` and `cos` that V8 uses for `Math.sin` and `Math.cos`, is licensed under
the LGPL-2.1-or-later. It builds a separate shared library (`libasrs_libm_lgpl.dylib`,
`asrs_libm_lgpl.dll` or `libasrs_libm_lgpl.so`) that asrs loads at runtime from next to its
executable, or from the path in `ASRS_LIBM_LGPL`. You can replace that library with your own
build; see [its README](crates/asrs-libm-lgpl/README.md). `cargo xtask bundle` assembles a
release directory with the app, the library, its source and every license text.
