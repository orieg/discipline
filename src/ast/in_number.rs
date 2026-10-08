//! Sources that hold many of one thing: many tests, many methods of one class, many
//! records in one test, handlers one inside another (#672).
//!
//! A reader that reads the file, the tests or the records of a test again for each test,
//! each method or each record does work that grows with the square of their number. Each
//! test here reads a source of some number of them and one of four times as many, and
//! holds the steps of the second to under five times the steps of the first: the work
//! is counted in steps (`ancestry::steps`), not in time, so the result is the same on
//! any machine. Beside each stands a test of what is read, which does not depend on how.

use super::{ancestry, default_registry, AssertVocabulary, ParsedFileFacts};

/// The facts of `src` under `path`, and the steps reading it counted.
fn read(path: &str, src: &str) -> (ParsedFileFacts, u64) {
    let registry = default_registry();
    let pack = registry.find_pack(path).expect("a pack for the path");
    let (facts, counted) =
        ancestry::steps(|| pack.extract(path, src, &AssertVocabulary::default()));
    (
        facts.unwrap_or_else(|e| panic!("{path} is read: {e:#}")),
        counted,
    )
}

/// `line(i)` for each of `0..n`, joined.
fn lines(n: usize, line: impl Fn(usize) -> String) -> String {
    (0..n).map(line).collect()
}

/// Holds the steps for `source(160)` to under five times the steps for `source(40)`,
/// and returns the facts of both.
fn in_proportion(
    what: &str,
    path: &str,
    source: impl Fn(usize) -> String,
) -> (ParsedFileFacts, ParsedFileFacts) {
    let ((few_facts, few), (many_facts, many)) =
        (read(path, &source(40)), read(path, &source(160)));
    assert!(few > 0, "{what}: nothing was counted");
    assert!(many < 5 * few, "{what}: {few} steps for 40, {many} for 160");
    (few_facts, many_facts)
}

/// The functions `src` holds whose bodies are judged, by name.
fn judged(src: &str) -> Vec<String> {
    read("pkg/a.py", src)
        .0
        .functions
        .into_iter()
        .map(|f| f.name)
        .collect()
}

/// A Python method that a class of the same file overrides is a contract and is not
/// judged: the class that overrides it derives from the method's class by name, is
/// another class, and defines the method in its own body.
#[test]
fn a_python_method_is_overridden_by_a_class_that_derives_from_its_class() {
    let names = |got: &[&str]| got.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    // Overridden: the method of the base is left out, the others are judged.
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\n    def stop(self):\n        pass\nclass Impl(Base):\n    def run(self):\n        return 1\n"),
        names(&["stop", "run"])
    );
    // A class that defines the method and derives from another class does not.
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\nclass Impl(Other):\n    def run(self):\n        return 1\n"),
        names(&["run", "run"])
    );
    // A class that derives from it and defines another method does not.
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\nclass Impl(Base):\n    def stop(self):\n        return 1\n"),
        names(&["run", "stop"])
    );
    // The base is named among several, with a module before it, or as a keyword.
    for bases in ["Mixin, Base", "pkg.Base", "Other, metaclass=Base"] {
        assert_eq!(
            judged(&format!(
                "class Base:\n    def run(self):\n        pass\nclass Impl({bases}):\n    def run(self):\n        return 1\n"
            )),
            names(&["run"]),
            "{bases}"
        );
    }
    // A longer name that holds the base's is another name.
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\nclass Impl(BaseOne):\n    def run(self):\n        return 1\n"),
        names(&["run", "run"])
    );
    // A class is not overridden by itself, though it names itself as a base; a second
    // class of the same name that derives from the name overrides the first.
    assert_eq!(
        judged("class Base(Base):\n    def run(self):\n        pass\n"),
        names(&["run"])
    );
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\nclass Base(Base):\n    def run(self):\n        pass\n"),
        names(&["run"])
    );
    // A class that names itself as a base is overridden by another that derives from
    // the name, and by a second class of its own name and bases.
    assert_eq!(
        judged("class Base(Base):\n    def run(self):\n        pass\nclass Impl(Base):\n    def run(self):\n        return 1\n"),
        names(&["run"])
    );
    assert_eq!(
        judged("class Base(Base):\n    def run(self):\n        pass\nclass Base(Base):\n    def run(self):\n        pass\n"),
        names(&[])
    );
    // Two classes derive from the base: either one overrides.
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\n    def stop(self):\n        pass\nclass One(Base):\n    def run(self):\n        return 1\nclass Two(Base):\n    def stop(self):\n        return 2\n    def run(self):\n        return 2\n"),
        names(&["run", "stop", "run"])
    );
    // The override is decorated, or its class is declared inside a function.
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\nclass Impl(Base):\n    @staticmethod\n    def run():\n        return 1\n"),
        names(&["run"])
    );
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\ndef make():\n    class Impl(Base):\n        def run(self):\n            return 1\n    return Impl\n"),
        names(&["make", "run"])
    );
    // A function of that name inside a method of the deriving class is not its method.
    assert_eq!(
        judged("class Base:\n    def run(self):\n        pass\nclass Impl(Base):\n    def go(self):\n        def run():\n            return 1\n        return run\n"),
        names(&["run", "go", "run"])
    );
}

/// The methods of one Python class cost steps in proportion to their number. Before
/// #672 the file was read for each method, to find a class that overrides it.
#[test]
fn python_methods_of_one_class_cost_steps_in_proportion_to_their_number() {
    let (_, many) = in_proportion("methods of a class", "pkg/a.py", |n| {
        format!(
            "class Base:\n{}class Impl(Base):\n    def m0(self):\n        return 1\n",
            lines(n, |i| format!("    def m{i}(self):\n        pass\n"))
        )
    });
    // Every method but the overridden one, and the override.
    assert_eq!(many.functions.len(), 160);
    // As many classes that derive from one base, each overriding its one method.
    let (_, many) = in_proportion("classes of one base", "pkg/a.py", |n| {
        format!(
            "class Base:\n    def run(self):\n        pass\n{}",
            lines(n, |i| format!(
                "class C{i}(Base):\n    def run(self):\n        return {i}\n"
            ))
        )
    });
    assert_eq!(many.functions.len(), 160);
    // As many methods of the base, each overridden by a class of its own.
    let (_, many) = in_proportion("methods and classes of one base", "pkg/a.py", |n| {
        format!(
            "class Base:\n{}{}",
            lines(n, |i| format!("    def m{i}(self):\n        pass\n")),
            lines(n, |i| format!(
                "class C{i}(Base):\n    def m{i}(self):\n        return {i}\n"
            ))
        )
    });
    assert_eq!(many.functions.len(), 160);
}

/// The cases of each test of `src`, a Go test file, by the name of the test.
fn go_cases(src: &str) -> Vec<(String, Option<usize>)> {
    read("m_test.go", src)
        .0
        .tests
        .into_iter()
        .map(|t| (t.name, t.cases))
        .collect()
}

/// A Go test runs the rows of the package-level tables its body names, and reads a
/// slice of a struct type the file declares as a table.
#[test]
fn a_go_test_counts_the_rows_of_the_package_tables_it_names() {
    let src = "package p\n\ntype row struct{ a int }\n\nvar cases = []struct{ a int }{{1}, {2}}\n\nvar (\n\trows = []row{{1}, {2}, {3}}\n\tother, more = 1, 2\n)\n\nfunc TestA(t *testing.T) {\n\tfor _, c := range cases {\n\t\tcheck(t, c)\n\t}\n}\n\nfunc TestB(t *testing.T) {\n\tfor _, r := range rows {\n\t\tcheck(t, r)\n\t}\n}\n\nfunc TestC(t *testing.T) {\n\tuse(rows)\n\tuse(cases)\n\tuse(rows)\n}\n\nfunc TestD(t *testing.T) {\n\tuse(other)\n}\n\nfunc TestE(t *testing.T) {\n\tlocal := []row{{1}}\n\tfor _, r := range local {\n\t\tcheck(t, r)\n\t}\n}\n";
    let named = |name: &str, cases: Option<usize>| (name.to_string(), cases);
    assert_eq!(
        go_cases(src),
        vec![
            named("TestA", Some(2)),
            named("TestB", Some(3)),
            // The rows of the first table the file declares among those named.
            named("TestC", Some(2)),
            named("TestD", None),
            named("TestE", Some(1)),
        ]
    );
}

/// The rows of the package-level tables a test names are counted in the order the file
/// declares the tables, each table once, whatever order the test names them in.
#[test]
fn a_go_test_counts_its_package_tables_in_the_order_the_file_declares_them() {
    let rows_of = |body: &str| -> Option<Vec<String>> {
        read(
            "m_test.go",
            &format!(
                "package p\n\nvar first = []struct{{ a int }}{{{{1}}, {{2}}}}\n\nvar second = []struct{{ a int }}{{{{3}}}}\n\nfunc TestA(t *testing.T) {{\n{body}}}\n"
            ),
        )
        .0
        .tests
        .remove(0)
        .case_rows
    };
    let in_order = rows_of("\tuse(first)\n\tuse(second)\n");
    assert_eq!(in_order.as_ref().map(Vec::len), Some(3));
    assert_eq!(rows_of("\tuse(second)\n\tuse(first)\n"), in_order);
    assert_eq!(
        rows_of("\tuse(second)\n\tuse(first)\n\tuse(second)\n\tuse(first)\n"),
        in_order
    );
    // The rows of the second table alone are the last of the three.
    assert_eq!(
        rows_of("\tuse(second)\n"),
        in_order.map(|rows| rows[2..].to_vec())
    );
}

/// The tests of one Go file cost steps in proportion to their number. Before #672 the
/// declarations of the file were read for each test, and its package-level tables
/// compared with every name of each test.
#[test]
fn go_tests_of_one_file_cost_steps_in_proportion_to_their_number() {
    let (_, many) = in_proportion("Go tests", "m_test.go", |n| {
        format!(
            "package p\n\n{}{}",
            lines(n, |i| format!(
                "var table{i} = []struct{{ a int }}{{{{1}}, {{2}}}}\n"
            )),
            lines(n, |i| {
                format!(
                "func Test{i}(t *testing.T) {{\n\tfor _, v := range table{i} {{\n\t\tassert.Equal(t, {i}, f(v))\n\t}}\n}}\n"
            )
            })
        )
    });
    assert_eq!(many.tests.len(), 160);
    assert!(many.tests.iter().all(|t| t.cases == Some(2)));
}

/// The caught assertions of each test of `src`, as `(test, line, handler line,
/// tautology)`.
fn caught(path: &str, src: &str) -> Vec<(String, usize, usize, bool)> {
    read(path, src)
        .0
        .tests
        .iter()
        .flat_map(|t| {
            t.caught_assertions
                .iter()
                .map(|c| (t.name.clone(), c.line, c.handler_line, c.tautology))
        })
        .collect()
}

/// A caught assertion goes to the innermost test that holds its line, once, and is
/// marked as a tautology when it overlaps one.
#[test]
fn a_caught_assertion_is_placed_once_in_the_innermost_test_that_holds_it() {
    let named = |name: &str, line: usize, handler: usize, tautology: bool| {
        (name.to_string(), line, handler, tautology)
    };
    // Two handlers around one assertion, and a second assertion of the same line.
    assert_eq!(
        caught(
            "tests/m.test.js",
            "test('a', () => {\n  try {\n    try { expect(a).toBe(1); expect(b).toBe(2); } catch (e) {}\n  } catch (e) {}\n});\ntest('b', () => {\n  try { expect(c).toBe(c); } catch (e) {}\n  expect(d).toBe(4);\n});\n"
        ),
        vec![
            named("a", 3, 3, false),
            named("a", 3, 3, false),
            named("b", 7, 7, true),
        ]
    );
    // A function declared in a test is part of the test.
    assert_eq!(
        caught(
            "tests/test_m.py",
            "def test_outer():\n    try:\n        assert a == 1\n    except AssertionError:\n        pass\n    def test_inner():\n        try:\n            assert b == b\n            assert c == 3\n        except AssertionError:\n            pass\n    test_inner()\n"
        ),
        vec![
            named("test_outer", 3, 4, false),
            named("test_outer", 8, 10, true),
            named("test_outer", 9, 10, false),
        ]
    );
}

/// Caught assertions cost steps in proportion to their number, in one test and in many.
/// Before #672 every test of the file was read to place each, and every assertion the
/// test held was compared with each.
#[test]
fn caught_assertions_cost_steps_in_proportion_to_their_number() {
    let (_, many) = in_proportion("caught in one test", "tests/test_m.py", |n| {
        format!(
            "def test_x():\n    try:\n{}    except AssertionError:\n        pass\n",
            lines(n, |i| format!("        assert a == {i}\n"))
        )
    });
    assert_eq!(many.tests[0].caught_assertions.len(), 160);
    let (_, many) = in_proportion("caught in many tests", "tests/m.test.js", |n| {
        lines(n, |i| {
            format!("test('t{i}', () => {{ try {{ expect(a).toBe({i}); }} catch (e) {{}} }});\n")
        })
    });
    assert!(many.tests.iter().all(|t| t.caught_assertions.len() == 1));
    let (_, many) = in_proportion("caught tautologies", "tests/test_m.py", |n| {
        format!(
            "def test_x():\n    try:\n{}    except AssertionError:\n        pass\n",
            lines(n, |i| format!("        assert a{i} == a{i}\n"))
        )
    });
    assert_eq!(many.tests[0].caught_assertions.len(), 160);
    assert!(many.tests[0].caught_assertions.iter().all(|c| c.tautology));
}

/// Tests that call a helper in a loop, and tests that call a method of their class,
/// cost steps in proportion to their number. Before #672 every test and every helper
/// of the file was read to place each call.
#[test]
fn calls_placed_in_their_tests_cost_steps_in_proportion_to_the_tests() {
    let (_, many) = in_proportion("looped helper calls", "tests/test_m.py", |n| {
        format!(
            "def check(x):\n    assert x\n{}",
            lines(n, |i| format!(
                "def test_{i}():\n    for v in xs:\n        check(v)\n"
            ))
        )
    });
    assert_eq!(many.tests.len(), 160);
    assert!(many.tests.iter().all(|t| t.helper_reach.looped == 1));
    let (_, many) = in_proportion("receiver calls", "tests/t.rs", |n| {
        format!(
            "struct A;\nimpl A {{\n    fn done(&self) {{\n        assert!(self.ok());\n    }}\n}}\n{}",
            lines(n, |i| format!(
                "#[test]\nfn t{i}() {{\n    let v = make({i});\n    v.done();\n}}\n"
            ))
        )
    });
    assert_eq!(many.tests.len(), 160);
    assert!(many.tests.iter().all(|t| t.method_checks == 1));
}

/// The retry marker of each test of `src`, a Python test file.
fn retries(src: &str) -> Vec<Option<String>> {
    read("tests/test_m.py", src)
        .0
        .tests
        .into_iter()
        .map(|t| t.retries)
        .collect()
}

/// A retry marker inside a test is that test's; one above a test, ending within four
/// lines of it, decorates it; and of several the first in the source is taken.
#[test]
fn a_retry_marker_goes_to_the_test_it_is_in_or_stands_above() {
    let some = |m: &str| Some(m.to_string());
    let flaky = "@pytest.mark.flaky(reruns=3)";
    // Above the first test, and four and five lines above the next ones.
    assert_eq!(
        retries(&format!(
            "{flaky}\ndef test_a():\n    assert a\n{flaky}\n\n\n\ndef test_b():\n    assert b\n{flaky}\n\n\n\n\ndef test_c():\n    assert c\n"
        )),
        vec![some(flaky), some(flaky), None]
    );
    // Two markers above one test: the first in the source. It ends four lines above
    // the second test too, and five above the third.
    assert_eq!(
        retries("@flaky(max_runs=2)\n@pytest.mark.flaky(reruns=3)\ndef test_a():\n    assert a\ndef test_b():\n    assert b\ndef test_c():\n    assert c\n"),
        vec![some("@flaky(max_runs=2)"), some("@flaky(max_runs=2)"), None]
    );
    // A marker on a function that is not a test, just above a test.
    assert_eq!(
        retries(&format!(
            "{flaky}\ndef helper():\n    pass\ndef test_a():\n    assert a\ndef test_b():\n    assert b\n"
        )),
        vec![some(flaky), None]
    );
    // A marker inside a test is that test's, before one above it.
    assert_eq!(
        retries(&format!(
            "{flaky}\ndef test_a():\n    @flaky(max_runs=2)\n    def inner():\n        pass\n    assert a\n"
        )),
        vec![some("@flaky(max_runs=2)")]
    );
}

/// Tests that carry a retry marker, and parametrized tests, cost steps in proportion to
/// their number. Before #672 every test was read for each marker and every marker for
/// each test, and every name the file binds for each `parametrize`.
#[test]
fn marked_tests_cost_steps_in_proportion_to_their_number() {
    let (_, many) = in_proportion("retry markers", "tests/test_m.py", |n| {
        lines(n, |i| {
            format!("@pytest.mark.flaky(reruns=3)\ndef test_{i}():\n    assert f({i}) == {i}\n")
        })
    });
    assert!(many.tests.iter().all(|t| t.retries.is_some()));
    let (_, many) = in_proportion("parametrized tests", "tests/test_m.py", |n| {
        lines(n, |i| {
            format!(
                "@pytest.mark.parametrize('x', [1, 2])\ndef test_{i}(x):\n    assert f(x) == {i}\n"
            )
        })
    });
    assert!(many.tests.iter().all(|t| t.cases == Some(2)));
}

/// `pytest.mark.parametrize` is a case source under the name the file binds pytest by,
/// and not once the file binds that name to something else.
#[test]
fn parametrize_is_a_case_source_by_what_the_file_binds_its_name_to() {
    let cases = |head: &str| -> Vec<Option<usize>> {
        read(
            "tests/test_m.py",
            &format!(
                "{head}@pytest.mark.parametrize('x', [1, 2, 3])\ndef test_a(x):\n    assert x\n"
            ),
        )
        .0
        .tests
        .into_iter()
        .map(|t| t.cases)
        .collect()
    };
    assert_eq!(cases(""), vec![Some(3)]);
    assert_eq!(cases("import pytest\n"), vec![Some(3)]);
    assert_eq!(cases("def helper():\n    pass\n"), vec![Some(3)]);
    assert_eq!(cases("def pytest():\n    pass\n"), vec![None]);
    assert_eq!(cases("pytest = make()\n"), vec![None]);
    // Bound to something else, then to the module again, and the other way round.
    assert_eq!(cases("pytest = make()\nimport pytest\n"), vec![Some(3)]);
    assert_eq!(cases("import pytest\npytest = make()\n"), vec![None]);
    // Bound to something else, then to pytest's `mark`: the name is no longer bound to
    // something else, and `pytest.mark` under it is read as in a file that binds nothing.
    assert_eq!(
        cases("pytest = make()\nfrom pytest import mark as pytest\n"),
        vec![Some(3)]
    );
}

/// Handlers one inside another cost steps in proportion to their number. Before #672 a
/// Java `catch` body was walked to its end for each handler around it, and the text of
/// a handler that holds handlers was folded whole for each of them.
#[test]
fn handlers_one_inside_another_cost_steps_in_proportion_to_their_number() {
    let nest = |n: usize, open: &dyn Fn(usize) -> String, close: &str| {
        format!("{}{}", lines(n, open), close.repeat(n))
    };
    crate::deep_stack::on_deep_stack(|| {
        in_proportion("Java handlers", "src/test/java/MTest.java", |n| {
            format!(
                "class MTest {{\n @Test void t() {{\n{} }}\n}}\n",
                nest(
                    n,
                    &|i| format!(
                        "try {{ assertEquals({i}, f({i})); }} catch (AssertionError e) {{\n"
                    ),
                    "}\n"
                )
            )
        });
        in_proportion("Python handlers of a source file", "src/m.py", |n| {
            format!(
                "def run():\n{}{}pass\n",
                lines(n, |i| {
                    let indent = "    ".repeat(i + 1);
                    format!("{indent}try:\n{indent}    f({i})\n{indent}except Exception:\n")
                }),
                "    ".repeat(n + 1)
            )
        });
        in_proportion("JavaScript handlers of a source file", "src/m.js", |n| {
            format!(
                "function run() {{\n{}}}\n",
                nest(n, &|i| format!("try {{ f({i}); }} catch (e) {{\n"), "}\n")
            )
        });
    })
    .unwrap();
}

/// A Java handler that throws, fails or asserts anywhere in its body does not swallow,
/// and what a lambda or a class in the body does is not the handler's.
#[test]
fn a_java_handler_swallows_unless_its_body_throws_fails_or_asserts() {
    let caught_with = |handler: &str| {
        caught(
            "src/test/java/MTest.java",
            &format!(
                "class MTest {{\n    @Test\n    void t() {{\n        try {{\n            assertEquals(4, add(2, 2));\n        }} catch (AssertionError e) {{\n            {handler}\n        }}\n    }}\n}}\n"
            ),
        )
        .len()
    };
    for swallowing in [
        "",
        "log(e);",
        "Runnable r = () -> { throw new IllegalStateException(e); };",
        "new Thread() { public void run() { fail(); } };",
        "try { g(); } catch (Exception x) { log(x); }",
    ] {
        assert_eq!(caught_with(swallowing), 1, "{swallowing}");
    }
    for handling in [
        "throw e;",
        "fail(\"no\");",
        "assertNotNull(e);",
        "log(e); if (strict) { throw new IllegalStateException(e); }",
        "try { g(); } catch (Exception x) { throw x; }",
        "try { g(); } finally { assertTrue(done); }",
        "try { g(); } catch (Exception x) { try { h(); } catch (Exception y) { fail(); } }",
    ] {
        assert_eq!(caught_with(handling), 0, "{handling}");
    }
}
