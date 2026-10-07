//! The stack the work of a run is done on.
//!
//! Most tree walkers call themselves once for each level of a syntax tree, and a tree
//! may nest [`TREE_DEPTH_LIMIT`](crate::ast::source_text::TREE_DEPTH_LIMIT) levels. A
//! thread's default stack does not hold that: the walker that takes the most stack for
//! a level ([`MEASURED_STACK_PER_LEVEL`]) ends a 2 MiB stack at 623 levels and the main
//! thread's 8 MiB at four times that. So the binary does all of its work inside
//! [`on_deep_stack`], on a thread whose stack is [`WORK_STACK_BYTES`]. The limit and
//! the stack are set against each other (`the_stack_holds_the_deepest_tree_four_times`):
//! a tree at the limit takes at most one fourth of the stack in the worst case measured.
//!
//! The stack is address space the thread reserves; a page of it is memory only once a
//! walker has reached it, so a run over ordinary sources uses what it used before.
//!
//! Anything that hands a pack a deeply nested source from another thread (a fuzz
//! target, a test) goes through [`on_deep_stack`] as the binary does: the depth limit
//! protects a walker on this stack, not on a smaller one.

/// The stack of the thread the work runs on.
pub const WORK_STACK_BYTES: usize = 256 * 1024 * 1024;

/// The most stack a walker took for one level of a tree, in bytes, in the worst case
/// measured: a build without optimisation, where a frame holds every temporary of its
/// function. It is the Go pack's call scanner on nested calls, which ended a thread's
/// 2 MiB stack at a tree 623 levels deep (2,097,152 / 623 = 3,366, rounded up). Every
/// other walker took less: from 3,190 bytes (Java, a chain of method calls) down to
/// 480 (the blanking of strings in a Rust macro argument parsed on its own).
pub const MEASURED_STACK_PER_LEVEL: usize = 3_400;

/// How many times over the stack holds a tree at the depth limit in that worst case.
pub const STACK_SAFETY_FACTOR: usize = 4;

// The limit and the stack against each other, checked when the crate is built: a tree
// at the depth limit, descended by the walker that takes the most stack for a level,
// fits in the stack `STACK_SAFETY_FACTOR` times.
const _: () = assert!(
    crate::ast::source_text::TREE_DEPTH_LIMIT * MEASURED_STACK_PER_LEVEL * STACK_SAFETY_FACTOR
        <= WORK_STACK_BYTES
);

/// `work`'s result, with `work` run on a thread whose stack is [`WORK_STACK_BYTES`].
///
/// The thread has the caller's name, so a message that names the thread (a panic, a
/// stack overflow) reads as it did. A panic of `work` goes on in the caller with the
/// same payload: the hook has already reported it on the thread it happened on, and
/// the caller ends as it would have ended had it panicked itself. The error is the
/// thread that could not be started; nothing has run then.
pub fn on_deep_stack<T: Send>(work: impl FnOnce() -> T + Send) -> std::io::Result<T> {
    let name = std::thread::current()
        .name()
        .unwrap_or("worker")
        .to_string();
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name(name)
            .stack_size(WORK_STACK_BYTES)
            .spawn_scoped(scope, work)?;
        match worker.join() {
            Ok(result) => Ok(result),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::source_text::TREE_DEPTH_LIMIT;

    /// The arithmetic the limit and the stack are set by: a tree at the depth limit,
    /// descended by the walker that takes the most stack for a level, takes no more
    /// than one fourth of the stack.
    #[test]
    fn the_stack_holds_the_deepest_tree_four_times() {
        let deepest_descent = TREE_DEPTH_LIMIT * MEASURED_STACK_PER_LEVEL;
        assert_eq!(deepest_descent, 13_926_400);
        assert!(deepest_descent * STACK_SAFETY_FACTOR <= WORK_STACK_BYTES);
        // How many times it holds it in fact.
        assert_eq!(WORK_STACK_BYTES / deepest_descent, 19);
        // The figures the per-level cost comes from: a 2 MiB stack ended at 623 levels.
        assert_eq!(2 * 1024 * 1024 / 623, 3_366);
        const { assert!(MEASURED_STACK_PER_LEVEL >= 3_366) };
        // And the limit is above the deepest source found in other projects (2,056
        // levels, a header of generated macros) by about a factor of two.
        const { assert!(TREE_DEPTH_LIMIT >= 4_096) };
    }

    #[test]
    fn the_work_runs_on_a_thread_of_the_callers_name_and_returns_its_result() {
        let outer = std::thread::current().id();
        let name = std::thread::current().name().map(str::to_string);
        let borrowed = String::from("borrowed by the work");
        let (inner, inner_name, len) = on_deep_stack(|| {
            (
                std::thread::current().id(),
                std::thread::current().name().map(str::to_string),
                borrowed.len(),
            )
        })
        .unwrap();
        assert_ne!(inner, outer);
        assert_eq!(inner_name, name);
        assert_eq!(len, 20);
    }

    /// A descent that a default stack cannot hold: 16,000 frames of at least 1 kB, more
    /// than the 8 MiB of a main thread and eight times a spawned thread's 2 MiB.
    #[test]
    fn the_stack_holds_a_descent_a_default_stack_does_not() {
        fn descend(levels: usize) -> usize {
            let frame = std::hint::black_box([1u8; 1024]);
            if levels == 0 {
                return 0;
            }
            descend(levels - 1) + frame[levels % 1024] as usize
        }
        assert_eq!(on_deep_stack(|| descend(16_000)).unwrap(), 16_000);
    }

    /// A panic of the work is the caller's panic, with its payload.
    #[test]
    fn a_panic_of_the_work_is_the_callers() {
        let caught =
            std::panic::catch_unwind(|| on_deep_stack(|| -> u8 { panic!("the work failed") }));
        let payload = caught.unwrap_err();
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"the work failed"));
    }
}
