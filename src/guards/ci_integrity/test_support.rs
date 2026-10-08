//! Fixtures the `ci-integrity` unit tests share.

pub(super) fn steps(yaml: &str) -> Vec<serde_yaml::Value> {
    serde_yaml::from_str::<serde_yaml::Value>(yaml)
        .unwrap()
        .as_sequence()
        .unwrap()
        .clone()
}

pub(super) const SHA: &str = "b4ffde65f46336ab88eb53be808477a3936bae11";

thread_local! {
    /// Every text `load_yaml` was given on this thread, in order.
    static LOADS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Records one YAML load of `text`.
pub(super) fn record_load(text: &str) {
    LOADS.with(|l| l.borrow_mut().push(text.to_string()));
}

/// How many times `load_yaml` was given exactly `text` on this thread while `run` ran.
pub(super) fn loads_of<T>(text: &str, run: impl FnOnce() -> T) -> (usize, T) {
    LOADS.with(|l| l.borrow_mut().clear());
    let result = run();
    let count = LOADS.with(|l| l.borrow().iter().filter(|t| t.as_str() == text).count());
    (count, result)
}
