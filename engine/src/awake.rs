//! Keeps the computer from sleeping while something downloads (Windows).
//!
//! `SetThreadExecutionState` belongs to the thread that calls it and tokio moves tasks between
//! threads, so one small dedicated thread owns the request: it holds
//! `ES_CONTINUOUS | ES_SYSTEM_REQUIRED` while any `Awake` guard is alive and releases it when the
//! last one drops. The display may still turn off. Linux: nothing in v0.1 (the retry covers a
//! sleep); `hold` is a counter only.

use std::sync::{Condvar, Mutex, OnceLock};

static COUNT: Mutex<usize> = Mutex::new(0);
static CHANGED: Condvar = Condvar::new();
static THREAD: OnceLock<()> = OnceLock::new();

/// While alive, the system stays awake. Cheap; nest freely.
pub struct Awake(());

pub fn hold() -> Awake {
    THREAD.get_or_init(|| {
        std::thread::Builder::new().name("keep-awake".into()).spawn(run).expect("keep-awake thread");
    });
    *COUNT.lock().unwrap() += 1;
    CHANGED.notify_all();
    Awake(())
}

impl Drop for Awake {
    fn drop(&mut self) {
        *COUNT.lock().unwrap() -= 1;
        CHANGED.notify_all();
    }
}

/// Number of live guards (tests, and `/api/hardware`-style reporting if ever needed).
pub fn holders() -> usize {
    *COUNT.lock().unwrap()
}

fn run() {
    let mut held = false;
    let mut n = COUNT.lock().unwrap();
    loop {
        let want = *n > 0;
        if want != held {
            set(want);
            held = want;
            eprintln!("[engine] keep-awake {}", if want { "on (downloading)" } else { "off" });
        }
        n = CHANGED.wait(n).unwrap();
    }
}

#[cfg(windows)]
fn set(on: bool) {
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
    let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
    unsafe { SetThreadExecutionState(flags) };
}

#[cfg(not(windows))]
fn set(_on: bool) {}

#[cfg(test)]
mod tests {
    #[test]
    fn guards_count_up_and_down() {
        let base = super::holders();
        let a = super::hold();
        let b = super::hold();
        assert_eq!(super::holders(), base + 2);
        drop(a);
        drop(b);
        assert_eq!(super::holders(), base);
    }
}
