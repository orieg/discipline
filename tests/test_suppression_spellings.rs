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
# pragma: no branch
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

/// A comment that only quotes an annotation is not a suppression in Java, and an
/// annotation inside another's arguments is not read.
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

// The spellings below are ones a tool honours that no pack read before (#685).

/// The annotation is read by the last segment of its name, so the qualified form is a
/// site. The package is not resolved: `SuppressWarnings` of another package is read too.
#[test]
fn java_reads_a_qualified_suppress_warnings_annotation() {
    check(
        "src/M.java",
        "\
package p;
@java.lang.SuppressWarnings(\"unchecked\")
class A {
  @java.lang.SuppressWarnings({\"a\", \"b\"})
  int field;
  @java.lang.SuppressWarnings
  void f(@my.pkg.SuppressWarnings( \"p\" ) int p) {
    @java.lang.SuppressWarnings(\"x\")
    int x = 1;
  }
}
",
        &[
            r#"2 lint "unchecked" "@java.lang.SuppressWarnings(\"unchecked\")""#,
            r#"4 lint "{\"a\", \"b\"}" "@java.lang.SuppressWarnings({\"a\", \"b\"})""#,
            r#"6 lint "all" "@java.lang.SuppressWarnings""#,
            r#"7 lint "p" "@my.pkg.SuppressWarnings( \"p\" )""#,
            r#"8 lint "x" "@java.lang.SuppressWarnings(\"x\")""#,
        ],
    );
}

#[test]
fn java_reads_no_suppression_from_an_annotation_whose_last_segment_is_another_name() {
    check(
        "src/M.java",
        "\
@my.NotSuppressWarnings(\"x\")
@Suppressed
@java.lang.Deprecated
@SuppressWarnings.Inner(\"x\")
@suppresswarnings(\"x\")
class A {
  String s = \"@java.lang.SuppressWarnings\";
}
",
        &[],
    );
}

#[test]
fn java_reads_nosonar_and_noinspection_comments() {
    check(
        "src/M.java",
        "// NOSONAR\npackage p;\nclass A { // NOSONAR\n  int field = 1; // why NOSONAR here\n  //noinspection unchecked\n  void f() {\n    // noinspection A, B\n    int x = 1; /* NOSONAR */\n    //noinspection\tTabbed\n    /*\n     * NOSONAR\n     */\n  }\n}\n",
        &[
            r#"1 lint "NOSONAR" "// NOSONAR""#,
            r#"3 lint "NOSONAR" "// NOSONAR""#,
            r#"4 lint "NOSONAR" "// why NOSONAR here""#,
            r#"5 lint "unchecked" "//noinspection unchecked""#,
            r#"7 lint "A, B" "// noinspection A, B""#,
            r#"8 lint "NOSONAR" "/* NOSONAR */""#,
            r#"9 lint "Tabbed" "//noinspection\tTabbed""#,
            r#"10 lint "NOSONAR" "/*\n     * NOSONAR\n     */""#,
        ],
    );
}

#[test]
fn java_reads_no_suppression_from_a_near_miss_comment_or_a_string() {
    check(
        "src/M.java",
        "\
// nosonar
// NO SONAR
//noinspection
// noinspectionX
// x noinspection Y
/* noinspection X */
class A {
  String s = \"// NOSONAR\";
  String t = \"//noinspection X\";
}
",
        &[],
    );
}

/// A file annotation is a node kind of its own, and a qualified name's first identifier
/// is its package: both are read by the last segment of the annotation's type.
#[test]
fn kotlin_reads_qualified_and_file_level_suppress_annotations() {
    check(
        "src/m.kt",
        "\
@file:Suppress(\"FILE\")
@file:kotlin.Suppress(\"Q\")
package p
@kotlin.Suppress(\"UNCHECKED_CAST\")
class A {
  @kotlin.Suppress
  val field = 1
  @my.pkg.Suppress(\"other\")
  fun f() {
    @kotlin.Suppress(\"A\", \"B\")
    val x = 1
  }
  @get:kotlin.Suppress(\"G\")
  val p = 1
  @android.annotation.SuppressLint(\"NewApi\")
  fun g() {}
}
",
        &[
            r#"1 lint "FILE" "@file:Suppress(\"FILE\")""#,
            r#"2 lint "Q" "@file:kotlin.Suppress(\"Q\")""#,
            r#"4 lint "UNCHECKED_CAST" "@kotlin.Suppress(\"UNCHECKED_CAST\")""#,
            r#"6 lint "all" "@kotlin.Suppress""#,
            r#"8 lint "other" "@my.pkg.Suppress(\"other\")""#,
            r#"10 lint "A\", \"B" "@kotlin.Suppress(\"A\", \"B\")""#,
            r#"13 lint "G" "@get:kotlin.Suppress(\"G\")""#,
            r#"15 lint "NewApi" "@android.annotation.SuppressLint(\"NewApi\")""#,
        ],
    );
}

#[test]
fn kotlin_reads_no_suppression_from_an_annotation_whose_last_segment_is_another_name() {
    check(
        "src/m.kt",
        "\
@file:JvmName(\"n\")
@file:my.NotSuppress(\"x\")
package p
@my.NotSuppress(\"x\")
@Suppressed
@kotlin.Deprecated(\"x\")
@Suppress.Inner(\"x\")
class A {
  val s = \"@file:Suppress\"
  @Outer(kotlin.Suppress(\"inner\"))
  fun f() {}
}
",
        &[],
    );
}

#[test]
fn kotlin_reads_nosonar_comments() {
    check(
        "src/m.kt",
        "\
// NOSONAR
package p
class A { // NOSONAR
  val field = 1 //NOSONAR reason
  fun f() {
    val x = 1 /* NOSONAR */
    val y = 2 // nosonar
  }
}
",
        &[
            r#"1 lint "NOSONAR" "// NOSONAR""#,
            r#"3 lint "NOSONAR" "// NOSONAR""#,
            r#"4 lint "NOSONAR" "//NOSONAR reason""#,
            r#"6 lint "NOSONAR" "/* NOSONAR */""#,
            r#"7 lint "NOSONAR" "// nosonar""#,
        ],
    );
}

#[test]
fn kotlin_reads_no_suppression_from_a_near_miss_comment_or_a_string() {
    check(
        "src/m.kt",
        "\
// see NOSONAR
// NO SONAR
// NOSONA
//noinspection X
// ktlint-disable
class A {
  val s = \"// NOSONAR\"
}
",
        &[],
    );
}

#[test]
fn scala_reads_qualified_annotations() {
    check(
        "src/m.scala",
        "\
@scala.annotation.nowarn
class A {
  @scala.annotation.nowarn(\"cat=deprecation\")
  val field = 1
  @annotation.nowarn(\"msg=x\")
  def f(p: Int): Unit = {
    @scala.annotation.nowarn val z = p
  }
  @my.pkg.nowarn def g(): Unit = ()
  @java.lang.SuppressWarnings(Array(\"x\")) def h(): Unit = ()
}
",
        &[
            r#"1 lint "all" "@scala.annotation.nowarn""#,
            r#"3 lint "cat=deprecation" "@scala.annotation.nowarn(\"cat=deprecation\")""#,
            r#"5 lint "msg=x" "@annotation.nowarn(\"msg=x\")""#,
            r#"7 lint "all" "@scala.annotation.nowarn""#,
            r#"9 lint "all" "@my.pkg.nowarn""#,
            r#"10 lint "Array(\"x" "@java.lang.SuppressWarnings(Array(\"x\"))""#,
        ],
    );
}

#[test]
fn scala_reads_no_suppression_from_an_annotation_whose_last_segment_is_another_name() {
    check(
        "src/m.scala",
        "\
@my.notnowarn
@nowarned
@scala.deprecated
@nowarn.Inner
class A {
  val s = \"@scala.annotation.nowarn\"
}
",
        &[],
    );
}

/// RuboCop reads each space of `# rubocop : <mode>` as any run of blanks.
#[test]
fn ruby_reads_rubocop_comments_spaced_as_rubocop_accepts() {
    check(
        "src/m.rb",
        "#rubocop:disable Metrics/AbcSize\nclass A\n  #  rubocop:disable Style/X\n  def f\n    x = 1 #rubocop:todo A, B  \n    # rubocop : disable C\n    #\trubocop: disable D\n  end\nend\n",
        &[
            r##"1 lint "Metrics/AbcSize" "#rubocop:disable Metrics/AbcSize""##,
            r##"3 lint "Style/X" "#  rubocop:disable Style/X""##,
            r##"5 lint "A, B" "#rubocop:todo A, B""##,
            r##"6 lint "C" "# rubocop : disable C""##,
            r##"7 lint "D" "#\trubocop: disable D""##,
        ],
    );
}

#[test]
fn ruby_reads_no_suppression_from_a_near_miss_comment_or_a_string() {
    check(
        "src/m.rb",
        "\
#rubocop:enable A
# rubo cop:disable A
# rubocopdisable A
# rubocop;disable A
## rubocop:disable A
# note # rubocop:disable A
s = \"#rubocop:disable A\"
=begin
#rubocop:disable A
=end
",
        &[],
    );
}

/// `# type:ignore` as the tokenizer reads it, `noqa` in any letter-case and after other
/// comment text, and `disable-next`. A comment that carries two tools' suppressions is
/// one site for each.
#[test]
fn python_reads_the_spellings_its_tools_accept() {
    check(
        "src/m.py",
        "\
# NOQA
# pylint: disable-next=unused-import
import os  # NoQA: F401
class A:
    # type:ignore
    def test_f(self):  # note # noqa
        x = os.sep  # type:ignore[attr-defined]
        y = x  # type: ignore # noqa: E501
        z = y  #type:  ignore
        assert z  # pylint: disable=a # NOQA
        w = 1  # note #noqa:E1 # more
        v = 2  # noqa # noqa: E2
",
        &[
            r##"1 lint "all" "# NOQA""##,
            r##"2 lint "unused-import" "# pylint: disable-next=unused-import""##,
            r##"3 lint "F401" "# NoQA: F401""##,
            r##"5 type "mypy" "# type:ignore""##,
            r##"6 lint "all" "# noqa""##,
            r##"7 type "mypy" "# type:ignore[attr-defined]""##,
            r##"8 type "mypy" "# type: ignore # noqa: E501""##,
            r##"8 lint "E501" "# noqa: E501""##,
            r##"9 type "mypy" "#type:  ignore""##,
            r##"10 lint "a # NOQA" "# pylint: disable=a # NOQA""##,
            r##"10 lint "all" "# NOQA""##,
            r##"11 lint "E1 # more" "#noqa:E1 # more""##,
            r##"12 lint "all" "# noqa # noqa: E2""##,
        ],
    );
}

#[test]
fn python_reads_no_suppression_from_a_near_miss_comment_or_a_string() {
    check(
        "src/m.py",
        "\
# note noqa
# note # no qa
# typ:ignore
# type;ignore
# note # type: ignore
# pylint: disable-nxt=x
# pylint: enable-next=x
# RUFF: NOQA
s = \"# note # noqa\"
t = '''# type:ignore'''
u = \"# NOQA\"
",
        &[],
    );
}
