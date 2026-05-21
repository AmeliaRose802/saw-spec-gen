// Async version of add_one.
//
// The body is functionally identical to the synchronous demo
// (`x + 1`) but we use an `async fn`, so rustc lowers it into a
// coroutine state machine. After compilation the bitcode contains
// (at least) these new symbols:
//
//   • add_one                       — constructs the coroutine
//                                     (sret of the future state)
//   • <add_one coroutine resume>    — the actual state machine
//   • <ReadyU32 as Future>::poll    — Future impl for our trivial
//                                     immediately-ready helper
//
// `saw-spec-gen from-llvm-ir` walks all of these and emits
// adversarial override specs for the ones we don't want to verify
// directly. We then verify `ReadyU32::poll` against a Cryptol spec
// in `add_one_spec.cry`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

// A future that is `Ready` immediately. Hand-written so the demo
// does not depend on std futures combinators and compiles cleanly
// with `-C panic=abort`. The single `u32` field is the value to
// return from `poll`.
//
// `#[repr(transparent)]` makes `&ReadyU32` ABI-identical to `&u32`,
// which keeps the LLVM signature of `poll` predictable for SAW.
#[repr(transparent)]
pub struct ReadyU32(pub u32);

impl Future for ReadyU32 {
    type Output = u32;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u32> {
        Poll::Ready(self.0)
    }
}

// The async function under verification.
//
// Mathematically: `add_one(x).await == x + 1`. SAW does not run the
// executor — instead, we point `llvm_verify` at the coroutine resume
// function rustc emits for this body (`add_one::{closure#0}`), and
// prove its `Poll<u32>` return value equals `(0, x + 1)` for all x.
// SAW walks into `into_future` and `ReadyU32::poll` using their real
// bodies, so the proof rules out an arithmetic bug anywhere along
// that path.
pub async fn add_one(x: u32) -> u32 {
    let y = ReadyU32(x).await;
    y + 1
}

