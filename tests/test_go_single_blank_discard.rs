//! Go: `_ = f()` of a call whose callee is known to return an error is a discarded result
//! (#651).
//!
//! `error-swallowing` sorts a Go discard by the callee's name. The two-value form
//! (`_, _ = f.Write(nil)`) was read; the single blank assignment (`_ = os.Remove(p)`) was
//! not. It is now `Result Discarded` when the callee is on the known-fallible list, the
//! same evidence the two-value form uses. A callee off that list, a conversion and a type
//! assertion assigned to a single blank are not reported: nothing says their one value is
//! an error.
//!
//! Each case drives the real binary over a base side without the statement and a head
//! side that adds it. The sources are fixtures built by this file.

mod common;
use common::Repo;

fn source(statement: &str) -> String {
    format!(
        "package store\n\nimport (\n\t\"os\"\n\t\"strings\"\n)\n\nvar _ = strings.ToUpper\n\nfunc Clean(p string, f *os.File, v interface{{}}) {{\n{statement}\tprintln(p, f, v)\n}}\n"
    )
}

fn titles(statement: &str) -> (i32, Vec<String>) {
    let repo = Repo::new();
    repo.commit_base_files(&[("store/clean.go", &source(""))], "feat: base");
    repo.write("store/clean.go", &source(statement));
    repo.commit("feat: clean up");
    let run = repo.check(&[]);
    (run.code, run.titles("error-swallowing"))
}

#[test]
fn a_single_blank_assignment_of_a_known_fallible_call_is_a_discarded_result() {
    for statement in [
        "\t_ = os.Remove(p)\n",
        "\t_ = f.Close()\n",
        "\t_ = os.RemoveAll(p)\n",
    ] {
        let (code, titles) = titles(statement);
        assert_eq!(
            (code, titles.as_slice()),
            (1, &["Result Discarded".to_string()][..]),
            "{statement}"
        );
    }
}

#[test]
fn the_two_value_form_is_reported_as_before() {
    let (code, titles) = titles("\t_, _ = f.Write(nil)\n");
    assert_eq!(
        (code, titles.as_slice()),
        (1, &["Result Discarded".to_string()][..])
    );
}

#[test]
fn a_single_blank_assignment_of_anything_else_is_not_reported() {
    for statement in [
        "\t_ = strings.ToUpper(p)\n",
        "\t_ = len(p)\n",
        "\t_ = []byte(p)\n",
        "\t_ = v.(string)\n",
        "\t_ = p\n",
    ] {
        let (code, titles) = titles(statement);
        assert_eq!((code, titles.len()), (0, 0), "{statement}: {titles:?}");
    }
}
