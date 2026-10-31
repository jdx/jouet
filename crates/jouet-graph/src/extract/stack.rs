//! Stack-budget guard for the recursive walkers.
//!
//! Every walker recurses per AST level, and tree-sitter parses a
//! pathologically nested file (clang's `parser_overflow.c` nests 16,384 `{`)
//! without trouble, so it is the walk that would overflow. codegraph's guard
//! (`codegraph-kernel/src/stack.rs`, #1581) asks the OS for each thread's
//! stack bounds; jouet runs every extraction on a thread it spawns with a
//! known stack ([`STACK_SIZE`]), so a descent budget measured from the
//! entry stack pointer is exact and needs no platform code.
//!
//! `exhausted()` is checked at the top of every recursive walker function
//! (the `stack_guard!` macro). Once it trips it latches, every walker
//! returns at once, and [`run_guarded`] reports the file as too deeply
//! nested instead of letting the process die.

use std::cell::Cell;

/// The stack each extraction thread gets.
pub(crate) const STACK_SIZE: usize = 64 * 1024 * 1024;

/// How far below the entry stack pointer a walk may go: the rest of
/// [`STACK_SIZE`] is headroom for the frames between two guard checks and
/// for everything that runs after the guard trips.
const BUDGET: usize = STACK_SIZE - 8 * 1024 * 1024;

thread_local! {
    /// Lowest stack-pointer value the walk may reach. `0` outside a walk.
    static THRESHOLD: Cell<usize> = const { Cell::new(0) };
    static OVERFLOWED: Cell<bool> = const { Cell::new(false) };
}

/// Approximate current stack pointer: the address of a local. Stacks grow
/// downward on every target jouet ships for.
#[inline(always)]
fn current_sp() -> usize {
    let marker = 0u8;
    std::hint::black_box(&marker) as *const u8 as usize
}

pub(crate) fn exhausted() -> bool {
    if OVERFLOWED.get() {
        return true;
    }
    let threshold = THRESHOLD.get();
    if threshold != 0 && current_sp() < threshold {
        OVERFLOWED.set(true);
        return true;
    }
    false
}

/// Runs one walk under the guard. Call it at the top of a thread spawned
/// with [`STACK_SIZE`].
pub(crate) fn run_guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    OVERFLOWED.set(false);
    THRESHOLD.set(current_sp().saturating_sub(BUDGET));
    let out = f();
    THRESHOLD.set(0);
    if OVERFLOWED.replace(false) {
        return Err("nested too deeply to walk".to_string());
    }
    out
}
