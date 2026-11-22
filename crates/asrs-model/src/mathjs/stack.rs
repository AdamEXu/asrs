//! mathjs's parser, compile pass and evaluator recurse once per nesting level, and upstream's
//! `evaluate` fails with V8's `RangeError: Maximum call stack size exceeded` on input nested past
//! its stack. Parsing and evaluation here run on a thread of their own whose stack is larger
//! than any nesting upstream accepts needs, and the recursive steps call [`check`], which turns
//! running past [`BUDGET`] into the same `RangeError` instead of overflowing the stack.

use std::cell::Cell;

use asrs_js::string::JsString;

use super::MathError;

const STACK_SIZE: usize = 128 << 20;
const BUDGET: usize = 96 << 20;

thread_local! {
    static BASE: Cell<usize> = const { Cell::new(0) };
}

#[inline(never)]
fn here() -> usize {
    let marker = 0u8;
    std::ptr::from_ref(std::hint::black_box(&marker)) as usize
}

pub(super) fn run<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("mathjs".to_owned())
            .stack_size(STACK_SIZE)
            .spawn_scoped(scope, || {
                BASE.set(here());
                f()
            })
            .expect("spawn the mathjs thread");
        worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

pub(super) fn check() -> Result<(), MathError> {
    let base = BASE.get();
    if base != 0 && base.abs_diff(here()) > BUDGET {
        Err(MathError {
            name: "RangeError",
            message: JsString::from("Maximum call stack size exceeded"),
        })
    } else {
        Ok(())
    }
}
