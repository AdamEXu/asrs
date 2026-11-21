# asrs

asrs is a port of [AdvantageScope](https://github.com/Mechanical-Advantage/AdvantageScope),
the robot telemetry and log viewer, from Electron and TypeScript to Rust and
[GPUI](https://www.gpui.rs). It aims to match upstream's behaviour and rendering exactly,
checked against upstream itself by the parity harness under `oracle/` and `parity/`.

asrs is an independent project. It is not affiliated with, endorsed by or supported by
Littleton Robotics or FRC Team 6328 (Mechanical Advantage). Please report problems with
asrs here, not to the AdvantageScope project.

## Building

```sh
cargo xtask assets      # copy upstream assets from reference/AdvantageScope
cargo run -p asrs-app
cargo xtask check       # layout rules, formatting, clippy and tests
cargo xtask licenses    # third-party license bundle for release builds
cargo xtask libm        # build the LGPL sin/cos library asrs loads at runtime
cargo xtask bundle      # release directory: app, sin/cos library, licenses
```

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
