//! Allocation-failure injection. yaxml forbids unsafe code, so it cannot
//! install a failing allocator; this target can. Input: four bytes choosing
//! which allocation fails, then the `driver::allocfail_run` input. Each input
//! runs three times: unarmed (counting allocations), armed (allocation
//! `1 + choice % count` returns null), and unarmed again. The armed result
//! must equal the first or be an allocation error; the third must equal the
//! first. libFuzzer's own allocations are C++ and never pass through here.
//!
//! An infallible allocation (`Vec::push`, `String` formatting, ...) that hits
//! the null aborts in std; the alloc error hook unwinds instead so the run is
//! dropped quietly, as std's policy makes those failures fatal anyway.
//!
//! Triage: `YAXML_ALLOCFAIL_TRACE=1` prints a backtrace at the failed
//! allocation; `YAXML_ALLOCFAIL_SWEEP=1` fails each allocation in turn, which
//! is how `regressions/allocfail` replays. Audit: `YAXML_ALLOCFAIL_MIN_SIZE=n` instead fails every armed
//! allocation of at least n bytes, so with the trace, large inputs show the
//! infallible allocations that grow with them (docs/security.md).
#![no_main]
#![feature(alloc_error_hook)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

#[allow(dead_code)]
#[path = "../../../yaxml/tests/support/fuzz/driver.rs"]
mod driver;

/// Allocations since the last reset, the one to fail (0: none) and the
/// smallest size that fails (audit mode).
static COUNT: AtomicUsize = AtomicUsize::new(0);
static FAIL_AT: AtomicUsize = AtomicUsize::new(0);
static FAIL_SIZE: AtomicUsize = AtomicUsize::new(usize::MAX);
static TRACE: AtomicBool = AtomicBool::new(false);

struct Failing;

impl Failing {
    fn fails(size: usize) -> bool {
        let hit = COUNT.fetch_add(1, Relaxed) + 1 == FAIL_AT.load(Relaxed)
            || size >= FAIL_SIZE.load(Relaxed);
        if hit && TRACE.load(Relaxed) {
            // Triage only: the capture re-enters this allocator, with sizes
            // disarmed; a write error is ignored, as `alloc` must not unwind.
            let armed = FAIL_SIZE.swap(usize::MAX, Relaxed);
            let trace = Backtrace::force_capture();
            let _ = writeln!(
                std::io::stderr(),
                "failed allocation of {size} bytes\n{trace}"
            );
            FAIL_SIZE.store(armed, Relaxed);
        }
        hit
    }
}

// SAFETY: every call forwards to `System` unchanged or returns null, which
// `GlobalAlloc` permits for any allocation request.
unsafe impl GlobalAlloc for Failing {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if Self::fails(layout.size()) {
            null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if Self::fails(layout.size()) {
            null_mut()
        } else {
            unsafe { System.alloc_zeroed(layout) }
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if Self::fails(size) {
            null_mut()
        } else {
            unsafe { System.realloc(ptr, layout, size) }
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Failing = Failing;

/// Panic payload of an infallible allocation that hit the injected failure.
struct Infallible;

/// Which allocations an input fails.
#[derive(Clone, Copy)]
enum Mode {
    /// The one its first four bytes choose.
    Chosen,
    /// Each in turn (`YAXML_ALLOCFAIL_SWEEP`): replays a regression without
    /// depending on how many allocations precede the faulty one.
    Sweep,
    /// Every one of at least this many bytes (`YAXML_ALLOCFAIL_MIN_SIZE`).
    Size(usize),
}

/// Installs the hooks once; the mode comes from the environment.
fn install() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        TRACE.store(std::env::var_os("YAXML_ALLOCFAIL_TRACE").is_some(), Relaxed);
        std::alloc::set_alloc_error_hook(|layout| {
            if TRACE.load(Relaxed) {
                let _ = writeln!(std::io::stderr(), "infallible: {} bytes", layout.size());
            }
            std::panic::panic_any(Infallible)
        });
        // libfuzzer-sys's hook aborts on every panic; let these unwind.
        let abort = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !info.payload().is::<Infallible>() {
                abort(info);
            }
        }));
        let size = std::env::var("YAXML_ALLOCFAIL_MIN_SIZE").ok();
        match size.and_then(|size| size.parse().ok()) {
            Some(size) => Mode::Size(size),
            None if std::env::var_os("YAXML_ALLOCFAIL_SWEEP").is_some() => Mode::Sweep,
            None => Mode::Chosen,
        }
    })
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some((choice, input)) = data.split_first_chunk::<4>() else {
        return;
    };
    let mode = install();
    COUNT.store(0, Relaxed);
    let baseline = driver::allocfail_run(input);
    let count = COUNT.load(Relaxed);
    let chosen = 1 + u32::from_le_bytes(*choice) as usize % count.max(1);
    // The allocation index each armed run fails (0: by size instead).
    let runs = match mode {
        Mode::Chosen => chosen..=chosen,
        Mode::Sweep => 1..=count,
        Mode::Size(_) => 0..=0,
    };
    for at in runs {
        let size = match mode {
            Mode::Size(size) => size,
            _ => usize::MAX,
        };
        COUNT.store(0, Relaxed);
        FAIL_AT.store(at, Relaxed);
        FAIL_SIZE.store(size, Relaxed);
        let armed = catch_unwind(AssertUnwindSafe(|| driver::allocfail_run(input)));
        FAIL_AT.store(0, Relaxed);
        FAIL_SIZE.store(usize::MAX, Relaxed);
        match armed {
            Ok(armed) => driver::allocfail_check(&baseline, &armed),
            Err(payload) if payload.is::<Infallible>() => {}
            Err(payload) => resume_unwind(payload),
        }
        let rerun = driver::allocfail_run(input);
        assert_eq!(rerun, baseline, "rerun after failure differs");
    }
});
