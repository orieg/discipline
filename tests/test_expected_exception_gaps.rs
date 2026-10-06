//! Expected exceptions through the real binary (#560): how base and head expectations
//! of one test are paired, which class changes are a widening, and the forms each
//! language's extractor reads. A `reported!` case is a widening that must be reported;
//! a `silent!` case is a control that must stay unreported.

mod common;
use common::{Repo, Run};

const GATE: &str = "assertion-reduction";
const WIDENED: &str = "Expected Exception Or Panic Widened";

#[derive(Clone, Copy)]
enum Lang {
    Python,
    Java,
    Js,
    Ts,
    CSharp,
    Rust,
    Php,
}

fn indent(body: &str, by: &str) -> String {
    body.lines()
        .map(|l| format!("{by}{l}\n"))
        .collect::<String>()
}

/// The test file of `lang` holding one test with `body`. Rust takes the whole file.
fn source(lang: Lang, body: &str) -> (&'static str, String) {
    match lang {
        Lang::Python => (
            "tests/test_sut.py",
            format!(
                "import pytest\n\n\ndef test_rejects():\n{}",
                indent(body, "    ")
            ),
        ),
        Lang::Java => (
            "src/test/java/SutTest.java",
            format!(
                "import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\nclass SutTest {{\n    @Test\n    void rejects() {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::Js => (
            "tests/sut.test.js",
            format!("test(\"rejects\", () => {{\n{}}});\n", indent(body, "  ")),
        ),
        Lang::Ts => (
            "tests/sut.test.ts",
            format!("test(\"rejects\", () => {{\n{}}});\n", indent(body, "  ")),
        ),
        Lang::CSharp => (
            "tests/SutTests.cs",
            format!(
                "using Xunit;\npublic class SutTests {{\n    [Fact]\n    public void Rejects() {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
        Lang::Rust => ("tests/sut.rs", format!("{body}\n")),
        Lang::Php => (
            "tests/SutTest.php",
            format!(
                "<?php\nuse PHPUnit\\Framework\\TestCase;\n\nclass SutTest extends TestCase {{\n    public function testRejects(): void {{\n{}    }}\n}}\n",
                indent(body, "        ")
            ),
        ),
    }
}

/// Commits `base` on the base side and `head` on the work branch, then checks.
fn change(lang: Lang, base: &str, head: &str) -> Run {
    let repo = Repo::new();
    let (path, base_src) = source(lang, base);
    let (_, head_src) = source(lang, head);
    repo.commit_base(path, &base_src, "test: base");
    repo.write(path, &head_src);
    repo.commit("test: change the test");
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

macro_rules! reported {
    ($name:ident, $lang:ident, $base:expr, $head:expr) => {
        reported!($name, $lang, $base, $head, "");
    };
    ($name:ident, $lang:ident, $base:expr, $head:expr, $needle:expr) => {
        #[test]
        fn $name() {
            let run = change(Lang::$lang, $base, $head);
            let got = widened_messages(&run);
            assert_eq!(got.len(), 1, "{got:?}\n{}{}", run.stdout, run.stderr);
            assert!(got[0].contains($needle), "{got:?}");
            assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
        }
    };
}

macro_rules! silent {
    ($name:ident, $lang:ident, $base:expr, $head:expr) => {
        #[test]
        fn $name() {
            let run = change(Lang::$lang, $base, $head);
            let got = widened_messages(&run);
            assert!(got.is_empty(), "{got:?}\n{}{}", run.stdout, run.stderr);
        }
    };
}

// ---------------------------------------------------------------------------
// Pairing of base and head expectations within one test.
// ---------------------------------------------------------------------------

reported!(
    py_widening_with_an_edited_block_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)",
    "with pytest.raises(Exception):\n    f(-2)",
    "from `ValueError` to `Exception`"
);

reported!(
    py_widening_with_a_renamed_as_binding_is_reported,
    Python,
    "with pytest.raises(ValueError) as e:\n    f(-1)",
    "with pytest.raises(Exception) as err:\n    f(-1)"
);

reported!(
    py_one_of_two_identical_sites_widened_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\ng()\nwith pytest.raises(ValueError):\n    f(-1)",
    "with pytest.raises(ValueError):\n    f(-1)\ng()\nwith pytest.raises(Exception):\n    f(-1)"
);

reported!(
    py_site_replaced_by_a_wider_site_is_reported,
    Python,
    "with pytest.raises(ValueError, match=\"neg\"):\n    f(-1)",
    "with pytest.raises(Exception):\n    g()"
);

reported!(
    py_reorder_with_one_site_widened_and_edited_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nwith pytest.raises(TypeError):\n    g(\"x\")",
    "with pytest.raises(TypeError):\n    g(\"x\")\nwith pytest.raises(Exception):\n    f(-2)"
);

reported!(
    py_site_removed_while_a_wider_one_is_added_is_reported,
    Python,
    "with pytest.raises(KeyError):\n    f(-1)\nwith pytest.raises(TypeError):\n    g(\"x\")",
    "with pytest.raises(TypeError):\n    g(\"x\")\nwith pytest.raises(BaseException):\n    h()"
);

reported!(
    py_site_dropped_while_the_assertion_count_holds_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nassert g() == 1",
    "assert f(1) == 1\nassert g() == 1",
    "no longer"
);

/// Control: an expectation removed along with a lower assertion count is the count's
/// finding, and is not reported a second time as a dropped expectation.
#[test]
fn py_site_dropped_with_a_lower_count_is_the_reduction_finding() {
    let run = change(
        Lang::Python,
        "with pytest.raises(ValueError):\n    f(-1)\nassert g() == 1",
        "assert g() == 1",
    );
    assert_eq!(
        run.titles(GATE),
        vec!["Assertion Count Decreased In Existing Test"],
        "{}",
        run.stdout
    );
}

// Control: two unchanged sites that trade strictness were already paired by skeleton.
reported!(
    py_two_sites_swapping_strictness_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nwith pytest.raises(Exception):\n    g()",
    "with pytest.raises(Exception):\n    f(-1)\nwith pytest.raises(ValueError):\n    g()"
);

silent!(
    py_pure_reorder_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nwith pytest.raises(Exception):\n    g()",
    "with pytest.raises(Exception):\n    g()\nwith pytest.raises(ValueError):\n    f(-1)"
);

silent!(
    py_reorder_with_edited_blocks_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)\nwith pytest.raises(Exception):\n    g()",
    "with pytest.raises(Exception):\n    g(2)\nwith pytest.raises(ValueError):\n    f(-2)"
);

silent!(
    py_edited_block_with_the_same_expectation_is_silent,
    Python,
    "with pytest.raises(ValueError, match=\"neg\"):\n    f(-1)",
    "with pytest.raises(ValueError, match=\"neg\"):\n    f(-2)"
);

silent!(
    py_added_wider_site_beside_a_kept_one_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)",
    "with pytest.raises(ValueError):\n    f(-2)\nwith pytest.raises(Exception):\n    g()"
);

reported!(
    java_widening_with_an_edited_lambda_is_reported,
    Java,
    "assertThrows(IllegalStateException.class, () -> sut.run(1));",
    "assertThrows(Throwable.class, () -> sut.run(2));",
    "from `IllegalStateException` to `Throwable`"
);

silent!(
    java_edited_lambda_with_the_same_expectation_is_silent,
    Java,
    "assertThrows(IllegalStateException.class, () -> sut.run(1));",
    "assertThrows(IllegalStateException.class, () -> sut.run(2));"
);

reported!(
    js_type_dropped_with_an_edited_callback_is_reported,
    Js,
    "expect(() => f(1)).toThrow(TypeError);",
    "expect(() => f(2)).toThrow();"
);

reported!(
    ts_type_dropped_with_an_edited_callback_is_reported,
    Ts,
    "expect(() => f(1)).toThrow(TypeError);",
    "expect(() => f(2)).toThrow();"
);

silent!(
    js_edited_callback_with_the_same_expectation_is_silent,
    Js,
    "expect(() => f(1)).toThrow(TypeError);",
    "expect(() => f(2)).toThrow(TypeError);"
);

reported!(
    csharp_widening_with_an_edited_lambda_is_reported,
    CSharp,
    "Assert.Throws<ArgumentNullException>(() => sut.Run(1));",
    "Assert.Throws<Exception>(() => sut.Run(2));"
);

silent!(
    csharp_edited_lambda_with_the_same_expectation_is_silent,
    CSharp,
    "Assert.Throws<ArgumentNullException>(() => sut.Run(1));",
    "Assert.Throws<ArgumentNullException>(() => sut.Run(2));"
);

reported!(
    php_message_expectation_dropped_while_the_class_is_kept_is_reported,
    Php,
    "$this->expectException(InvalidArgumentException::class);\n$this->expectExceptionMessage(\"negative\");\n$this->assertSame(1, a());\nsut(-1);",
    "$this->expectException(InvalidArgumentException::class);\n$this->assertSame(1, a());\n$this->assertSame(2, b());\nsut(-1);",
    "no longer"
);

// ---------------------------------------------------------------------------
// The class hierarchy.
// ---------------------------------------------------------------------------

reported!(
    py_key_error_to_lookup_error_is_reported,
    Python,
    "with pytest.raises(KeyError):\n    f(-1)",
    "with pytest.raises(LookupError):\n    f(-1)",
    "from `KeyError` to `LookupError`"
);

reported!(
    py_file_not_found_to_os_error_is_reported,
    Python,
    "with pytest.raises(FileNotFoundError):\n    f(-1)",
    "with pytest.raises(OSError):\n    f(-1)"
);

reported!(
    py_unittest_zero_division_to_arithmetic_error_is_reported,
    Python,
    "with self.assertRaises(ZeroDivisionError):\n    f(-1)",
    "with self.assertRaises(ArithmeticError):\n    f(-1)"
);

reported!(
    py_tuple_grown_is_reported,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)",
    "with pytest.raises((ValueError, TypeError)):\n    f(-1)",
    "TypeError"
);

reported!(
    py_tuple_grown_by_one_member_is_reported,
    Python,
    "with pytest.raises((ValueError, TypeError)):\n    f(-1)",
    "with pytest.raises((ValueError, TypeError, KeyError)):\n    f(-1)"
);

silent!(
    py_tuple_reordered_is_silent,
    Python,
    "with pytest.raises((ValueError, TypeError)):\n    f(-1)",
    "with pytest.raises((TypeError, ValueError)):\n    f(-1)"
);

silent!(
    py_tuple_shrunk_is_silent,
    Python,
    "with pytest.raises((ValueError, TypeError)):\n    f(-1)",
    "with pytest.raises(ValueError):\n    f(-1)"
);

silent!(
    py_narrowing_to_a_subclass_is_silent,
    Python,
    "with pytest.raises(LookupError):\n    f(-1)",
    "with pytest.raises(KeyError):\n    f(-1)"
);

silent!(
    py_sibling_replacement_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)",
    "with pytest.raises(TypeError):\n    f(-1)"
);

silent!(
    py_qualified_spelling_of_the_same_class_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)",
    "with pytest.raises(builtins.ValueError):\n    f(-1)"
);

reported!(
    py_file_not_found_to_an_alias_of_os_error_is_reported,
    Python,
    "with pytest.raises(FileNotFoundError):\n    f(-1)",
    "with pytest.raises(IOError):\n    f(-1)"
);

silent!(
    py_alias_of_os_error_is_silent,
    Python,
    "with pytest.raises(IOError):\n    f(-1)",
    "with pytest.raises(OSError):\n    f(-1)"
);

// The rule for an unknown relation: a replacement between two classes that neither
// the standard hierarchy nor the general names order is not reported.
silent!(
    py_replacement_between_two_user_classes_is_silent,
    Python,
    "with pytest.raises(OrderError):\n    f(-1)",
    "with pytest.raises(PaymentError):\n    f(-1)"
);

reported!(
    java_file_not_found_to_io_exception_is_reported,
    Java,
    "assertThrows(FileNotFoundException.class, () -> sut.run(1));",
    "assertThrows(IOException.class, () -> sut.run(1));",
    "from `FileNotFoundException` to `IOException`"
);

reported!(
    java_illegal_argument_to_runtime_exception_is_reported,
    Java,
    "assertThrows(IllegalArgumentException.class, () -> sut.run(1));",
    "assertThrows(RuntimeException.class, () -> sut.run(1));"
);

reported!(
    java_number_format_to_illegal_argument_is_reported,
    Java,
    "assertThrows(NumberFormatException.class, () -> sut.run(1));",
    "assertThrows(java.lang.IllegalArgumentException.class, () -> sut.run(1));"
);

silent!(
    java_narrowing_to_a_subclass_is_silent,
    Java,
    "assertThrows(IOException.class, () -> sut.run(1));",
    "assertThrows(FileNotFoundException.class, () -> sut.run(1));"
);

silent!(
    java_sibling_replacement_is_silent,
    Java,
    "assertThrows(IllegalArgumentException.class, () -> sut.run(1));",
    "assertThrows(IllegalStateException.class, () -> sut.run(1));"
);

silent!(
    java_qualified_spelling_of_the_same_class_is_silent,
    Java,
    "assertThrows(IOException.class, () -> sut.run(1));",
    "assertThrows(java.io.IOException.class, () -> sut.run(1));"
);

silent!(
    java_replacement_between_two_user_classes_is_silent,
    Java,
    "assertThrows(OrderException.class, () -> sut.run(1));",
    "assertThrows(PaymentException.class, () -> sut.run(1));"
);

// An exact-type assertion accepts no subclass, so moving it to a parent class other
// than a general one changes which class is expected and accepts no more than before.
silent!(
    java_exact_assertion_moved_to_a_parent_class_is_silent,
    Java,
    "assertThrowsExactly(NumberFormatException.class, () -> sut.run(1));",
    "assertThrowsExactly(IllegalArgumentException.class, () -> sut.run(1));"
);

reported!(
    csharp_throws_any_moved_to_a_parent_class_is_reported,
    CSharp,
    "Assert.ThrowsAny<ArgumentNullException>(() => sut.Run(1));",
    "Assert.ThrowsAny<ArgumentException>(() => sut.Run(1));",
    "from `ArgumentNullException` to `ArgumentException`"
);

reported!(
    csharp_throws_any_file_not_found_to_io_exception_is_reported,
    CSharp,
    "Assert.ThrowsAny<FileNotFoundException>(() => sut.Run(1));",
    "Assert.ThrowsAny<System.IO.IOException>(() => sut.Run(1));"
);

silent!(
    csharp_exact_assertion_moved_to_a_parent_class_is_silent,
    CSharp,
    "Assert.Throws<ArgumentNullException>(() => sut.Run(1));",
    "Assert.Throws<ArgumentException>(() => sut.Run(1));"
);

silent!(
    csharp_qualified_spelling_of_the_same_class_is_silent,
    CSharp,
    "Assert.Throws<ArgumentNullException>(() => sut.Run(1));",
    "Assert.Throws<System.ArgumentNullException>(() => sut.Run(1));"
);

silent!(
    csharp_throws_any_narrowed_is_silent,
    CSharp,
    "Assert.ThrowsAny<ArgumentException>(() => sut.Run(1));",
    "Assert.ThrowsAny<ArgumentNullException>(() => sut.Run(1));"
);

// Control: `Error` is a general name, so this was already reported.
reported!(
    js_type_error_to_error_is_reported,
    Js,
    "expect(() => f(1)).toThrow(TypeError);",
    "expect(() => f(1)).toThrow(Error);",
    "from `TypeError` to `Error`"
);

silent!(
    js_sibling_replacement_is_silent,
    Js,
    "expect(() => f(1)).toThrow(TypeError);",
    "expect(() => f(1)).toThrow(RangeError);"
);

silent!(
    js_narrowing_is_silent,
    Js,
    "expect(() => f(1)).toThrow(Error);",
    "expect(() => f(1)).toThrow(TypeError);"
);

reported!(
    php_expected_class_moved_to_exception_is_reported,
    Php,
    "$this->expectException(InvalidArgumentException::class);\nsut(-1);",
    "$this->expectException(Exception::class);\nsut(-1);",
    "from `InvalidArgumentException` to `Exception`"
);

reported!(
    php_expected_class_moved_to_a_parent_class_is_reported,
    Php,
    "$this->expectException(InvalidArgumentException::class);\nsut(-1);",
    "$this->expectException(\\LogicException::class);\nsut(-1);"
);

silent!(
    php_sibling_replacement_is_silent,
    Php,
    "$this->expectException(InvalidArgumentException::class);\nsut(-1);",
    "$this->expectException(DomainException::class);\nsut(-1);"
);

silent!(
    php_qualified_spelling_of_the_same_class_is_silent,
    Php,
    "$this->expectException(InvalidArgumentException::class);\nsut(-1);",
    "$this->expectException(\\InvalidArgumentException::class);\nsut(-1);"
);

// ---------------------------------------------------------------------------
// A matcher loosened while still present.
// ---------------------------------------------------------------------------

const RUST_NARROW: &str =
    "#[test]\n#[should_panic(expected = \"overflow\")]\nfn t() {\n    f();\n}";

reported!(
    rust_expected_emptied_is_reported,
    Rust,
    RUST_NARROW,
    "#[test]\n#[should_panic(expected = \"\")]\nfn t() {\n    f();\n}"
);

reported!(
    rust_expected_shortened_is_reported,
    Rust,
    "#[test]\n#[should_panic(expected = \"attempt to add with overflow\")]\nfn t() {\n    f();\n}",
    "#[test]\n#[should_panic(expected = \"overflow\")]\nfn t() {\n    f();\n}"
);

reported!(
    rust_name_value_message_dropped_is_reported,
    Rust,
    "#[test]\n#[should_panic = \"overflow\"]\nfn t() {\n    f();\n}",
    "#[test]\n#[should_panic]\nfn t() {\n    f();\n}"
);

silent!(
    rust_name_value_form_rewritten_as_expected_is_silent,
    Rust,
    "#[test]\n#[should_panic = \"overflow\"]\nfn t() {\n    f();\n}",
    RUST_NARROW
);

silent!(
    rust_expected_lengthened_is_silent,
    Rust,
    RUST_NARROW,
    "#[test]\n#[should_panic(expected = \"attempt to add with overflow\")]\nfn t() {\n    f();\n}"
);

reported!(
    rust_proptest_expected_dropped_is_reported,
    Rust,
    "proptest! {\n    #[test]\n    #[should_panic(expected = \"overflow\")]\n    fn t(x in 0..10i32) {\n        f(x);\n    }\n}",
    "proptest! {\n    #[test]\n    #[should_panic]\n    fn t(x in 0..10i32) {\n        f(x);\n    }\n}"
);

reported!(
    py_match_emptied_is_reported,
    Python,
    "with pytest.raises(ValueError, match=\"negative\"):\n    f(-1)",
    "with pytest.raises(ValueError, match=\"\"):\n    f(-1)"
);

reported!(
    py_match_made_a_wildcard_is_reported,
    Python,
    "with pytest.raises(ValueError, match=r\"negative\"):\n    f(-1)",
    "with pytest.raises(ValueError, match=r\".*\"):\n    f(-1)"
);

reported!(
    py_match_shortened_is_reported,
    Python,
    "with pytest.raises(ValueError, match=\"negative value\"):\n    f(-1)",
    "with pytest.raises(ValueError, match=\"value\"):\n    f(-1)"
);

silent!(
    py_match_requoted_is_silent,
    Python,
    "with pytest.raises(ValueError, match=\"negative\"):\n    f(-1)",
    "with pytest.raises(ValueError, match=r'negative'):\n    f(-1)"
);

silent!(
    py_match_replaced_by_another_pattern_is_silent,
    Python,
    "with pytest.raises(ValueError, match=\"negative\"):\n    f(-1)",
    "with pytest.raises(ValueError, match=\"must be positive\"):\n    f(-1)"
);

reported!(
    py_assert_raises_regex_emptied_is_reported,
    Python,
    "with self.assertRaisesRegex(ValueError, \"negative\"):\n    f(-1)",
    "with self.assertRaisesRegex(ValueError, \"\"):\n    f(-1)"
);

reported!(
    js_message_emptied_is_reported,
    Js,
    "expect(() => f(1)).toThrow(\"negative\");",
    "expect(() => f(1)).toThrow(\"\");"
);

reported!(
    js_regex_made_a_wildcard_is_reported,
    Js,
    "expect(() => f(1)).toThrow(/negative/);",
    "expect(() => f(1)).toThrow(/.*/);"
);

// ---------------------------------------------------------------------------
// Forms the extractors did not read.
// ---------------------------------------------------------------------------

reported!(
    py_call_form_widened_is_reported,
    Python,
    "pytest.raises(ValueError, f, -1)",
    "pytest.raises(Exception, f, -1)",
    "from `ValueError` to `Exception`"
);

reported!(
    py_expected_exception_keyword_widened_is_reported,
    Python,
    "with pytest.raises(expected_exception=ValueError):\n    f(-1)",
    "with pytest.raises(expected_exception=Exception):\n    f(-1)"
);

silent!(
    py_keyword_spelling_of_the_same_class_is_silent,
    Python,
    "with pytest.raises(ValueError):\n    f(-1)",
    "with pytest.raises(expected_exception=ValueError):\n    f(-1)"
);

reported!(
    java_exact_assertion_relaxed_is_reported,
    Java,
    "assertThrowsExactly(IllegalStateException.class, () -> sut.run(1));",
    "assertThrows(IllegalStateException.class, () -> sut.run(1));",
    "exact"
);

silent!(
    java_assertion_made_exact_is_silent,
    Java,
    "assertThrows(IllegalStateException.class, () -> sut.run(1));",
    "assertThrowsExactly(IllegalStateException.class, () -> sut.run(1));"
);

fn junit4(annotation: &str) -> String {
    format!(
        "import org.junit.Test;\n\npublic class SutTest {{\n    {annotation}\n    public void rejects() {{\n        sut.run(1);\n    }}\n}}\n"
    )
}

fn junit4_change(base: &str, head: &str) -> Run {
    let repo = Repo::new();
    let path = "src/test/java/SutTest.java";
    repo.commit_base(path, &junit4(base), "test: base");
    repo.write(path, &junit4(head));
    repo.commit("test: change the annotation");
    repo.check(&[])
}

#[test]
fn java_annotation_with_a_second_element_widened_is_reported() {
    let run = junit4_change(
        "@Test(expected = IllegalStateException.class, timeout = 100)",
        "@Test(expected = Exception.class, timeout = 100)",
    );
    let got = widened_messages(&run);
    assert_eq!(got.len(), 1, "{got:?}\n{}{}", run.stdout, run.stderr);
    assert!(
        got[0].contains("from `IllegalStateException` to `Exception`"),
        "{got:?}"
    );
}

/// Control: the single-element annotation was already read.
#[test]
fn java_annotation_widened_is_reported() {
    let run = junit4_change(
        "@Test(expected = IllegalStateException.class)",
        "@Test(expected = Exception.class)",
    );
    assert_eq!(widened_messages(&run).len(), 1, "{}", run.stdout);
}

/// Control: another element changing beside the same class is silent.
#[test]
fn java_annotation_timeout_changed_is_silent() {
    let run = junit4_change(
        "@Test(expected = IllegalStateException.class, timeout = 100)",
        "@Test(timeout = 200, expected = IllegalStateException.class)",
    );
    assert!(widened_messages(&run).is_empty(), "{}", run.stdout);
}

reported!(
    js_member_class_dropped_is_reported,
    Js,
    "expect(() => f(1)).toThrow(errors.NotFound);",
    "expect(() => f(1)).toThrow();"
);

reported!(
    js_template_message_dropped_is_reported,
    Js,
    "expect(() => f(1)).toThrow(`negative`);",
    "expect(() => f(1)).toThrow();"
);

reported!(
    js_message_constant_dropped_is_reported,
    Js,
    "expect(() => f(1)).toThrow(ERR_MSG);",
    "expect(() => f(1)).toThrow();",
    "matcher"
);

silent!(
    js_member_class_kept_is_silent,
    Js,
    "expect(() => f(1)).toThrow(errors.NotFound);",
    "expect(() => f(2)).toThrow(errors.NotFound);"
);

reported!(
    csharp_throws_replaced_by_catch_is_reported,
    CSharp,
    "Assert.Throws<ArgumentException>(() => sut.Run(1));",
    "Assert.Catch<Exception>(() => sut.Run(1));"
);

reported!(
    csharp_qualified_receiver_widened_is_reported,
    CSharp,
    "Xunit.Assert.Throws<ArgumentNullException>(() => sut.Run(1));",
    "Xunit.Assert.Throws<Exception>(() => sut.Run(1));"
);

reported!(
    csharp_typeof_form_widened_is_reported,
    CSharp,
    "Assert.Throws(typeof(ArgumentNullException), () => sut.Run(1));",
    "Assert.Throws(typeof(Exception), () => sut.Run(1));",
    "from `ArgumentNullException` to `Exception`"
);

reported!(
    csharp_fluent_throw_widened_is_reported,
    CSharp,
    "act.Should().Throw<ArgumentNullException>();",
    "act.Should().Throw<Exception>();"
);

reported!(
    csharp_fluent_message_dropped_is_reported,
    CSharp,
    "act.Should().Throw<ArgumentNullException>().WithMessage(\"*negative*\");",
    "act.Should().Throw<ArgumentNullException>();"
);

reported!(
    csharp_fluent_exact_relaxed_is_reported,
    CSharp,
    "act.Should().ThrowExactly<ArgumentNullException>();",
    "act.Should().Throw<ArgumentNullException>();"
);

silent!(
    csharp_fluent_unchanged_is_silent,
    CSharp,
    "act.Should().Throw<ArgumentNullException>().WithMessage(\"*negative*\");\nsut.Run(1);",
    "act.Should().Throw<ArgumentNullException>().WithMessage(\"*negative*\");\nsut.Run(2);"
);

// ---------------------------------------------------------------------------
// Changes that were reported and are not a widening.
// ---------------------------------------------------------------------------

silent!(
    js_negated_expectation_losing_its_type_is_silent,
    Js,
    "expect(() => f(1)).not.toThrow(TypeError);",
    "expect(() => f(1)).not.toThrow();"
);

reported!(
    js_negated_expectation_gaining_a_type_is_reported,
    Js,
    "expect(() => f(1)).not.toThrow();",
    "expect(() => f(1)).not.toThrow(TypeError);"
);

silent!(
    py_matcher_traded_for_a_narrower_type_is_silent,
    Python,
    "with pytest.raises(Exception, match=\"neg\"):\n    f(-1)",
    "with pytest.raises(ValueError):\n    f(-1)"
);

silent!(
    py_matcher_traded_for_a_subclass_is_silent,
    Python,
    "with pytest.raises(LookupError, match=\"neg\"):\n    f(-1)",
    "with pytest.raises(KeyError):\n    f(-1)"
);

// Control: a matcher dropped beside the same type is a widening.
reported!(
    py_matcher_dropped_beside_the_same_type_is_reported,
    Python,
    "with pytest.raises(ValueError, match=\"neg\"):\n    f(-1)",
    "with pytest.raises(ValueError):\n    f(-1)",
    "matcher"
);

// ---------------------------------------------------------------------------
// Left as they are: pinned so that a change of rule is a visible change.
// ---------------------------------------------------------------------------

/// A user class whose base is written in the same file is not resolved: the move to
/// that base is an unknown relation, and is not reported.
#[test]
fn py_user_class_moved_to_its_declared_base_is_not_resolved() {
    let file = |raised: &str| {
        format!(
            "import pytest\n\n\nclass AppError(RuntimeError):\n    pass\n\n\nclass OrderError(AppError):\n    pass\n\n\ndef test_rejects():\n    with pytest.raises({raised}):\n        f(-1)\n"
        )
    };
    let repo = Repo::new();
    repo.commit_base("tests/test_sut.py", &file("OrderError"), "test: base");
    repo.write("tests/test_sut.py", &file("AppError"));
    repo.commit("test: expect the base class");
    let run = repo.check(&[]);
    assert!(widened_messages(&run).is_empty(), "{}", run.stdout);
}

// ---------------------------------------------------------------------------
// `vacuous-tests` reads the same expectations.
// ---------------------------------------------------------------------------

fn new_csharp_test(body: &str) -> Run {
    let repo = Repo::new();
    let (path, src) = source(Lang::CSharp, body);
    repo.write(path, &src);
    repo.commit("test: add a csharp test");
    repo.check(&[])
}

#[test]
fn csharp_fluent_throw_only_test_is_not_vacuous() {
    let run = new_csharp_test("act.Should().Throw<ArgumentException>();");
    assert!(run.titles("vacuous-tests").is_empty(), "{}", run.stdout);
}

/// Control: a `Should()` chain that expects no failure is not an expected exception.
#[test]
fn csharp_fluent_chain_without_throw_is_vacuous() {
    let run = new_csharp_test("act.Should();");
    assert_eq!(
        run.titles("vacuous-tests"),
        vec!["Vacuous Test Added"],
        "{}",
        run.stdout
    );
}
