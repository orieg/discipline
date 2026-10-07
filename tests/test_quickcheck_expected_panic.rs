//! `#[should_panic]` on a case inside `quickcheck!` is an expectation, compared as one
//! on a `proptest!` case is (#626). Both macros already read the attribute; this pins it
//! through the real binary for each.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WIDENED: &str = "Expected Exception Or Panic Widened";

fn change(file: &str, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    repo.write(file, base);
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo.write(file, head);
    repo.commit("refactor: change");
    repo.check(&["--base", "main"])
}

fn titles(run: &Run) -> Vec<String> {
    run.titles("assertion-reduction")
}

#[test]
fn a_should_panic_on_a_quickcheck_case_is_compared_as_on_a_proptest_case() {
    let quickcheck = |attr: &str| {
        format!("quickcheck! {{\n    {attr}\n    fn adds(a: u8, b: u8) -> bool {{\n        add(a, b) >= a\n    }}\n}}\n")
    };
    let proptest = |attr: &str| {
        format!("proptest! {{\n    #[test]\n    {attr}\n    fn adds(a in 0..9u8, b in 0..9u8) {{\n        prop_assert!(add(a, b) >= a);\n    }}\n}}\n")
    };
    let exact = "#[should_panic(expected = \"overflow\")]";
    for form in [&quickcheck as &dyn Fn(&str) -> String, &proptest] {
        let run = change("tests/props.rs", &form(exact), &form("#[should_panic]"));
        assert_eq!(titles(&run), vec![WIDENED.to_string()], "{}", run.stdout);
        // Control: the matcher kept, a comment added.
        let kept = format!("// Overflow.\n{}", form(exact));
        let run = change("tests/props.rs", &form(exact), &kept);
        assert_eq!(titles(&run), Vec::<String>::new(), "{}", run.stdout);
    }
}
