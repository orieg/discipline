//! Fixtures the `ci-integrity` unit tests share.

pub(super) fn steps(yaml: &str) -> Vec<serde_yaml::Value> {
    serde_yaml::from_str::<serde_yaml::Value>(yaml)
        .unwrap()
        .as_sequence()
        .unwrap()
        .clone()
}

pub(super) const SHA: &str = "b4ffde65f46336ab88eb53be808477a3936bae11";
