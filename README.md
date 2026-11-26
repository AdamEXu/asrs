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
cargo xtask bundle      # release packages: app, sin/cos library, documentation, licenses
cargo xtask smoke       # check the packages: install, start, OCR, file types, library swap
cargo xtask version     # the version, its numeric forms, and whether a tag matches it
```

Building needs a checkout of upstream AdvantageScope at the commit pinned in
`xtask/src/oracle.rs` in `reference/AdvantageScope` (its icons and tables are read at build
time). On Linux, GPUI needs the X11, Wayland, xkbcommon, fontconfig, FreeType and Vulkan
development packages; `.github/workflows/ci.yml` lists them for Ubuntu. It also lists GTK 3 and
WebKitGTK 4.1, which only `asrs-webview` links: the process that shows the Documentation tab's
page. Without it (not built, no WebKitGTK, or a Wayland window) the tab links to the online
documentation instead.

## Packages

`cargo xtask bundle` builds the packages of the platform it runs on in `target/bundle/host`:

| Platform | Packages | Needs |
|---|---|---|
| macOS 12 and later | `.dmg` with `asrs.app` | Xcode's command line tools |
| Windows 10 and later | `.zip`, installer for the current user | [Inno Setup 6](https://jrsoftware.org) |
| Linux (the build machine's glibc and later) | `.tar.gz` with `install.sh`, AppImage | squashfs-tools |

The packages carry the workspace's version (see [Versions and releases](#versions-and-releases)).
They are unsigned; `asrs.app` is signed ad hoc with
the hardened runtime and `packaging/macos/entitlements.plist`, as a Developer ID signature would
be. asrs registers as an alternate program for `.wpilog`, `.wpilogxz`, `.rlog`, `.revlog`,
`.dslog` and `.dsevents` files, so they open in it only where the user picks it or nothing else
opens them. On macOS and Windows it is also offered for `.csv` files, in a way that does not make it
their default anywhere; Linux has no such way, so there it is not. The icon in `packaging/icons` is a placeholder.
On Linux with a Vulkan loader older than 1.4.345 (`libvulkan1` on Ubuntu 24.04 and 26.04), the
3D view keeps its device after its last tab closes and frees only its large resources, because
destroying it while GPUI recreates its swapchain can crash those loaders (#486). Updating the
loader to 1.4.345 or newer lifts that.
`--no-docs` leaves out the documentation, which may not be published yet (#310).

`asrs --self-test [<image> <left> <top> <width> <height>]` checks a package without a window, and
`asrs --version` prints its version.

## Versions and releases

asrs has versions of its own, unrelated to upstream's. `version` under `[workspace.package]` in
`Cargo.toml` is the only place one is written: a SemVer version, `<major>.<minor>.<patch>` or that
with `-alpha.<n>`, `-beta.<n>` or `-rc.<n>`. A release's tag is `v` and the version, for example
`v0.1.0-alpha.1`. About, `--version`, crash output and the packages take it from there.

The version of upstream asrs is a port of is `VERSION` in
`crates/asrs-model/src/build_constants.rs`. Layout files and the state file's name carry it and
they do not change with asrs's version; parity builds present it in place of asrs's. About and
`--version` show both: `asrs 0.1.0-alpha.1 (AdvantageScope 27.0.0-alpha-6)`.

Some fields hold numbers only. `cargo xtask version` prints what they get, with a build number made
from the pre-release: `alpha.<n>` is n, `beta.<n>` 1000 + n, `rc.<n>` 2000 + n (n from 1 to 999)
and a release 3000. It grows with the version, so each package counts as newer than the ones before:

| Field | Form | `0.1.0-alpha.1` | `0.1.0` |
|---|---|---|---|
| macOS `CFBundleShortVersionString` | major.minor.patch | `0.1.0` | `0.1.0` |
| macOS `CFBundleVersion` | (major x 1000 + minor).patch.build | `1.0.1` | `1.0.3000` |
| Windows file and product version | major.minor.patch.build | `0.1.0.1` | `0.1.0.3000` |

To prepare a release (nothing in the repository tags or publishes one):

1. Set the version in `Cargo.toml` and run `cargo update --workspace` for `Cargo.lock`. A
   version that was released is not reused, even for the same code built again.
2. Run `cargo xtask version --tag v<version>` and `cargo xtask check`, and merge the change.
3. Wait for CI on that commit of `main`: its `package` jobs build and check the packages.
4. Check that the packages and their file names have the version, and that they may be published
   (#310 for the documentation; the AppImage needs its runtime's source next to it).
5. Install over the previous release on macOS and Windows and open a saved layout.
6. Tag that commit, after running `cargo xtask version --tag v<version>` on it once more.

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
and runs `cargo fmt`, clippy with and without the parity features and the committed-vector tests.
The `package` jobs run `cargo xtask docs`, `bundle` and `smoke` on the same five targets and upload
the packages without the documentation (and without the AppImage, whose runtime contains LGPL code
that comes without its source). While the repository is private only the x64 Linux jobs run. CI sets `ASRS_CI=1`, under which these tests skip with a printed
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
build; see [its README](crates/asrs-libm-lgpl/README.md). Every package `cargo xtask bundle`
makes contains the app, the library, its source and every license text.
