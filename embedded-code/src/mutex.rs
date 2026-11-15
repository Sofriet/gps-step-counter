//! A `Mutex<T>` for data shared between the main code and interrupt handlers
//! on a single core. It disables interrupts while the data is used.
//!
//! A spinlock would not work here: if the main code holds the lock and an
//! interrupt handler tries to take it, the handler spins forever because the
//! main code can't continue until the handler returns.

use core::cell::{Cell, UnsafeCell};
use cortex_m::interrupt;

/// A Mutex holding a `T`. It can only be shared (`Sync`) if `T` is `Send`.
pub struct Mutex<T> {
    /// UnsafeCell, because the data is changed through a shared reference
    /// (the Mutex is a static).
    data: UnsafeCell<T>,
    /// True while `update` is running. Used to catch a nested `update` on the
    /// same Mutex, which would otherwise give out a second `&mut T`.
    locked: Cell<bool>,
}

// SAFETY: sharing a Mutex<T> (Sync) shares the T inside it, so T must be Send.
// On this single-core Cortex-M3 there are no threads, only the main code and
// interrupt handlers, and `data` and `locked` are only used in `update` with
// interrupts disabled, so only one of them can use them at a time.
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// Create a new Mutex. This is a const fn so it can be used for statics.
    pub const fn new(data: T) -> Self {
        Self {
            data: UnsafeCell::new(data),
            locked: Cell::new(false),
        }
    }

    /// Run `f` with exclusive access to the data and interrupts disabled, and
    /// return its result. Afterwards interrupts are enabled again if they were
    /// enabled before.
    ///
    /// Panics if `f` calls `update` on the same Mutex again.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        interrupt::free(|_| {
            assert!(!self.locked.get(), "Mutex::update called inside update");
            self.locked.set(true);

            // SAFETY: interrupts are disabled, so no handler can run and use
            // the data, and `locked` was false, so no outer `update` holds a
            // `&mut T` to it. `f` can't return the reference (R can't borrow
            // from it), so it is gone before interrupts are enabled again.
            let r = unsafe { &mut *self.data.get() };
            let result = f(r);
            self.locked.set(false);
            result
        })
    }
}
