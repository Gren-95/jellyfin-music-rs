//! Ctrl-C (SIGINT) and SIGTERM during a sync. The first signal only sets a flag: the sync stops
//! starting downloads, breaks off the running ones between two reads (keeping their `.part`
//! files for the next run) and deletes nothing. A second signal ends the process at once.
//!
//! Signal handlers are installed with libc's `signal` through a tiny FFI declaration instead of
//! a crate. The handler only touches an atomic and calls `_exit`, both async-signal-safe.

use std::sync::atomic::{AtomicBool, Ordering};

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
/// The exit code of the second signal, the shell's convention for "killed by SIGINT".
const FORCED_EXIT_CODE: i32 = 130;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn signal(signum: i32, handler: usize) -> usize;
    fn _exit(status: i32) -> !;
}

extern "C" fn on_signal(_signum: i32) {
    if INTERRUPTED.swap(true, Ordering::SeqCst) {
        // SAFETY: `_exit` is async-signal-safe and never returns.
        unsafe { _exit(FORCED_EXIT_CODE) }
    }
}

/// Routes SIGINT and SIGTERM to the flag read by [`interrupted`].
pub fn install() {
    let handler: extern "C" fn(i32) = on_signal;
    for signum in [SIGINT, SIGTERM] {
        // SAFETY: installs a handler with the C ABI `void (*)(int)` signature `signal` expects.
        unsafe {
            signal(signum, handler as usize);
        }
    }
}

/// Whether Ctrl-C or SIGTERM was received.
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}
