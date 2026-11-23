# Third-party notices

asrs contains code ported or derived from the projects below, plus data and
assets copied from them. Full license texts are in [`licenses/`](licenses/).
Rust crates compiled into the app are listed separately: run
`cargo xtask licenses` to generate `target/licenses/THIRD_PARTY_LICENSES.txt`,
which release builds bundle together with this file, `LICENSE`, `licenses/` and the
source of the LGPL `asrs-libm-lgpl` library (see below).

Files that are line-by-line ports carry a short header naming their origin.

## AdvantageScope

- License: BSD-3-Clause style ([`licenses/AdvantageScope.txt`](licenses/AdvantageScope.txt))
- Copyright (c) 2021-2026 Littleton Robotics
- Source: https://github.com/Mechanical-Advantage/AdvantageScope (pinned in `assets/UPSTREAM`)

asrs as a whole is a derived work of AdvantageScope: every crate ports its
TypeScript, CSS and C sources (including the WPILOG indexer,
`src/hub/dataSources/wpilog/indexer/wpilogIndexer.c`). Also taken from upstream:

- `www/symbols` icons, embedded into `asrs-ui` at build time, and `www/textures`,
  `icons`, `bundledAssets` and `licenses`, copied into `assets/` by `cargo xtask assets`.
- `crates/asrs-model/src/source_list/configs/*.json`: upstream's tab controller
  source list configs.
- `crates/asrs-log/src/units/tables.rs` and `crates/asrs-formats/src/spark/tables.rs`,
  generated from upstream TypeScript by `cargo xtask tables`.
- `parity/specs/assets/release`: asset `config.json` files from AdvantageScopeAssets
  releases (test data, with its own [`LICENSE`](parity/specs/assets/release/LICENSE)).

asrs is not affiliated with or endorsed by Littleton Robotics or FRC Team 6328.

## Google Material Symbols

- License: Apache-2.0 ([`licenses/Apache-2.0.txt`](licenses/Apache-2.0.txt))
- Copyright Google LLC
- Used in: upstream's `www/symbols` icons, embedded into the app by `crates/asrs-ui/build.rs`

## three.js

- License: MIT ([`licenses/three.js.txt`](licenses/three.js.txt))
- Copyright (c) 2010-2024 three.js authors
- Version: r168 (npm `three` 0.168.0)
- Used in: `crates/asrs-field3d/src/three/`, `crates/asrs-field3d/src/render/mod.rs`,
  `render/scene.rs`, and `render/phong.wgsl` (`MeshPhongMaterial` shader chunks)

## camera-controls

- License: MIT ([`licenses/camera-controls.txt`](licenses/camera-controls.txt))
- Copyright (c) 2017 yomotsu
- Version: 2.9.0
- Used in: `crates/asrs-field3d/src/camera_controls.rs`

## Fuse.js

- License: Apache-2.0 ([`licenses/Fuse.js.txt`](licenses/Fuse.js.txt))
- Copyright 2017 Kirollos Risk
- Version: 7.0.0
- Used in: `crates/asrs-log/src/log/fuse.rs`, translated to Rust and modified (only the
  string-list Bitap search configuration upstream uses)

## ansi_up

- License: MIT ([`licenses/ansi_up.txt`](licenses/ansi_up.txt))
- Copyright (c) 2011 github.com/drudru
- Version: 6.0.6
- Used in: `crates/asrs-tabs/src/ansi.rs`

## heatmap.js

- License: MIT ([`licenses/heatmap.js.txt`](licenses/heatmap.js.txt))
- Copyright (c) 2015 Patrick Wied
- Version: 2.0.5, the fork at `github.com/jwbonner/heatmap.js#24b4455`
- Used in: `crates/asrs-canvas/src/heatmap.rs`

## Chart.js

- License: MIT ([`licenses/chart.js.txt`](licenses/chart.js.txt))
- Copyright (c) 2014-2022 Chart.js Contributors
- Version: 4.4.0
- Used in: `crates/asrs-tabs/src/histogram/` (`chart.rs`, `scale.rs`, `ticks.rs`, `tooltip.rs`,
  `animation.rs`), translated to Rust for the one bar chart configuration upstream uses

## @kurkle/color

- License: MIT ([`licenses/kurkle-color.txt`](licenses/kurkle-color.txt))
- Copyright (c) 2018-2021 Jukka Kurkela
- Version: 0.3.2 (as bundled with Chart.js 4.4.0)
- Used in: `crates/asrs-tabs/src/histogram/color.rs`

## @msgpack/msgpack

- License: ISC ([`licenses/msgpack-javascript.txt`](licenses/msgpack-javascript.txt))
- Copyright 2019 The MessagePack Community
- Version: 3.0.0-beta2
- Used in: `crates/asrs-log/src/msgpack.rs`

## protobufjs

- License: BSD-3-Clause ([`licenses/protobufjs.txt`](licenses/protobufjs.txt))
- Copyright (c) 2016 Daniel Wirtz
- Version: 8.3.0
- Used in: `crates/asrs-log/src/log/proto_decoder.rs` and `proto_decoder/`

## bignumber.js

- License: MIT ([`licenses/bignumber.js.txt`](licenses/bignumber.js.txt))
- Copyright (c) 2023 Michael Mclaughlin
- Version: 9.1.2
- Used in: `crates/asrs-formats/src/spark/bignum.rs`

## color-convert

- License: MIT ([`licenses/color-convert.txt`](licenses/color-convert.txt))
- Copyright (c) 2011-2016 Heather Arthur
- Version: 2.0.1
- Used in: `crates/asrs-js/src/color.rs`

## REV Robotics CAN specs

- License: MIT ([`licenses/REV Robotics.txt`](licenses/REV%20Robotics.txt))
- Copyright 2024 REV Robotics
- Source: upstream's `src/hub/dataSources/schema/spark/` (`can-spec-util.ts`, `bitstring.ts`,
  and `spark-frames-{2025,2026}.ts`, generated from https://github.com/REVrobotics/can-specs)
- Used in: `crates/asrs-formats/src/spark/` (`can_spec.rs`, `bitstring.rs`, generated `tables.rs`)

The generated `spark-frames` files carry no license header of their own; the MIT notice
above is the one REV attached to the utilities shipped alongside them.

## @rev-robotics/revlog-converter

- License: BSD-3-Clause ([`licenses/revlog-converter.txt`](licenses/revlog-converter.txt))
- Copyright (c) 2025, REV Robotics
- Version: 2.0.0 (npm), source https://github.com/REVrobotics/node-revlog-converter
- Used in: `crates/asrs-formats/src/revlog.rs` and `revlog/dbc.rs` (ports of
  `revlogParser` and `revDBC`), and `crates/asrs-formats/src/revlog/*.public.dbc`, the
  package's DBC files copied unmodified

## tesseract.js

- License: Apache-2.0 ([`licenses/Apache-2.0.txt`](licenses/Apache-2.0.txt))
- Copyright (c) the tesseract.js contributors (Kevin Kwok, Guillermo Webster, Jerome Wu, Balearica)
- Version: 6.0.1
- Used in: `crates/asrs-ocr/src/engine.rs` (the recognition path of `src/worker-script` and the
  Emscripten glue of the core, translated to Rust) and `timer.rs` (the scheduler's queue)

## tesseract.js-core

- License: Apache-2.0 ([`licenses/Apache-2.0.txt`](licenses/Apache-2.0.txt)); the libraries compiled
  into it are listed with their licenses in
  [`licenses/tesseract.js-core.txt`](licenses/tesseract.js-core.txt)
- Version: 6.0.0 (npm), source https://github.com/naptha/tesseract.js-core
- Used in: `crates/asrs-ocr/data/tesseract-core-simd.wasm`, copied unmodified
  (sha256 `ca11ee92a02023e4f5083eefd0966fb83b297416da54e997b9f6ce445f719c9b`)

It contains Tesseract OCR 5.1.0-284-g9c08 (Apache-2.0, Copyright Google Inc. and the Tesseract
contributors; built from https://github.com/Balearica/tesseract/tree/5a21eeb), Leptonica, libjpeg,
libpng, libtiff, libwebp, giflib, zlib, OpenLibm and the Emscripten 3.1.38 runtime. This software is
based in part on the work of the Independent JPEG Group.

## Tesseract English model

- License: Apache-2.0 ([`licenses/Apache-2.0.txt`](licenses/Apache-2.0.txt))
- Copyright Google Inc. (tessdata_best `eng`, converted to integers by tesseract.js)
- Version: npm `@tesseract.js-data/eng` 1.0.0, `4.0.0_best_int/eng.traineddata.gz`
- Used in: `crates/asrs-ocr/data/eng.traineddata.gz`, copied unmodified
  (sha256 `45b4cb346724ac1774f1c36f42f182b887bcdb28ebe63e6fff90ac41f3fcff91`)

The npm package's own metadata says MIT; its source repository (https://github.com/naptha/tessdata)
and the model it converts are Apache-2.0.

## V8

- License: BSD-3-Clause ([`licenses/V8.txt`](licenses/V8.txt))
- Copyright 2006-2011 the V8 project authors
- Version: 14.2 (as shipped in Electron 39)
- Used in: `crates/asrs-js/src/sort.rs` (TimSort from `array-sort.tq`),
  `crates/asrs-js/src/number.rs` (number formatting and parsing),
  `crates/asrs-js/src/json/parse.rs` (`JsonParser`), `crates/asrs-js/src/math/fdlibm.rs`
  (`src/base/ieee754.cc`). Other `asrs-js` modules reproduce V8's observable behaviour
  (Irregexp matching, BigInt conversion, string encoding, error messages) without porting code.

### fdlibm

- License: fdlibm notice ([`licenses/fdlibm.txt`](licenses/fdlibm.txt)), preserved in
  `crates/asrs-js/src/math/fdlibm.rs`
- Copyright (C) 1993-2004 Sun Microsystems, Inc.

## GNU C Library (IBM Accurate Mathematical Library)

- License: LGPL-2.1-or-later ([`crates/asrs-libm-lgpl/LICENSE`](crates/asrs-libm-lgpl/LICENSE),
  also [`licenses/LGPL-2.1.txt`](licenses/LGPL-2.1.txt))
- Copyright (C) 2001-2022 Free Software Foundation, Inc.
- Source: `sysdeps/ieee754/dbl-64/s_sin.c`, `branred.c` and `sincostab.c`, as V8 bundles them
  under `third_party/glibc` for `Math.sin` and `Math.cos`
- Used in: `crates/asrs-libm-lgpl/src/glibc.rs` and `crates/asrs-libm-lgpl/src/sincostab.rs`

The `asrs-libm-lgpl` crate is licensed under the LGPL, not under the project's BSD license.
It is built as its own shared library (`libasrs_libm_lgpl.dylib`, `asrs_libm_lgpl.dll`,
`libasrs_libm_lgpl.so`), shipped next to the asrs executable and loaded at runtime through a
three-function C ABI by `crates/asrs-js/src/math/libm.rs`; no LGPL code is linked into the
asrs executable. Users may replace the library with a modified build, or point the
`ASRS_LIBM_LGPL` environment variable at one, as described in
[`crates/asrs-libm-lgpl/README.md`](crates/asrs-libm-lgpl/README.md). Release bundles
(`cargo xtask bundle`, `cargo xtask licenses`) include the library's complete source with a
standalone `Cargo.toml` in `asrs-libm-lgpl/`.

## Skia

- License: BSD-3-Clause ([`licenses/Skia.txt`](licenses/Skia.txt))
- Copyright (c) 2011 Google Inc.
- Version: Chromium m142
- Used in: `crates/asrs-canvas/src/` (analytic AA in `aaa.rs`, `scan.rs`, `blit.rs`;
  `stroke.rs`; `dash.rs`; `clip.rs`; `raster_clip.rs`; `geometry.rs`; `hairline.rs`;
  `skpath.rs`; `pixels.rs`; Graphite's MSAA tessellation in `msaa.rs`; mip levels in `mip.rs`; gradients, paths and
  the raster pipeline in `gradient.rs`, `path.rs`, `raster.rs`; text layout matrices in
  `text/layout.rs`; skcms, `SkColorSpace` and the colour-conversion pipeline stages in
  `color_profile.rs`)

## Chromium and Blink

- License: BSD-3-Clause ([`licenses/Chromium.txt`](licenses/Chromium.txt)); Blink's
  combined notices, including the Apple and WebKit sources Blink inherits, are in
  [`licenses/Blink.txt`](licenses/Blink.txt)
- Copyright The Chromium Authors
- Version: m142 (as shipped in Electron 39)
- Used in: `crates/asrs-canvas/src/` (`color.rs` CSS colour parsing, `path.rs` and `raster.rs`
  `CanvasPath`, `gradient.rs` stop handling, `text/dom.rs`, `text/inline.rs`,
  `text/layout.rs`, `text/metrics.rs` text layout and `TextMetrics`), and behaviour reproduced in
  `crates/asrs-ui` (SVG painting, filters, theme) and `crates/asrs-live/src/timers.rs`
  (`DOMTimer` scheduling)

## ICU, CLDR and Unicode data

- License: Unicode License v3 ([`licenses/ICU.txt`](licenses/ICU.txt))
- Copyright Unicode, Inc. and others
- Versions: ICU 74.2, CLDR 44.1, Unicode 15.1
- Used in: `crates/asrs-js/src/string/case_table.rs` and
  `crates/asrs-js/src/collation/tables.rs`, generated from ICU's output inside Electron;
  the ICU4X 1.4 crates and their CLDR data are listed in the crate license listing

## Time zone database

- License: public domain
- Bundled through the `jiff-tzdb` crate (IANA tzdata), listed in the crate license listing

## gpui-pre (vendored and modified)

- License: Apache-2.0 ([`vendor/gpui-pre/LICENSE-APACHE`](vendor/gpui-pre/LICENSE-APACHE))
- Copyright Zed Industries, Inc. and contributors
- Version: 0.3.12, from https://github.com/zed-industries/zed
- Used in: `vendor/gpui-pre/`, modified by asrs. The changes are recorded in
  [`patches/gpui-pre-0.3.12.patch`](patches/gpui-pre-0.3.12.patch), which modifies
  `src/app/headless_app_context.rs`, `src/taffy.rs`, `src/util.rs` and `src/window.rs`. Each
  of those files starts with a line stating that asrs modified it.

gpui-component, gpui-kit and the other gpui crates are unmodified crate dependencies and
appear in the crate license listing.

## Notices carried over from upstream

Upstream AdvantageScope bundles these notices for code and services its features use.
asrs copies them into `assets/licenses` and keeps them here for the same features
(the FTC Dashboard source and The Blue Alliance integration).

- ACME Robotics, MIT, Copyright (c) 2018-2022 ACME Robotics
  ([`licenses/ACME Robotics.txt`](licenses/ACME%20Robotics.txt))
- The Blue Alliance, MIT, Copyright (c) 2020 The Blue Alliance
  ([`licenses/The Blue Alliance.txt`](licenses/The%20Blue%20Alliance.txt))

## simple-statistics

- License: ISC ([`licenses/simple-statistics.txt`](licenses/simple-statistics.txt))
- Copyright (c) 2014, Tom MacWright
- Version: 7.8.7
- Used in: `crates/asrs-tabs/src/simple_statistics.rs`, the subset of functions the
  Statistics controller calls, translated to Rust

## @mcap/core and @foxglove/crc

- License: MIT ([`licenses/mcap.txt`](licenses/mcap.txt); both packages carry the same text)
- Copyright (c) Foxglove Technologies Inc
- Version: @mcap/core 1.3.0, @foxglove/crc 0.0.3
- Used in: `crates/asrs-formats/src/mcap.rs` (`McapWriter`, `McapRecordBuilder`,
  `ChunkBuilder`, `BufferBuilder` and the CRC-32, translated to Rust)

## ssh2

- License: MIT ([`licenses/ssh2.txt`](licenses/ssh2.txt))
- Copyright Brian White
- Version: 1.14.0
- Used in: `crates/asrs-services/src/download/sftp.rs` (`lib/protocol/SFTP.js`: the client session,
  `readdir` and `fastGet`), `handshake.rs` (the ident, packet and key exchange parsing of
  `lib/protocol`) and `client.rs` (the event order of `lib/client.js`), translated to Rust. The SSH
  transport is the `russh` crate (Apache-2.0)

## lzma-native

- License: MIT ([`licenses/lzma-native.txt`](licenses/lzma-native.txt))
- Copyright (c) 2014-2016 Anna Henningsen
- Version: 8.0.6
- Used in: `crates/asrs-formats/src/wpilogxz.rs`, which reproduces how `JSLzmaStream` and
  `LZMAStream::doLZMACode` feed liblzma (input splitting, output buffering, decoder restarts)

## XZ Utils (liblzma)

- License: public domain (liblzma; see `COPYING` in the XZ Utils source)
- Copyright: none claimed; written by Lasse Collin, Igor Pavlov and others
- Version: 5.2.5, built from the source bundled in the `lzma-sys` crate (`xz2`'s `static`
  feature) and linked into the app
- Used in: `.wpilogxz` decompression (`crates/asrs-formats/src/wpilogxz.rs`)

## math.js

- License: Apache-2.0 ([`licenses/mathjs.txt`](licenses/mathjs.txt), which starts with its NOTICE)
- Copyright (C) 2013-2022 Jos de Jong
- Version: 11.3.0
- Used in: `crates/asrs-model/src/mathjs/`, translated to Rust and modified (the expression
  parser and the evaluation of the number subset that upstream's unit conversion prompt uses),
  and `crates/asrs-model/src/unit_conversion_dialog.rs`

## typed-function

- License: MIT ([`licenses/typed-function.txt`](licenses/typed-function.txt))
- Copyright (c) 2014-2018 Jos de Jong
- Version: 4.1.0
- Used in: `crates/asrs-model/src/mathjs/` (signature dispatch and argument error messages of
  math.js functions)

## npm packages behind the asset downloader

- License: MIT ([`licenses/asset-download-npm.txt`](licenses/asset-download-npm.txt), one
  copyright notice per package)
- Packages and versions (from upstream's lockfile): download 8.0.0, got 8.3.2,
  url-parse-lax 3.0.0, prepend-http 2.0.0, decompress 4.2.1, decompress-unzip 4.0.1,
  yauzl 2.10.0, archive-type 4.0.0 (file-type 4.4.0), file-type 11.1.0, ext-name 5.0.0,
  ext-list 2.2.2, mime-db 1.52.0, filenamify 3.0.0, filename-reserved-regex 2.0.0,
  content-disposition 0.5.4
- Used in: `crates/asrs-services/src/{asset_downloader,download_deps,file_type,zip}.rs`,
  translated to Rust; `crates/asrs-services/src/ext_list.rs` is generated from ext-list over
  mime-db, and `zip.rs` carries yauzl's CP437 table
