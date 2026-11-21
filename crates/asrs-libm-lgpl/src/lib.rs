// asrs-libm-lgpl: V8's Math.sin and Math.cos, ported from the IBM Accurate Mathematical Library
// in the GNU C Library, as a replaceable shared library for asrs.
// Copyright (C) 2001-2022 Free Software Foundation, Inc.
// Copyright (c) 2026 Adam Xu
// Licensed under the GNU Lesser General Public License, version 2.1 or (at your option) any
// later version. See LICENSE in this directory.
//
// asrs loads this library at runtime through the C ABI below (see README.md). A replacement
// must export the same three symbols and return ABI_VERSION from asrs_libm_lgpl_abi_version.

#![allow(clippy::disallowed_methods)]

mod glibc;
mod sincostab;

const ABI_VERSION: u32 = 1;

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn asrs_libm_lgpl_abi_version() -> u32 {
    ABI_VERSION
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn asrs_libm_lgpl_sin(x: f64) -> f64 {
    glibc::sin(x)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn asrs_libm_lgpl_cos(x: f64) -> f64 {
    glibc::cos(x)
}
