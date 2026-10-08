//! The suppressions each language pack reads (#493): every spelling a pack's table
//! names, where it sits in the file, and what the pack reports as the rule and the
//! snippet. Each pack has controls beside its spellings: the same words in a string, in
//! a comment that is not a suppression, and in a neighbouring spelling the pack does not
//! read. Each pack reads these from one table, through `ast::suppressions`.

use discipline::ast::{default_registry, AssertVocabulary, EscapeHatchSite};

/// The suppressions of `src` read as the file `path`, one line each: the line, `type`
/// with the tool or `lint` with the rule, and the snippet.
fn sites(path: &str, src: &str) -> Vec<String> {
    let registry = default_registry();
    let pack = registry.find_pack(path).expect("a pack for the path");
    let facts = pack
        .extract(path, src, &AssertVocabulary::default())
        .expect("the source is read");
    facts
        .escape_hatches
        .iter()
        .map(|site| match site {
            EscapeHatchSite::TypeIgnore {
                line,
                tool,
                snippet,
            } => format!("{line} type {tool:?} {snippet:?}"),
            EscapeHatchSite::LinterDisable {
                line,
                rule,
                snippet,
            } => format!("{line} lint {rule:?} {snippet:?}"),
            EscapeHatchSite::UnsafeBlock { line, .. } => format!("{line} unsafe"),
        })
        .collect()
}

fn check(path: &str, src: &str, expected: &[&str]) {
    let got = sites(path, src);
    assert_eq!(got, expected, "{path}:\n{src}");
}

#[test]
fn javascript_reads_its_type_and_linter_comments() {
    let src = "\
// @ts-nocheck
/* eslint-disable no-console */
import { a } from 'a';
class A {
  // eslint-disable-next-line
  f() {
    // @ts-ignore
    const x = a(); // eslint-disable-line no-alert, no-eval
    /* @ts-expect-error TS2322 */
    const y = x;
    // eslint-disable-next-line no-eval -- reason
    /* istanbul ignore next */
    return y; // c8 ignore next
  }
}
// eslint-disable
";
    let expected = [
        r#"1 type "typescript" "// @ts-nocheck""#,
        r#"2 lint "no-console" "/* eslint-disable no-console */""#,
        r#"5 lint "all" "// eslint-disable-next-line""#,
        r#"7 type "typescript" "// @ts-ignore""#,
        r#"8 lint "no-alert, no-eval" "// eslint-disable-line no-alert, no-eval""#,
        r#"9 type "typescript" "/* @ts-expect-error TS2322 */""#,
        r#"11 lint "no-eval -- reason" "// eslint-disable-next-line no-eval -- reason""#,
        r#"12 lint "coverage" "/* istanbul ignore next */""#,
        r#"13 lint "coverage" "// c8 ignore next""#,
        r#"16 lint "all" "// eslint-disable""#,
    ];
    for path in ["src/m.js", "src/m.ts", "src/m.tsx", "src/m.mjs"] {
        check(path, src, &expected);
    }
}

#[test]
fn javascript_reads_no_suppression_from_a_string_or_another_comment() {
    check(
        "src/m.ts",
        "\
// eslint-enable
// @ts-check
// x @ts-ignore
// x eslint-disable
const s = \"// eslint-disable\";
const t = `/* @ts-ignore */`;
const u = '// istanbul ignore next';
# eslint-disable
",
        &[],
    );
}

/// A coverage marker counts anywhere in a comment, and the first rule a comment
/// matches is the one reported.
#[test]
fn javascript_coverage_marker_counts_anywhere_in_a_comment() {
    check(
        "src/m.js",
        "\
// do not write istanbul ignore here
/* see c8 ignore */
// @ts-ignore eslint-disable istanbul ignore
// eslint-disable istanbul ignore
//// eslint-disable-line a
// eslint-disable-lineeslint-disable-line b
",
        &[
            r#"1 lint "coverage" "// do not write istanbul ignore here""#,
            r#"2 lint "coverage" "/* see c8 ignore */""#,
            r#"3 type "typescript" "// @ts-ignore eslint-disable istanbul ignore""#,
            r#"4 lint "istanbul ignore" "// eslint-disable istanbul ignore""#,
            r#"5 lint "a" "//// eslint-disable-line a""#,
            r#"6 lint "eslint-disable-line b" "// eslint-disable-lineeslint-disable-line b""#,
        ],
    );
}

#[test]
fn python_reads_its_type_and_linter_comments() {
    check(
        "src/m.py",
        "\
# ruff: noqa
# pylint: disable=unused-import
import os  # noqa: F401
class A:
    # ruff: noqa: E501
    def test_f(self):  # pragma: no cover - reason
        x = os.sep  # type: ignore[attr-defined]
        y = x  # noqa
        ## noqa:E1,E2
        #noqa:
        assert y  # pylint: disable=a, b
",
        &[
            r##"1 lint "all" "# ruff: noqa""##,
            r##"2 lint "unused-import" "# pylint: disable=unused-import""##,
            r##"3 lint "F401" "# noqa: F401""##,
            r##"5 lint "E501" "# ruff: noqa: E501""##,
            r##"6 lint "coverage" "# pragma: no cover - reason""##,
            r##"7 type "mypy" "# type: ignore[attr-defined]""##,
            r##"8 lint "all" "# noqa""##,
            r###"9 lint "E1,E2" "## noqa:E1,E2""###,
            r##"10 lint "" "#noqa:""##,
            r##"11 lint "a, b" "# pylint: disable=a, b""##,
        ],
    );
}

#[test]
fn python_reads_no_suppression_from_a_string_or_another_comment() {
    check(
        "src/m.py",
        "\
# pylint: enable=x
# pylint: disable-next=x
# pragma: no branch
# type:ignore
# NOQA
# x noqa
#/* noqa */
#// noqa
s = \"# noqa\"
t = '''# type: ignore'''
",
        &[],
    );
}

#[test]
fn ruby_reads_rubocop_comments() {
    check(
        "src/m.rb",
        "# rubocop:disable Metrics/AbcSize\nclass A\n  # rubocop:todo Style/X\n  def f\n    x = 1 # rubocop:disable A, B  \n    # rubocop:disable\n  end\nend\n",
        &[
            r##"1 lint "Metrics/AbcSize" "# rubocop:disable Metrics/AbcSize""##,
            r##"3 lint "Style/X" "# rubocop:todo Style/X""##,
            r##"5 lint "A, B" "# rubocop:disable A, B""##,
            r##"6 lint "" "# rubocop:disable""##,
        ],
    );
}

#[test]
fn ruby_reads_no_suppression_from_a_string_or_another_comment() {
    check(
        "src/m.rb",
        "\
# rubocop:enable A
#rubocop:disable A
#  rubocop:disable A
## rubocop:disable A
// rubocop:disable A
s = \"# rubocop:disable A\"
=begin
# rubocop:disable A
=end
",
        &[],
    );
}

#[test]
fn java_reads_the_suppress_warnings_annotation() {
    check(
        "src/M.java",
        "\
package p;
@SuppressWarnings(\"unchecked\")
class A {
  @SuppressWarnings({\"a\", \"b\"})
  int field;
  @SuppressWarnings
  void f(@SuppressWarnings( \"p\" ) int p) {
    @SuppressWarnings(value = \"x\")
    int x = 1;
    @SuppressWarnings(\"\")
    int y = 2;
  }
}
",
        &[
            r#"2 lint "unchecked" "@SuppressWarnings(\"unchecked\")""#,
            r#"4 lint "{\"a\", \"b\"}" "@SuppressWarnings({\"a\", \"b\"})""#,
            r#"6 lint "all" "@SuppressWarnings""#,
            r#"7 lint "p" "@SuppressWarnings( \"p\" )""#,
            r#"8 lint "value = \"x" "@SuppressWarnings(value = \"x\")""#,
            r#"10 lint "" "@SuppressWarnings(\"\")""#,
        ],
    );
}

/// A comment is never a suppression in Java, and an annotation inside another's
/// arguments is not read.
#[test]
fn java_reads_no_suppression_from_a_comment_a_string_or_another_annotation() {
    check(
        "src/M.java",
        "\
// @SuppressWarnings(\"x\")
/* @SuppressWarnings(\"x\") */
/** {@code @SuppressWarnings(\"x\")} */
// NOLINT
@Deprecated
@Override
@java.lang.SuppressWarnings(\"x\")
@Outer(@SuppressWarnings(\"inner\"))
class A {
  String s = \"@SuppressWarnings\";
}
",
        &[],
    );
}

#[test]
fn kotlin_reads_its_suppress_annotations() {
    check(
        "src/m.kt",
        "\
@Suppress(\"UNCHECKED_CAST\")
class A {
  @SuppressWarnings(\"x\")
  val field = 1
  @SuppressLint(\"NewApi\")
  fun f() {
    @Suppress(\"A\", \"B\")
    val x = 1
  }
  @Suppress
  fun g() {}
  @get:Suppress(\"G\")
  val p = 1
}
",
        &[
            r#"1 lint "UNCHECKED_CAST" "@Suppress(\"UNCHECKED_CAST\")""#,
            r#"3 lint "x" "@SuppressWarnings(\"x\")""#,
            r#"5 lint "NewApi" "@SuppressLint(\"NewApi\")""#,
            r#"7 lint "A\", \"B" "@Suppress(\"A\", \"B\")""#,
            r#"10 lint "all" "@Suppress""#,
            r#"12 lint "G" "@get:Suppress(\"G\")""#,
        ],
    );
}

#[test]
fn kotlin_reads_no_suppression_from_a_comment_a_string_or_another_annotation() {
    check(
        "src/m.kt",
        "\
// @Suppress(\"x\")
/* @Suppress(\"x\") */
@Deprecated(\"x\")
class A {
  val s = \"@Suppress\"
  @Outer(Suppress(\"inner\"))
  fun f() {}
}
",
        &[],
    );
}

#[test]
fn scala_reads_its_annotations_and_linter_comments() {
    check(
        "src/m.scala",
        "\
// scalastyle:off magic.number
@nowarn
class A {
  @nowarn(\"cat=deprecation\")
  val field = 1
  @SuppressWarnings(Array(\"x\"))
  def f(p: Int): Unit = {
    /* scalafix:off */
    val y = (p: @unchecked)
    val z = y // scalafix:ok Rule; reason
    // scalafix:off DisableSyntax.var extra
  }
  @nowarn(\"\")
  def g(): Unit = ()
  @nowarn( \"p\" )
  def h(): Unit = ()
}
",
        &[
            r#"1 lint "magic.number" "// scalastyle:off magic.number""#,
            r#"2 lint "all" "@nowarn""#,
            r#"4 lint "cat=deprecation" "@nowarn(\"cat=deprecation\")""#,
            r#"6 lint "Array(\"x" "@SuppressWarnings(Array(\"x\"))""#,
            r#"8 lint "all" "/* scalafix:off */""#,
            r#"9 lint "all" "@unchecked""#,
            r#"10 lint "Rule;" "// scalafix:ok Rule; reason""#,
            r#"11 lint "DisableSyntax.var" "// scalafix:off DisableSyntax.var extra""#,
            r#"13 lint "all" "@nowarn(\"\")""#,
            r#"15 lint " \"p\" " "@nowarn( \"p\" )""#,
        ],
    );
}

#[test]
fn scala_reads_no_suppression_from_a_string_or_another_comment() {
    check(
        "src/m.scala",
        "\
// scalastyle:on
// scalafix:on
// x scalastyle:off
// @nowarn
/* outer /* scalastyle:off */ */
@deprecated
class A {
  val s = \"// scalastyle:off\"
}
",
        &[],
    );
}

#[test]
fn swift_reads_swiftlint_comments() {
    check(
        "src/m.swift",
        "\
// swiftlint:disable force_cast line_length
class A {
  // swiftlint:disable:next force_try
  func f() {
    let x = 1 // swiftlint:disable:this x
    // swiftlint:disable:previous y
    /* swiftlint:disable:next z */
    // swiftlint:disable
    // swiftlint:disable:next
    // swiftlint:disable:next:this:previous w
  }
}
",
        &[
            r#"1 lint "force_cast" "// swiftlint:disable force_cast line_length""#,
            r#"3 lint "force_try" "// swiftlint:disable:next force_try""#,
            r#"5 lint "x" "// swiftlint:disable:this x""#,
            r#"6 lint "y" "// swiftlint:disable:previous y""#,
            r#"7 lint "z" "/* swiftlint:disable:next z */""#,
            r#"8 lint "all" "// swiftlint:disable""#,
            r#"9 lint "all" "// swiftlint:disable:next""#,
            r#"10 lint "w" "// swiftlint:disable:next:this:previous w""#,
        ],
    );
}

#[test]
fn swift_reads_no_suppression_from_a_string_or_another_comment() {
    check(
        "src/m.swift",
        "\
// swiftlint:enable x
// x swiftlint:disable y
class A {
  let s = \"// swiftlint:disable x\"
}
",
        &[],
    );
}
