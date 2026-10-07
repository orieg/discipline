//! Expected exceptions through the real binary (#594): the standard class tables and the
//! qualified names they apply to, a project class moved to a base declared in its file,
//! a dropped site replaced by an assertion on the same call, the wording of two kept
//! findings, and the forms each language's extractor reads. A `*_is_reported` case is a
//! widening that must be reported once; a `*_is_silent` case is a control that must stay
//! unreported; a `*_is_not_vacuous` case is a new test whose only check is the form.

mod common;
use common::{Repo, Run};

const GATE: &str = "assertion-reduction";
const WIDENED: &str = "Expected Exception Or Panic Widened";
const VACUOUS: &str = "Vacuous Test Added";

#[derive(Clone, Copy)]
enum Lang {
    Python,
    Unittest,
    Java,
    Js,
    Ts,
    CSharp,
    MsTest,
    Php,
    Kotlin,
    Rspec,
    Minitest,
    Cpp,
}

fn indent(body: &str, by: &str) -> String {
    body.lines()
        .map(|l| format!("{by}{l}\n"))
        .collect::<String>()
}

/// The test file of `lang`: `prelude` (imports and declarations) above one test with `body`.
fn source(lang: Lang, prelude: &str, body: &str) -> (&'static str, String) {
    match lang {
        Lang::Python => (
            "tests/test_sut.py",
            format!(
                "import pytest\n{prelude}\n\ndef test_rejects():\n{}",
                indent(body, "    ")
            ),
        ),
        Lang::Unittest => (
            "tests/test_sut.py",
            format!(
                "import unittest\n{prelude}\n\nclass TestSut(unittest.TestCase):\n    def test_rejects(self):\n{}",
                indent(body, "        ")
            ),
        ),
        Lang::Java => (
            "src/test/java/SutTest.java",
            format!(
                "import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\nimport static org.assertj.core.api.Assertions.*;\n\nclass SutTest {{\n{}    @Test\n    void rejects() {{\n{}    }}\n}}\n",
                indent(prelude, "    "),
                indent(body, "        ")
            ),
        ),
        Lang::Js => (
            "tests/sut.test.js",
            format!(
                "{prelude}\ntest(\"rejects\", () => {{\n{}}});\n",
                indent(body, "  ")
            ),
        ),
        Lang::Ts => (
            "tests/sut.test.ts",
            format!(
                "{prelude}\ntest(\"rejects\", () => {{\n{}}});\n",
                indent(body, "  ")
            ),
        ),
        Lang::CSharp => (
            "tests/SutTests.cs",
            format!(
                "using Xunit;\n{prelude}\npublic class SutTests {{\n    [Fact]\n    public async Task Rejects() {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::MsTest => (
            "tests/SutTests.cs",
            format!(
                "using Microsoft.VisualStudio.TestTools.UnitTesting;\n{prelude}\n[TestClass]\npublic class SutTests {{\n    [TestMethod]\n    public async Task Rejects() {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::Php => (
            "tests/SutTest.php",
            format!(
                "<?php\nuse PHPUnit\\Framework\\TestCase;\n{prelude}\nclass SutTest extends TestCase {{\n    public function testRejects(): void {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::Kotlin => (
            "src/test/kotlin/SutTest.kt",
            format!(
                "import org.junit.jupiter.api.Test\nimport kotlin.test.*\n{prelude}\nclass SutTest {{\n    @Test\n    fun rejects() {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::Rspec => (
            "spec/sut_spec.rb",
            format!(
                "{prelude}\nRSpec.describe Sut do\n  it \"rejects\" do\n{}  end\nend\n",
                indent(body, "    ")
            ),
        ),
        Lang::Minitest => (
            "test/sut_test.rb",
            format!(
                "require \"minitest/autorun\"\n{prelude}\nclass SutTest < Minitest::Test\n  def test_rejects\n{}  end\nend\n",
                indent(body, "    ")
            ),
        ),
        Lang::Cpp => (
            "tests/sut_test.cc",
            format!(
                "#include <gtest/gtest.h>\n#include <stdexcept>\n{prelude}\nTEST(Sut, Rejects) {{\n{}}}\n",
                indent(body, "  ")
            ),
        ),
    }
}

/// Commits `base` on the base side and `head` on the work branch, then checks. Each side
/// is `(prelude, body)`.
fn change(lang: Lang, base: (&str, &str), head: (&str, &str)) -> Run {
    let repo = Repo::new();
    let (path, base_src) = source(lang, base.0, base.1);
    let (_, head_src) = source(lang, head.0, head.1);
    repo.commit_base(path, &base_src, "test: base");
    repo.write(path, &head_src);
    repo.commit("test: change the test");
    repo.check(&[])
}

/// Adds one new test with `body` and checks.
fn added(lang: Lang, prelude: &str, body: &str) -> Run {
    let repo = Repo::new();
    let (path, src) = source(lang, prelude, body);
    repo.write(path, &src);
    repo.commit("test: add a test");
    repo.check(&[])
}

fn widened_messages(run: &Run) -> Vec<String> {
    run.violations(GATE)
        .iter()
        .filter(|v| v["title"] == WIDENED)
        .map(|v| {
            assert_eq!(v["code"], "assertion-reduction/expected-exception-widened");
            v["message"].as_str().unwrap_or("").to_string()
        })
        .collect()
}

/// `reported!(name, Lang, [prelude,] base, head [, needle])`: exactly one finding, whose
/// message holds `needle`. The prelude is written `[prelude]`, or
/// `[base prelude => head prelude]` when the change edits it too.
macro_rules! reported {
    ($name:ident, $lang:ident, [$bp:expr => $hp:expr], $base:expr, $head:expr, $needle:expr) => {
        #[test]
        fn $name() {
            let run = change(Lang::$lang, ($bp, $base), ($hp, $head));
            let got = widened_messages(&run);
            assert_eq!(got.len(), 1, "{got:?}\n{}{}", run.stdout, run.stderr);
            assert!(got[0].contains($needle), "{got:?}");
            assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
        }
    };
    ($name:ident, $lang:ident, [$prelude:expr], $base:expr, $head:expr) => {
        reported!($name, $lang, [$prelude => $prelude], $base, $head, "");
    };
    ($name:ident, $lang:ident, [$prelude:expr], $base:expr, $head:expr, $needle:expr) => {
        reported!($name, $lang, [$prelude => $prelude], $base, $head, $needle);
    };
    ($name:ident, $lang:ident, $base:expr, $head:expr) => {
        reported!($name, $lang, ["" => ""], $base, $head, "");
    };
    ($name:ident, $lang:ident, $base:expr, $head:expr, $needle:expr) => {
        reported!($name, $lang, ["" => ""], $base, $head, $needle);
    };
}

macro_rules! silent {
    ($name:ident, $lang:ident, [$bp:expr => $hp:expr], $base:expr, $head:expr) => {
        #[test]
        fn $name() {
            let run = change(Lang::$lang, ($bp, $base), ($hp, $head));
            let got = widened_messages(&run);
            assert!(got.is_empty(), "{got:?}\n{}{}", run.stdout, run.stderr);
        }
    };
    ($name:ident, $lang:ident, [$prelude:expr], $base:expr, $head:expr) => {
        silent!($name, $lang, [$prelude => $prelude], $base, $head);
    };
    ($name:ident, $lang:ident, $base:expr, $head:expr) => {
        silent!($name, $lang, ["" => ""], $base, $head);
    };
}

/// A new test whose only check is `body` is not reported by `vacuous-tests`.
macro_rules! not_vacuous {
    ($name:ident, $lang:ident, [$prelude:expr], $body:expr) => {
        #[test]
        fn $name() {
            let run = added(Lang::$lang, $prelude, $body);
            assert!(
                run.titles("vacuous-tests").is_empty(),
                "{}{}",
                run.stdout,
                run.stderr
            );
        }
    };
    ($name:ident, $lang:ident, $body:expr) => {
        not_vacuous!($name, $lang, [""], $body);
    };
}

/// Control for `not_vacuous!`: a new test with `body` alone is reported.
macro_rules! vacuous {
    ($name:ident, $lang:ident, [$prelude:expr], $body:expr) => {
        #[test]
        fn $name() {
            let run = added(Lang::$lang, $prelude, $body);
            assert_eq!(
                run.titles("vacuous-tests"),
                vec![VACUOUS],
                "{}{}",
                run.stdout,
                run.stderr
            );
        }
    };
}

// ---------------------------------------------------------------------------
// Item 1: the standard tables, and the names they apply to.
// ---------------------------------------------------------------------------

reported!(
    py_finalization_error_to_runtime_error_is_reported,
    Python,
    "with pytest.raises(PythonFinalizationError):\n    f(-1)",
    "with pytest.raises(RuntimeError):\n    f(-1)",
    "from `PythonFinalizationError` to `RuntimeError`"
);

reported!(
    java_malformed_input_to_character_coding_is_reported,
    Java,
    "assertThrows(MalformedInputException.class, () -> f(-1));",
    "assertThrows(CharacterCodingException.class, () -> f(-1));",
    "from `MalformedInputException` to `CharacterCodingException`"
);

reported!(
    java_rejected_execution_to_runtime_exception_is_reported,
    Java,
    "assertThrows(RejectedExecutionException.class, () -> f(-1));",
    "assertThrows(RuntimeException.class, () -> f(-1));"
);

// A class qualified by a name that is not the standard library's is a project class.
silent!(
    py_project_class_sharing_a_standard_name_is_silent,
    Python,
    "with pytest.raises(errors.TimeoutError):\n    f(-1)",
    "with pytest.raises(OSError):\n    f(-1)"
);

// Controls: unqualified, and qualified by the standard library.
reported!(
    py_unqualified_standard_name_is_reported,
    Python,
    "with pytest.raises(TimeoutError):\n    f(-1)",
    "with pytest.raises(OSError):\n    f(-1)"
);

reported!(
    py_builtins_qualified_standard_name_is_reported,
    Python,
    "with pytest.raises(builtins.TimeoutError):\n    f(-1)",
    "with pytest.raises(builtins.OSError):\n    f(-1)"
);

// Control: a project class moved to a general name is still a widening.
reported!(
    py_project_class_moved_to_exception_is_reported,
    Python,
    "with pytest.raises(errors.TimeoutError):\n    f(-1)",
    "with pytest.raises(Exception):\n    f(-1)"
);

silent!(
    java_project_class_sharing_a_standard_name_is_silent,
    Java,
    "assertThrows(my.pkg.FileNotFoundException.class, () -> f(-1));",
    "assertThrows(IOException.class, () -> f(-1));"
);

reported!(
    java_standard_qualified_name_is_reported,
    Java,
    "assertThrows(java.io.FileNotFoundException.class, () -> f(-1));",
    "assertThrows(java.io.IOException.class, () -> f(-1));"
);

silent!(
    csharp_project_class_sharing_a_standard_name_is_silent,
    CSharp,
    "Assert.ThrowsAny<My.Storage.FileNotFoundException>(() => sut.Run());",
    "Assert.ThrowsAny<IOException>(() => sut.Run());"
);

reported!(
    csharp_standard_qualified_name_is_reported,
    CSharp,
    "Assert.ThrowsAny<System.IO.FileNotFoundException>(() => sut.Run());",
    "Assert.ThrowsAny<System.IO.IOException>(() => sut.Run());"
);

silent!(
    php_project_class_sharing_a_standard_name_is_silent,
    Php,
    "$this->expectException(\\App\\InvalidArgumentException::class);\nf(-1);",
    "$this->expectException(\\LogicException::class);\nf(-1);"
);

reported!(
    php_root_qualified_standard_name_is_reported,
    Php,
    "$this->expectException(\\InvalidArgumentException::class);\nf(-1);",
    "$this->expectException(\\LogicException::class);\nf(-1);"
);

// ---------------------------------------------------------------------------
// Item 2: a project class and the base its own file declares.
// ---------------------------------------------------------------------------

const PY_ERRORS: &str =
    "\n\nclass AppError(RuntimeError):\n    pass\n\n\nclass OrderError(AppError):\n    pass\n\n\nclass PaymentError(AppError):\n    pass\n\n\nclass LineError(OrderError):\n    pass\n";

reported!(
    py_project_class_moved_to_its_same_file_base_is_reported,
    Python,
    [PY_ERRORS],
    "with pytest.raises(OrderError):\n    f(-1)",
    "with pytest.raises(AppError):\n    f(-1)",
    "from `OrderError` to `AppError`"
);

reported!(
    py_project_class_moved_to_a_same_file_grandparent_is_reported,
    Python,
    [PY_ERRORS],
    "with pytest.raises(LineError):\n    f(-1)",
    "with pytest.raises(AppError):\n    f(-1)"
);

// The file's hierarchy is chained onto the standard one.
reported!(
    py_project_class_moved_past_its_standard_base_is_reported,
    Python,
    ["\n\nclass MissingKey(KeyError):\n    pass\n"],
    "with pytest.raises(MissingKey):\n    f(-1)",
    "with pytest.raises(LookupError):\n    f(-1)",
    "from `MissingKey` to `LookupError`"
);

silent!(
    py_same_file_sibling_replacement_is_silent,
    Python,
    [PY_ERRORS],
    "with pytest.raises(OrderError):\n    f(-1)",
    "with pytest.raises(PaymentError):\n    f(-1)"
);

silent!(
    py_narrowing_to_a_same_file_subclass_is_silent,
    Python,
    [PY_ERRORS],
    "with pytest.raises(AppError):\n    f(-1)",
    "with pytest.raises(OrderError):\n    f(-1)"
);

// A class the file imports is declared nowhere the gate reads.
silent!(
    py_class_declared_in_another_file_is_silent,
    Python,
    ["from app.errors import AppError, OrderError\n"],
    "with pytest.raises(OrderError):\n    f(-1)",
    "with pytest.raises(AppError):\n    f(-1)"
);

// The change re-parents the class: the base side still says `AppError` is above it.
reported!(
    py_class_reparented_by_the_change_is_reported,
    Python,
    [PY_ERRORS => "\n\nclass AppError(RuntimeError):\n    pass\n\n\nclass OrderError(RuntimeError):\n    pass\n"],
    "with pytest.raises(OrderError):\n    f(-1)",
    "with pytest.raises(AppError):\n    f(-1)",
    "from `OrderError` to `AppError`"
);

// A file that declares a class under a standard name: its own declaration is what counts.
silent!(
    py_same_file_class_under_a_standard_name_is_silent,
    Python,
    ["\n\nclass TimeoutError(Exception):\n    pass\n"],
    "with pytest.raises(TimeoutError):\n    f(-1)",
    "with pytest.raises(OSError):\n    f(-1)"
);

const JAVA_ERRORS: &str = "static class AppException extends RuntimeException {}\nstatic class OrderException extends AppException {}\nstatic class PaymentException extends AppException {}\n";

reported!(
    java_project_class_moved_to_its_same_file_base_is_reported,
    Java,
    [JAVA_ERRORS],
    "assertThrows(OrderException.class, () -> f(-1));",
    "assertThrows(AppException.class, () -> f(-1));",
    "from `OrderException` to `AppException`"
);

silent!(
    java_same_file_sibling_replacement_is_silent,
    Java,
    [JAVA_ERRORS],
    "assertThrows(OrderException.class, () -> f(-1));",
    "assertThrows(PaymentException.class, () -> f(-1));"
);

const CSHARP_ERRORS: &str =
    "public class AppException : System.Exception {}\npublic class OrderException : AppException, IOrderFault {}\n";

reported!(
    csharp_project_class_moved_to_its_same_file_base_is_reported,
    CSharp,
    [CSHARP_ERRORS],
    "Assert.ThrowsAny<OrderException>(() => sut.Run());",
    "Assert.ThrowsAny<AppException>(() => sut.Run());",
    "from `OrderException` to `AppException`"
);

// An exact assertion accepts no subclass: the move is a substitution.
silent!(
    csharp_exact_assertion_moved_to_a_same_file_base_is_silent,
    CSharp,
    [CSHARP_ERRORS],
    "Assert.Throws<OrderException>(() => sut.Run());",
    "Assert.Throws<AppException>(() => sut.Run());"
);

const JS_ERRORS: &str = "class AppError extends Error {}\nclass OrderError extends AppError {}\nclass PaymentError extends AppError {}\n";

reported!(
    js_project_class_moved_to_its_same_file_base_is_reported,
    Js,
    [JS_ERRORS],
    "expect(() => f(-1)).toThrow(OrderError);",
    "expect(() => f(-1)).toThrow(AppError);",
    "from `OrderError` to `AppError`"
);

reported!(
    ts_project_class_moved_to_its_same_file_base_is_reported,
    Ts,
    [JS_ERRORS],
    "expect(() => f(-1)).toThrow(OrderError);",
    "expect(() => f(-1)).toThrow(AppError);"
);

silent!(
    js_same_file_sibling_replacement_is_silent,
    Js,
    [JS_ERRORS],
    "expect(() => f(-1)).toThrow(OrderError);",
    "expect(() => f(-1)).toThrow(PaymentError);"
);

const PHP_ERRORS: &str =
    "class AppException extends \\RuntimeException {}\nclass OrderException extends AppException {}\nclass PaymentException extends AppException {}\n";

reported!(
    php_project_class_moved_to_its_same_file_base_is_reported,
    Php,
    [PHP_ERRORS],
    "$this->expectException(OrderException::class);\nf(-1);",
    "$this->expectException(AppException::class);\nf(-1);",
    "from `OrderException` to `AppException`"
);

silent!(
    php_same_file_sibling_replacement_is_silent,
    Php,
    [PHP_ERRORS],
    "$this->expectException(OrderException::class);\nf(-1);",
    "$this->expectException(PaymentException::class);\nf(-1);"
);

// ---------------------------------------------------------------------------
// Item 3: a site dropped while the assertion count holds.
// ---------------------------------------------------------------------------

const PY_DROPPED_BASE: &str = "with pytest.raises(ValueError):\n    f(-1)\nassert g() == 1";
const PY_DROPPED_HEAD: &str = "assert f(1) == 0\nassert g() == 1";

// The behaviour is now asserted to succeed, on the same call.
silent!(
    py_site_replaced_by_an_assertion_on_the_same_call_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nassert g() == 1",
    "assert f(-1) == 0\nassert g() == 1"
);

silent!(
    py_site_replaced_by_an_assertion_on_the_same_call_respaced_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1, key = \"a b\")\nassert g() == 1",
    "assert f( -1, key=\"a b\" ) == 0\nassert g() == 1"
);

// Controls: other arguments, another callee, a string argument that differs in its
// spacing, and a call the base side already asserted on.
reported!(
    py_site_replaced_by_an_assertion_on_other_arguments_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nassert g() == 1",
    "assert f(1) == 0\nassert g() == 1"
);

reported!(
    py_site_replaced_by_an_assertion_on_another_callee_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nassert g() == 1",
    "assert h(-1) == 0\nassert g() == 1"
);

reported!(
    py_site_replaced_by_an_assertion_on_another_string_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(\"a b\")\nassert g() == 1",
    "assert f(\"ab\") == 0\nassert g() == 1"
);

reported!(
    py_site_dropped_beside_an_assertion_the_base_had_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nassert f(-1) == 0",
    "assert f(-1) == 0\nassert g() == 1"
);

// A block that does more than the one call is not the same behaviour.
reported!(
    py_site_with_two_statements_replaced_by_an_assertion_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    setup()\n    f(-1)\nassert g() == 1",
    "assert f(-1) == 0\nassert g() == 1"
);

/// The finding says which expectation is gone and how a replaced one is lifted.
#[test]
fn py_dropped_site_names_the_expectation_and_the_directive_is_reported() {
    let run = change(Lang::Python, ("", PY_DROPPED_BASE), ("", PY_DROPPED_HEAD));
    let got = widened_messages(&run);
    assert_eq!(got.len(), 1, "{got:?}\n{}{}", run.stdout, run.stderr);
    assert!(
        got[0].contains("expected exception `ValueError` is no longer checked"),
        "{got:?}"
    );
    assert!(
        got[0].contains("lifted with `allow-assertion-drop: test_rejects <reason>`"),
        "{got:?}"
    );
}

/// Control: the directive the message names lifts the finding.
#[test]
fn py_dropped_site_is_lifted_by_the_directive_it_names_is_silent() {
    let repo = Repo::new();
    let (path, base) = source(Lang::Python, "", PY_DROPPED_BASE);
    let (_, head) = source(Lang::Python, "", PY_DROPPED_HEAD);
    repo.commit_base(path, &base, "test: base");
    repo.write(path, &head);
    repo.commit("test: f no longer raises\n\nallow-assertion-drop: test_rejects f returns zero for a negative input now");
    let run = repo.check(&[]);
    assert!(
        widened_messages(&run).is_empty(),
        "{}{}",
        run.stdout,
        run.stderr
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
}

silent!(
    js_site_replaced_by_an_assertion_on_the_same_call_is_silent,
    Js,
    "expect(() => f(-1)).toThrow(RangeError);\nexpect(g()).toBe(1);",
    "expect(f(-1)).toBe(0);\nexpect(g()).toBe(1);"
);

reported!(
    js_site_replaced_by_an_assertion_on_other_arguments_is_reported,
    Js,
    "expect(() => f(-1)).toThrow(RangeError);\nexpect(g()).toBe(1);",
    "expect(f(1)).toBe(0);\nexpect(g()).toBe(1);"
);

// ---------------------------------------------------------------------------
// Item 4: two rewritten sites that trade strictness. Every base expectation still has
// a head expectation of its own that accepts no more; which call each guards is not
// compared.
// ---------------------------------------------------------------------------

silent!(
    py_two_rewritten_sites_trading_strictness_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nwith pytest.raises(Exception):\n    g()",
    "with pytest.raises(Exception):\n    f(-2)\nwith pytest.raises(ValueError):\n    g(2)"
);

// Control: the same rewrite with one site widened and nothing given back.
reported!(
    py_two_rewritten_sites_with_one_widened_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nwith pytest.raises(Exception):\n    g()",
    "with pytest.raises(Exception):\n    f(-2)\nwith pytest.raises(Exception):\n    g(2)"
);

// ---------------------------------------------------------------------------
// Item 6: what the message says when a matcher arrives beside a wider or missing type.
// ---------------------------------------------------------------------------

reported!(
    py_type_widened_with_a_matcher_gained_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)",
    "with pytest.raises(Exception, match=\"neg\"):\n    f(-1)",
    "expected exception type widened from `ValueError` to `Exception`; the matcher added beside it does not narrow the type"
);

reported!(
    js_type_replaced_by_a_message_is_reported,
    Js,
    "expect(() => f(-1)).toThrow(TypeError);",
    "expect(() => f(-1)).toThrow(\"negative\");",
    "expected exception type `TypeError` was removed; a message matcher replaced it"
);

// Control: the type removed with nothing in its place keeps its wording.
reported!(
    js_type_removed_is_reported,
    Js,
    "expect(() => f(-1)).toThrow(TypeError);",
    "expect(() => f(-1)).toThrow();",
    "expected exception type was removed"
);

// ---------------------------------------------------------------------------
// Item 7, Python: `raises` imported bare from pytest, and `assertWarns`.
// ---------------------------------------------------------------------------

const PY_BARE: &str = "from pytest import raises\n";

reported!(
    py_bare_raises_widened_is_reported,
    Python,
    [PY_BARE],
    "with raises(ValueError, match=\"neg\"):\n    f(-1)",
    "with raises(Exception):\n    f(-1)",
    "from `ValueError` to `Exception`"
);

reported!(
    py_bare_raises_under_an_alias_widened_is_reported,
    Python,
    ["from pytest import raises as throws\n"],
    "with throws(KeyError):\n    f(-1)",
    "with throws(LookupError):\n    f(-1)"
);

reported!(
    py_bare_raises_call_form_widened_is_reported,
    Python,
    [PY_BARE],
    "raises(KeyError, f, -1)",
    "raises(LookupError, f, -1)"
);

silent!(
    py_bare_raises_narrowed_is_silent,
    Python,
    [PY_BARE],
    "with raises(LookupError):\n    f(-1)",
    "with raises(KeyError):\n    f(-1)"
);

// Control: a `raises` the file does not import from pytest is not the pytest one.
silent!(
    py_bare_raises_from_another_module_is_silent,
    Python,
    ["from mylib import raises\n"],
    "with raises(ValueError):\n    f(-1)",
    "with raises(Exception):\n    f(-1)"
);

not_vacuous!(
    py_bare_raises_only_test_is_not_vacuous,
    Python,
    [PY_BARE],
    "with raises(ValueError):\n    f(-1)"
);

vacuous!(
    py_bare_raises_from_another_module_only_test_is_vacuous,
    Python,
    ["from mylib import raises\n"],
    "with raises(ValueError):\n    f(-1)"
);

reported!(
    py_assert_warns_widened_is_reported,
    Unittest,
    "with self.assertWarns(DeprecationWarning):\n    f(-1)",
    "with self.assertWarns(Warning):\n    f(-1)",
    "from `DeprecationWarning` to `Warning`"
);

reported!(
    py_assert_warns_regex_pattern_dropped_is_reported,
    Unittest,
    "with self.assertWarnsRegex(DeprecationWarning, \"old api\"):\n    f(-1)",
    "with self.assertWarns(DeprecationWarning):\n    f(-1)",
    "matcher"
);

silent!(
    py_assert_warns_narrowed_is_silent,
    Unittest,
    "with self.assertWarns(Warning):\n    f(-1)",
    "with self.assertWarns(DeprecationWarning):\n    f(-2)"
);

not_vacuous!(
    py_assert_warns_only_test_is_not_vacuous,
    Unittest,
    "with self.assertWarns(DeprecationWarning):\n    f(-1)"
);

// Control: `assertRaisesRegex` was read already.
reported!(
    py_assert_raises_regex_widened_is_reported,
    Unittest,
    "with self.assertRaisesRegex(KeyError, \"neg\"):\n    f(-1)",
    "with self.assertRaisesRegex(LookupError, \"neg\"):\n    f(-1)"
);

// ---------------------------------------------------------------------------
// Item 7, Java: AssertJ.
// ---------------------------------------------------------------------------

reported!(
    java_assertj_thrown_by_widened_is_reported,
    Java,
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(NumberFormatException.class);",
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(IllegalArgumentException.class);",
    "from `NumberFormatException` to `IllegalArgumentException`"
);

reported!(
    java_assertj_thrown_by_type_dropped_is_reported,
    Java,
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(NumberFormatException.class);",
    "assertThatThrownBy(() -> f(-1));",
    "type was removed"
);

reported!(
    java_assertj_exact_relaxed_is_reported,
    Java,
    "assertThatThrownBy(() -> f(-1)).isExactlyInstanceOf(IllegalArgumentException.class);",
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(IllegalArgumentException.class);",
    "exact type"
);

reported!(
    java_assertj_message_dropped_is_reported,
    Java,
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(IllegalArgumentException.class).hasMessage(\"negative value\");",
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(IllegalArgumentException.class);",
    "matcher was removed"
);

reported!(
    java_assertj_message_made_a_part_of_itself_is_reported,
    Java,
    "assertThatThrownBy(() -> f(-1)).hasMessage(\"negative value\");",
    "assertThatThrownBy(() -> f(-1)).hasMessageContaining(\"value\");",
    "matches more messages"
);

reported!(
    java_assertj_whole_message_made_a_contained_one_is_reported,
    Java,
    "assertThatThrownBy(() -> f(-1)).hasMessage(\"negative value\");",
    "assertThatThrownBy(() -> f(-1)).hasMessageContaining(\"negative value\");",
    "matches more messages"
);

reported!(
    java_assertj_exception_of_type_widened_is_reported,
    Java,
    "assertThatExceptionOfType(NumberFormatException.class).isThrownBy(() -> f(-1)).withMessage(\"neg\");",
    "assertThatExceptionOfType(IllegalArgumentException.class).isThrownBy(() -> f(-1)).withMessage(\"neg\");",
    "from `NumberFormatException` to `IllegalArgumentException`"
);

reported!(
    java_assertj_thrown_by_replaced_by_does_not_throw_is_reported,
    Java,
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(IllegalArgumentException.class);",
    "assertThatCode(() -> f(-1)).doesNotThrowAnyException();",
    "no longer checked"
);

silent!(
    java_assertj_thrown_by_narrowed_is_silent,
    Java,
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(IllegalArgumentException.class);",
    "assertThatThrownBy(() -> f(-2)).isInstanceOf(NumberFormatException.class).hasMessageContaining(\"neg\");"
);

silent!(
    java_assertj_contained_message_made_whole_is_silent,
    Java,
    "assertThatThrownBy(() -> f(-1)).hasMessageContaining(\"value\");",
    "assertThatThrownBy(() -> f(-1)).hasMessage(\"negative value\");"
);

silent!(
    java_assertj_does_not_throw_unchanged_is_silent,
    Java,
    "assertThatCode(() -> f(1)).doesNotThrowAnyException();",
    "assertThatCode(() -> f(2)).doesNotThrowAnyException();"
);

not_vacuous!(
    java_assertj_thrown_by_only_test_is_not_vacuous,
    Java,
    "assertThatThrownBy(() -> f(-1)).isInstanceOf(IllegalArgumentException.class);"
);

not_vacuous!(
    java_assertj_exception_of_type_only_test_is_not_vacuous,
    Java,
    "assertThatExceptionOfType(IllegalArgumentException.class).isThrownBy(() -> f(-1));"
);

not_vacuous!(
    java_assertj_does_not_throw_only_test_is_not_vacuous,
    Java,
    "assertThatCode(() -> f(1)).doesNotThrowAnyException();"
);

// ---------------------------------------------------------------------------
// Item 7, JavaScript / TypeScript: Node's `assert` and Chai.
// ---------------------------------------------------------------------------

const NODE_ASSERT: &str = "const assert = require(\"node:assert\");\n";
const NODE_ASSERT_IMPORT: &str = "import assert from \"node:assert/strict\";\n";
const CHAI_EXPECT: &str = "const { expect } = require(\"chai\");\n";
const CHAI_ASSERT: &str = "import { assert } from \"chai\";\n";

reported!(
    js_node_assert_throws_widened_is_reported,
    Js,
    [NODE_ASSERT],
    "assert.throws(() => f(-1), RangeError);",
    "assert.throws(() => f(-1), Error);",
    "from `RangeError` to `Error`"
);

reported!(
    js_node_assert_throws_type_dropped_is_reported,
    Js,
    [NODE_ASSERT],
    "assert.throws(() => f(-1), RangeError);",
    "assert.throws(() => f(-1));",
    "type was removed"
);

reported!(
    js_node_assert_throws_pattern_dropped_is_reported,
    Js,
    [NODE_ASSERT],
    "assert.throws(() => f(-1), /negative/);",
    "assert.throws(() => f(-1));",
    "matcher was removed"
);

reported!(
    js_node_assert_throws_object_message_dropped_is_reported,
    Js,
    [NODE_ASSERT],
    "assert.throws(() => f(-1), { name: \"RangeError\", message: \"negative\" });",
    "assert.throws(() => f(-1), { name: \"RangeError\" });",
    "matcher was removed"
);

reported!(
    ts_node_assert_rejects_widened_is_reported,
    Ts,
    [NODE_ASSERT_IMPORT],
    "await assert.rejects(f(-1), RangeError);",
    "await assert.rejects(f(-1), Error);",
    "from `RangeError` to `Error`"
);

silent!(
    js_node_assert_throws_narrowed_is_silent,
    Js,
    [NODE_ASSERT],
    "assert.throws(() => f(-1), Error);",
    "assert.throws(() => f(-2), RangeError);"
);

// Node's third argument is the message of the assertion itself, not a constraint.
silent!(
    js_node_assert_throws_assertion_message_dropped_is_silent,
    Js,
    [NODE_ASSERT],
    "assert.throws(() => f(-1), RangeError, \"must reject a negative\");",
    "assert.throws(() => f(-1), RangeError);"
);

silent!(
    js_node_assert_does_not_throw_unchanged_is_silent,
    Js,
    [NODE_ASSERT],
    "assert.doesNotThrow(() => f(1));",
    "assert.doesNotThrow(() => f(2));"
);

not_vacuous!(
    js_node_assert_throws_only_test_is_not_vacuous,
    Js,
    [NODE_ASSERT],
    "assert.throws(() => f(-1), RangeError);"
);

not_vacuous!(
    js_node_assert_does_not_throw_only_test_is_not_vacuous,
    Js,
    [NODE_ASSERT],
    "assert.doesNotThrow(() => f(1));"
);

reported!(
    js_chai_expect_throw_widened_is_reported,
    Js,
    [CHAI_EXPECT],
    "expect(() => f(-1)).to.throw(RangeError, \"negative\");",
    "expect(() => f(-1)).to.throw(Error, \"negative\");",
    "from `RangeError` to `Error`"
);

reported!(
    js_chai_expect_throw_message_dropped_is_reported,
    Js,
    [CHAI_EXPECT],
    "expect(() => f(-1)).to.throw(RangeError, \"negative\");",
    "expect(() => f(-1)).to.throw(RangeError);",
    "matcher was removed"
);

reported!(
    js_chai_expect_not_throw_gaining_a_type_is_reported,
    Js,
    [CHAI_EXPECT],
    "expect(() => f(1)).to.not.throw();",
    "expect(() => f(1)).to.not.throw(TypeError);",
    "negated"
);

reported!(
    js_chai_assert_throws_message_dropped_is_reported,
    Js,
    [CHAI_ASSERT],
    "assert.throws(() => f(-1), RangeError, \"negative\");",
    "assert.throws(() => f(-1), RangeError);",
    "matcher was removed"
);

silent!(
    js_chai_expect_throw_narrowed_is_silent,
    Js,
    [CHAI_EXPECT],
    "expect(() => f(-1)).to.throw(Error);",
    "expect(() => f(-2)).to.throw(RangeError, \"negative\");"
);

not_vacuous!(
    js_chai_expect_throw_only_test_is_not_vacuous,
    Js,
    [CHAI_EXPECT],
    "expect(() => f(-1)).to.throw(RangeError);"
);

// Control: a stub told to throw configures a double and expects nothing.
silent!(
    js_stub_throws_is_not_an_expectation_is_silent,
    Js,
    "stub.throws(new RangeError(\"negative\"));\nexpect(g()).toBe(1);",
    "stub.throws(new Error());\nexpect(g()).toBe(1);"
);

// ---------------------------------------------------------------------------
// Item 7, C#: NUnit constraints, and a message after an awaited `ThrowAsync`.
// ---------------------------------------------------------------------------

reported!(
    csharp_nunit_instance_of_widened_is_reported,
    CSharp,
    "Assert.That(() => sut.Run(), Throws.InstanceOf<ArgumentNullException>());",
    "Assert.That(() => sut.Run(), Throws.InstanceOf<ArgumentException>());",
    "from `ArgumentNullException` to `ArgumentException`"
);

reported!(
    csharp_nunit_type_of_relaxed_is_reported,
    CSharp,
    "Assert.That(() => sut.Run(), Throws.TypeOf<ArgumentException>());",
    "Assert.That(() => sut.Run(), Throws.InstanceOf<ArgumentException>());",
    "exact type"
);

reported!(
    csharp_nunit_exception_type_of_dropped_is_reported,
    CSharp,
    "Assert.That(() => sut.Run(), Throws.Exception.TypeOf<ArgumentException>());",
    "Assert.That(() => sut.Run(), Throws.Exception);",
    "type was removed"
);

reported!(
    csharp_nunit_message_dropped_is_reported,
    CSharp,
    "Assert.That(() => sut.Run(), Throws.TypeOf<ArgumentException>().With.Message.EqualTo(\"negative\"));",
    "Assert.That(() => sut.Run(), Throws.TypeOf<ArgumentException>());",
    "matcher was removed"
);

reported!(
    csharp_nunit_named_exception_widened_is_reported,
    CSharp,
    "Assert.That(() => sut.Run(), Throws.ArgumentNullException);",
    "Assert.That(() => sut.Run(), Throws.Exception);"
);

silent!(
    csharp_nunit_unchanged_is_silent,
    CSharp,
    "Assert.That(() => sut.Run(1), Throws.TypeOf<ArgumentException>().With.Message.EqualTo(\"negative\"));",
    "Assert.That(() => sut.Run(2), Throws.TypeOf<ArgumentException>().With.Message.EqualTo(\"negative\"));"
);

silent!(
    csharp_nunit_instance_of_made_exact_is_silent,
    CSharp,
    "Assert.That(() => sut.Run(), Throws.InstanceOf<ArgumentException>());",
    "Assert.That(() => sut.Run(), Throws.TypeOf<ArgumentException>());"
);

silent!(
    csharp_nunit_throws_nothing_unchanged_is_silent,
    CSharp,
    "Assert.That(() => sut.Run(1), Throws.Nothing);",
    "Assert.That(() => sut.Run(2), Throws.Nothing);"
);

not_vacuous!(
    csharp_nunit_throws_only_test_is_not_vacuous,
    CSharp,
    "Assert.That(() => sut.Run(), Throws.TypeOf<ArgumentException>());"
);

reported!(
    csharp_fluent_message_after_an_awaited_throw_dropped_is_reported,
    CSharp,
    "(await act.Should().ThrowAsync<ArgumentException>()).WithMessage(\"*negative*\");",
    "await act.Should().ThrowAsync<ArgumentException>();",
    "matcher was removed"
);

silent!(
    csharp_fluent_message_after_an_awaited_throw_unchanged_is_silent,
    CSharp,
    "(await act.Should().ThrowAsync<ArgumentException>()).WithMessage(\"*negative*\");\nsut.Run(1);",
    "(await act.Should().ThrowAsync<ArgumentException>()).WithMessage(\"*negative*\");\nsut.Run(2);"
);

// In an MSTest file `Assert.Throws<T>` accepts subclasses (#626, #649): replacing
// `Assert.ThrowsExactly<T>` by it gives up the exact class.
reported!(
    csharp_mstest_throws_exactly_to_throws_is_reported,
    MsTest,
    "Assert.ThrowsExactly<ArgumentException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());",
    "no longer checked as the exact type"
);

// Control: an xUnit file, where `Assert.Throws<T>` is exact. `Assert.ThrowsException<T>`
// is exact too, so the one in place of the other keeps the exact class.
silent!(
    csharp_xunit_file_throws_exception_to_throws_is_silent,
    CSharp,
    "Assert.ThrowsException<ArgumentException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());"
);

// Its twin in an MSTest file, where `Assert.Throws<T>` accepts subclasses.
reported!(
    csharp_mstest_file_throws_exception_to_throws_is_reported,
    MsTest,
    "Assert.ThrowsException<ArgumentException>(() => sut.Run());",
    "Assert.Throws<ArgumentException>(() => sut.Run());",
    "no longer checked as the exact type"
);

// ---------------------------------------------------------------------------
// Item 7, PHP: a message pattern, and an expected exception object.
// ---------------------------------------------------------------------------

reported!(
    php_message_pattern_made_a_wildcard_is_reported,
    Php,
    "$this->expectException(\\DomainException::class);\n$this->expectExceptionMessageMatches('/negative \\d+/');\nf(-1);",
    "$this->expectException(\\DomainException::class);\n$this->expectExceptionMessageMatches('/.*/');\nf(-1);",
    "any message"
);

reported!(
    php_message_pattern_dropped_while_the_count_holds_is_reported,
    Php,
    "$this->expectException(\\DomainException::class);\n$this->expectExceptionMessageMatches('/negative \\d+/');\nf(-1);",
    "$this->expectException(\\DomainException::class);\n$this->assertSame(1, ready());\nf(-1);",
    "no longer checked"
);

silent!(
    php_message_pattern_replaced_by_another_pattern_is_silent,
    Php,
    "$this->expectExceptionMessageMatches('/negative \\d+/');\nf(-1);",
    "$this->expectExceptionMessageMatches('/^negative [0-9]+$/');\nf(-1);"
);

not_vacuous!(
    php_message_pattern_only_test_is_not_vacuous,
    Php,
    "$this->expectExceptionMessageMatches('/negative/');\nf(-1);"
);

reported!(
    php_exception_object_widened_is_reported,
    Php,
    "$this->expectExceptionObject(new \\InvalidArgumentException(\"negative\"));\nf(-1);",
    "$this->expectExceptionObject(new \\LogicException(\"negative\"));\nf(-1);",
    "from `InvalidArgumentException` to `LogicException`"
);

reported!(
    php_exception_object_message_dropped_is_reported,
    Php,
    "$this->expectExceptionObject(new \\DomainException(\"negative\"));\nf(-1);",
    "$this->expectExceptionObject(new \\DomainException());\nf(-1);",
    "no longer checked"
);

silent!(
    php_exception_object_unchanged_is_silent,
    Php,
    "$this->expectExceptionObject(new \\DomainException(\"negative\"));\nf(-1);",
    "$this->expectExceptionObject(new \\DomainException(\"negative\"));\nf(-2);"
);

not_vacuous!(
    php_exception_object_only_test_is_not_vacuous,
    Php,
    "$this->expectExceptionObject(new \\DomainException(\"negative\"));\nf(-1);"
);

// ---------------------------------------------------------------------------
// Item 7, Kotlin: JUnit, kotlin.test and Kotest.
// ---------------------------------------------------------------------------

reported!(
    kotlin_assert_throws_widened_is_reported,
    Kotlin,
    "assertThrows<NumberFormatException> { f(-1) }",
    "assertThrows<IllegalArgumentException> { f(-1) }",
    "from `NumberFormatException` to `IllegalArgumentException`"
);

reported!(
    kotlin_assert_fails_with_widened_is_reported,
    Kotlin,
    "assertFailsWith<NumberFormatException> { f(-1) }",
    "assertFailsWith<RuntimeException> { f(-2) }",
    "from `NumberFormatException` to `RuntimeException`"
);

reported!(
    kotlin_assert_fails_with_class_argument_widened_is_reported,
    Kotlin,
    "assertFailsWith(NumberFormatException::class) { f(-1) }",
    "assertFailsWith(IllegalArgumentException::class) { f(-1) }",
    "from `NumberFormatException` to `IllegalArgumentException`"
);

reported!(
    kotlin_assert_fails_with_type_dropped_is_reported,
    Kotlin,
    "assertFailsWith<NumberFormatException> { f(-1) }",
    "assertFails { f(-1) }",
    "type was removed"
);

reported!(
    kotlin_kotest_should_throw_widened_is_reported,
    Kotlin,
    "val e = shouldThrow<java.io.FileNotFoundException> { f(-1) }",
    "val e = shouldThrow<java.io.IOException> { f(-1) }",
    "from `FileNotFoundException` to `IOException`"
);

reported!(
    kotlin_kotest_should_throw_exactly_relaxed_is_reported,
    Kotlin,
    "shouldThrowExactly<IllegalArgumentException> { f(-1) }",
    "shouldThrow<IllegalArgumentException> { f(-1) }",
    "exact type"
);

const KOTLIN_ERRORS: &str =
    "open class AppException(m: String) : RuntimeException(m)\nclass OrderException(m: String) : AppException(m), Fault\nclass PaymentException(m: String) : AppException(m)\n";

reported!(
    kotlin_project_class_moved_to_its_same_file_base_is_reported,
    Kotlin,
    [KOTLIN_ERRORS],
    "assertFailsWith<OrderException> { f(-1) }",
    "assertFailsWith<AppException> { f(-1) }",
    "from `OrderException` to `AppException`"
);

silent!(
    kotlin_same_file_sibling_replacement_is_silent,
    Kotlin,
    [KOTLIN_ERRORS],
    "assertFailsWith<OrderException> { f(-1) }",
    "assertFailsWith<PaymentException> { f(-1) }"
);

silent!(
    kotlin_assert_throws_narrowed_is_silent,
    Kotlin,
    "assertThrows<IllegalArgumentException> { f(-1) }",
    "assertThrows<NumberFormatException> { f(-2) }"
);

silent!(
    kotlin_should_throw_made_exact_is_silent,
    Kotlin,
    "shouldThrow<IllegalArgumentException> { f(-1) }",
    "shouldThrowExactly<IllegalArgumentException> { f(-1) }"
);

silent!(
    kotlin_project_class_sharing_a_standard_name_is_silent,
    Kotlin,
    "assertFailsWith<my.pkg.FileNotFoundException> { f(-1) }",
    "assertFailsWith<IOException> { f(-1) }"
);

not_vacuous!(
    kotlin_assert_fails_with_only_test_is_not_vacuous,
    Kotlin,
    "assertFailsWith<IllegalArgumentException> { f(-1) }"
);

not_vacuous!(
    kotlin_should_throw_only_test_is_not_vacuous,
    Kotlin,
    "shouldThrow<IllegalArgumentException> { f(-1) }"
);

// ---------------------------------------------------------------------------
// Item 7, Ruby: RSpec and Minitest.
// ---------------------------------------------------------------------------

reported!(
    rspec_raise_error_widened_is_reported,
    Rspec,
    "expect { f(-1) }.to raise_error(KeyError)",
    "expect { f(-1) }.to raise_error(IndexError)",
    "from `KeyError` to `IndexError`"
);

reported!(
    rspec_raise_error_moved_to_standard_error_is_reported,
    Rspec,
    "expect { f(-1) }.to raise_error(ArgumentError, \"negative\")",
    "expect { f(-2) }.to raise_error(StandardError, \"negative\")",
    "from `ArgumentError` to `StandardError`"
);

reported!(
    rspec_raise_error_message_dropped_is_reported,
    Rspec,
    "expect { f(-1) }.to raise_error(ArgumentError, \"negative\")",
    "expect { f(-1) }.to raise_error(ArgumentError)",
    "matcher was removed"
);

reported!(
    rspec_raise_error_class_dropped_is_reported,
    Rspec,
    "expect { f(-1) }.to raise_error(ArgumentError)",
    "expect { f(-1) }.to raise_error",
    "type was removed"
);

reported!(
    rspec_with_message_dropped_is_reported,
    Rspec,
    "expect do\n  f(-1)\nend.to raise_error(ArgumentError).with_message(\"negative\")",
    "expect do\n  f(-1)\nend.to raise_error(ArgumentError)",
    "matcher was removed"
);

reported!(
    rspec_not_to_raise_error_gaining_a_class_is_reported,
    Rspec,
    "expect { f(1) }.not_to raise_error",
    "expect { f(1) }.not_to raise_error(ArgumentError)",
    "negated"
);

const RUBY_ERRORS: &str =
    "class AppError < StandardError; end\nclass OrderError < AppError; end\nclass PaymentError < AppError; end\n";

reported!(
    rspec_project_class_moved_to_its_same_file_base_is_reported,
    Rspec,
    [RUBY_ERRORS],
    "expect { f(-1) }.to raise_error(OrderError)",
    "expect { f(-1) }.to raise_error(AppError)",
    "from `OrderError` to `AppError`"
);

silent!(
    rspec_same_file_sibling_replacement_is_silent,
    Rspec,
    [RUBY_ERRORS],
    "expect { f(-1) }.to raise_error(OrderError)",
    "expect { f(-1) }.to raise_error(PaymentError)"
);

silent!(
    rspec_raise_error_narrowed_is_silent,
    Rspec,
    "expect { f(-1) }.to raise_error(IndexError)",
    "expect { f(-2) }.to raise_error(KeyError, \"negative\")"
);

silent!(
    rspec_project_class_sharing_a_standard_name_is_silent,
    Rspec,
    "expect { f(-1) }.to raise_error(Errors::KeyError)",
    "expect { f(-1) }.to raise_error(IndexError)"
);

not_vacuous!(
    rspec_raise_error_only_test_is_not_vacuous,
    Rspec,
    "expect { f(-1) }.to raise_error(ArgumentError)"
);

reported!(
    minitest_assert_raises_widened_is_reported,
    Minitest,
    "assert_raises(KeyError) { f(-1) }",
    "assert_raises(IndexError) { f(-1) }",
    "from `KeyError` to `IndexError`"
);

reported!(
    minitest_assert_raises_class_added_is_reported,
    Minitest,
    "assert_raises(KeyError) do\n  f(-1)\nend",
    "assert_raises(KeyError, TypeError) do\n  f(-1)\nend",
    "now also accepts `TypeError`"
);

silent!(
    minitest_assert_raises_narrowed_is_silent,
    Minitest,
    "assert_raises(IndexError) { f(-1) }",
    "assert_raises(KeyError) { f(-2) }"
);

not_vacuous!(
    minitest_assert_raises_only_test_is_not_vacuous,
    Minitest,
    "assert_raises(ArgumentError) { f(-1) }"
);

// ---------------------------------------------------------------------------
// Item 7, C++: googletest.
// ---------------------------------------------------------------------------

reported!(
    cpp_expect_throw_widened_is_reported,
    Cpp,
    "EXPECT_THROW(f(-1), std::invalid_argument);",
    "EXPECT_THROW(f(-1), std::logic_error);",
    "from `invalid_argument` to `logic_error`"
);

reported!(
    cpp_assert_throw_moved_to_std_exception_is_reported,
    Cpp,
    "ASSERT_THROW(f(-1), app::OrderError);",
    "ASSERT_THROW(f(-2), std::exception);",
    "from `OrderError` to `exception`"
);

reported!(
    cpp_expect_throw_replaced_by_any_throw_is_reported,
    Cpp,
    "EXPECT_THROW(f(-1), std::invalid_argument);",
    "EXPECT_ANY_THROW(f(-1));",
    "type was removed"
);

const CPP_ERRORS: &str =
    "class AppError : public std::runtime_error { using std::runtime_error::runtime_error; };\nclass OrderError : public AppError { using AppError::AppError; };\nstruct PaymentError : AppError { using AppError::AppError; };\n";

reported!(
    cpp_project_class_moved_to_its_same_file_base_is_reported,
    Cpp,
    [CPP_ERRORS],
    "EXPECT_THROW(f(-1), OrderError);",
    "EXPECT_THROW(f(-1), AppError);",
    "from `OrderError` to `AppError`"
);

reported!(
    cpp_project_class_moved_past_its_standard_base_is_reported,
    Cpp,
    [CPP_ERRORS],
    "EXPECT_THROW(f(-1), PaymentError);",
    "EXPECT_THROW(f(-1), std::runtime_error);",
    "from `PaymentError` to `runtime_error`"
);

silent!(
    cpp_same_file_sibling_replacement_is_silent,
    Cpp,
    [CPP_ERRORS],
    "EXPECT_THROW(f(-1), OrderError);",
    "EXPECT_THROW(f(-1), PaymentError);"
);

silent!(
    cpp_expect_throw_narrowed_is_silent,
    Cpp,
    "EXPECT_THROW(f(-1), std::logic_error);",
    "EXPECT_THROW(f(-2), std::invalid_argument);"
);

silent!(
    cpp_project_class_sharing_a_standard_name_is_silent,
    Cpp,
    "EXPECT_THROW(f(-1), mylib::out_of_range);",
    "EXPECT_THROW(f(-1), std::logic_error);"
);

silent!(
    cpp_expect_no_throw_unchanged_is_silent,
    Cpp,
    "EXPECT_NO_THROW(f(1));",
    "EXPECT_NO_THROW(f(2));"
);

not_vacuous!(
    cpp_expect_throw_only_test_is_not_vacuous,
    Cpp,
    "EXPECT_THROW(f(-1), std::invalid_argument);"
);
