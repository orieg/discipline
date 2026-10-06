//! How `assertion-reduction` resolves the helpers a test calls (#595): a helper of the
//! test's own file stands for the checks it holds and no more, a call through a type or an
//! object resolves to the same-file method of that name, helpers are followed three calls
//! deep in every pack, and a same-file helper wins over one of the same name in another
//! file. Suite methods (Go testify) are tests. Each shape is driven through the real
//! binary, with a control beside it that pins the other verdict.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const WEAKENED: &str = "Test Helper Function Weakened";
const DECREASED: &str = "Assertion Count Decreased In Existing Test";
const FATAL_WEAKENED: &str = "Fatal Assertions Weakened To Non-Fatal";
const LOOPED_NOTE: &str = "read as moved into same-file helpers that fail";
const MOVED_NOTE: &str = "read as moved into helper";
/// Packs where the fixture of the strength controls has no equality check to lose.
const NO_STRENGTH_TO_LOSE: &[&str] = &["python", "cpp"];

/// How one language spells a test file that holds its own helpers.
struct Lang {
    name: &'static str,
    path: &'static str,
    /// The `i`th assertion on the value `r`.
    check: fn(usize) -> String,
    /// A helper `name` with `body`, declared where a bare call in the test reaches it.
    helper: fn(&str, &str) -> String,
    /// A statement calling the helper `name` on `r`.
    call: fn(&str) -> String,
    /// A statement that checks nothing and calls no helper: with it, a helper that calls
    /// another is not a thin wrapper around it.
    other: &'static str,
    /// A failure exit of a helper under the condition given, and three conditions on
    /// `r`: an inequality comparison, an ordering comparison, and a truthiness test.
    exit: fn(&str) -> String,
    conditions: [&'static str; 3],
    /// The same call made once per element, in a loop of the test.
    looped_call: fn(&str) -> String,
    /// A helper body running `checks` once per element, in a loop of the helper.
    looped_body: fn(&str) -> String,
    /// One file: `top` declarations, `helpers` where the test reaches them, and one test
    /// with `body`.
    file: fn(&str, &str, &str) -> String,
    /// A type declared beside the test whose method the test calls through the type or an
    /// object: the type holding `methods`, one method `name` with `body`, and the call.
    receiver: Receiver,
}

struct Receiver {
    types: fn(&str, &str) -> String,
    method: fn(&str, &str) -> String,
    call: fn(&str, &str) -> String,
}

impl Lang {
    fn checks(&self, n: usize) -> String {
        (0..n).map(|i| (self.check)(i)).collect()
    }

    /// The test with `n` assertions of its own and no helper.
    fn inline(&self, n: usize) -> String {
        (self.file)("", "", &self.checks(n))
    }

    /// A type `ty` whose method `check` holds `n` assertions.
    fn checker(&self, ty: &str, n: usize) -> String {
        (self.receiver.types)(ty, &(self.receiver.method)("check", &self.checks(n)))
    }
}

/// A fixture spelled for a number: the `i`th assertion, a file holding `n` of them.
type Spelled = fn(usize) -> String;

fn indent(text: &str, by: &str) -> String {
    text.lines().map(|l| format!("{by}{l}\n")).collect()
}

fn langs() -> Vec<Lang> {
    vec![
        Lang {
            name: "python",
            path: "tests/test_api.py",
            check: |i| format!("    assert r.f{i} == {i}\n"),
            helper: |name, body| {
                let body = if body.is_empty() { "    pass\n" } else { body };
                format!("def {name}(r):\n{body}\n")
            },
            call: |name| format!("    {name}(r)\n"),
            other: "    log(r)\n",
            exit: |c| format!("    if {c}:\n        raise ValueError(r)\n"),
            conditions: ["r.f1 != 1", "r.f1 < 1", "not r.f1"],
            looped_call: |name| format!("    for x in rs:\n        {name}(x)\n"),
            looped_body: |checks| format!("    for r in items:\n{}", indent(checks, "    ")),
            file: |top, helpers, body| {
                format!("{top}{helpers}def test_create():\n    r = create()\n{body}")
            },
            receiver: Receiver {
                types: |ty, methods| format!("class {ty}:\n{methods}\n"),
                method: |name, body| {
                    let body = if body.is_empty() {
                        "        pass\n".to_string()
                    } else {
                        indent(body, "    ")
                    };
                    format!("    def {name}(self, r):\n{body}\n")
                },
                call: |ty, name| format!("    {ty}().{name}(r)\n"),
            },
        },
        Lang {
            name: "rust",
            path: "tests/api.rs",
            check: |i| format!("    assert_eq!(r.f{i}, {i});\n"),
            helper: |name, body| format!("fn {name}(r: &R) {{\n{body}}}\n\n"),
            call: |name| format!("    {name}(r);\n"),
            other: "    log(r);\n",
            exit: |c| format!("    if {c} {{\n        panic!(\"f1\");\n    }}\n"),
            conditions: ["r.f1 != 1", "r.f1 < 1", "r.bad"],
            looped_call: |name| format!("    for x in rs {{\n        {name}(x);\n    }}\n"),
            looped_body: |checks| format!("    for r in items {{\n{checks}    }}\n"),
            file: |top, helpers, body| {
                format!("{top}{helpers}#[test]\nfn create() {{\n    let r = &make();\n{body}}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("struct {ty};\n\nimpl {ty} {{\n{methods}}}\n\n"),
                method: |name, body| format!("    fn {name}(&self, r: &R) {{\n{body}    }}\n"),
                call: |ty, name| format!("    {ty}.{name}(r);\n"),
            },
        },
        Lang {
            name: "go",
            path: "api_test.go",
            check: |i| format!("\tif r.F{i} != {i} {{\n\t\tt.Error(\"f{i}\")\n\t}}\n"),
            helper: |name, body| {
                format!("func {name}(t *testing.T, r R) {{\n\tt.Helper()\n{body}}}\n\n")
            },
            call: |name| format!("\t{name}(t, r)\n"),
            other: "\tlog(r)\n",
            exit: |c| format!("\tif {c} {{\n\t\tpanic(\"f1\")\n\t}}\n"),
            conditions: ["r.F1 != 1", "r.F1 < 1", "r.Bad"],
            looped_call: |name| format!("\tfor _, x := range rs {{\n\t\t{name}(t, x)\n\t}}\n"),
            looped_body: |checks| format!("\tfor _, r := range items {{\n{checks}\t}}\n"),
            file: |top, helpers, body| {
                format!("package x\n\nimport \"testing\"\n\n{top}{helpers}func TestCreate(t *testing.T) {{\n\tr := create()\n{body}}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| {
                    format!("type {ty} struct{{}}\n\n{}", methods.replace("Checker", ty))
                },
                method: |name, body| {
                    format!("func (c Checker) {name}(t *testing.T, r R) {{\n{body}}}\n\n")
                },
                call: |ty, name| format!("\t{ty}{{}}.{name}(t, r)\n"),
            },
        },
        Lang {
            name: "typescript",
            path: "tests/api.test.ts",
            check: |i| format!("  expect(r.f{i}).toBe({i});\n"),
            helper: |name, body| format!("function {name}(r: R) {{\n{body}}}\n\n"),
            call: |name| format!("  {name}(r);\n"),
            other: "  log(r);\n",
            exit: |c| format!("  if ({c}) {{\n    throw new Error('f1');\n  }}\n"),
            conditions: ["r.f1 !== 1", "r.f1 < 1", "!r.f1"],
            looped_call: |name| format!("  for (const x of rs) {{\n    {name}(x);\n  }}\n"),
            looped_body: |checks| format!("  for (const r of items) {{\n{checks}  }}\n"),
            file: |top, helpers, body| {
                format!("import {{ test, expect }} from 'vitest';\n\n{top}{helpers}test('create', () => {{\n  const r = create();\n{body}}});\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("class {ty} {{\n{methods}}}\n\n"),
                method: |name, body| format!("  {name}(r: R) {{\n{body}  }}\n"),
                call: |ty, name| format!("  new {ty}().{name}(r);\n"),
            },
        },
        Lang {
            name: "java",
            path: "src/test/java/ApiTest.java",
            check: |i| format!("        assertEquals({i}, r.f{i});\n"),
            helper: |name, body| format!("    static void {name}(R r) {{\n{body}    }}\n\n"),
            call: |name| format!("        {name}(r);\n"),
            other: "        log(r);\n",
            exit: |c| {
                format!("        if ({c}) {{\n            throw new AssertionError(\"f1\");\n        }}\n")
            },
            conditions: ["r.f1 != 1", "r.f1 < 1", "r.bad"],
            looped_call: |name| {
                format!("        for (R x : rs) {{\n            {name}(x);\n        }}\n")
            },
            looped_body: |checks| format!("        for (R r : items) {{\n{checks}        }}\n"),
            file: |top, helpers, body| {
                format!("import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\n{top}class ApiTest {{\n{helpers}    @Test\n    void create() {{\n        R r = make();\n{body}    }}\n}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("class {ty} {{\n{methods}}}\n\n"),
                method: |name, body| format!("    static void {name}(R r) {{\n{body}    }}\n\n"),
                call: |ty, name| format!("        {ty}.{name}(r);\n"),
            },
        },
        Lang {
            name: "csharp",
            path: "tests/ApiTests.cs",
            check: |i| format!("        Assert.Equal({i}, r.F{i});\n"),
            helper: |name, body| {
                format!("    public static void {name}(R r)\n    {{\n{body}    }}\n\n")
            },
            call: |name| format!("        {name}(r);\n"),
            other: "        Log(r);\n",
            exit: |c| {
                format!("        if ({c})\n        {{\n            throw new Exception(\"f1\");\n        }}\n")
            },
            conditions: ["r.F1 != 1", "r.F1 < 1", "r.Bad"],
            looped_call: |name| {
                format!("        foreach (var x in rs)\n        {{\n            {name}(x);\n        }}\n")
            },
            looped_body: |checks| {
                format!("        foreach (var r in items)\n        {{\n{checks}        }}\n")
            },
            file: |top, helpers, body| {
                format!("using Xunit;\n\n{top}public class ApiTests\n{{\n{helpers}    [Fact]\n    public void Create()\n    {{\n        var r = Make();\n{body}    }}\n}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("public class {ty}\n{{\n{methods}}}\n\n"),
                method: |name, body| {
                    format!("    public static void {name}(R r)\n    {{\n{body}    }}\n\n")
                },
                call: |ty, name| format!("        {ty}.{name}(r);\n"),
            },
        },
        Lang {
            name: "kotlin",
            path: "src/test/kotlin/ApiTest.kt",
            check: |i| format!("    assertEquals({i}, r.f{i})\n"),
            helper: |name, body| format!("fun {name}(r: R) {{\n{body}}}\n\n"),
            call: |name| format!("    {name}(r)\n"),
            other: "    log(r)\n",
            exit: |c| format!("    if ({c}) {{\n        throw AssertionError(\"f1\")\n    }}\n"),
            conditions: ["r.f1 != 1", "r.f1 < 1", "r.bad"],
            looped_call: |name| format!("    for (x in rs) {{\n        {name}(x)\n    }}\n"),
            looped_body: |checks| format!("    for (r in items) {{\n{checks}    }}\n"),
            file: |top, helpers, body| {
                format!("import kotlin.test.Test\nimport kotlin.test.assertEquals\n\n{top}{helpers}class ApiTest {{\n    @Test\n    fun create() {{\n    val r = make()\n{body}    }}\n}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("object {ty} {{\n{methods}}}\n\n"),
                method: |name, body| format!("    fun {name}(r: R) {{\n{body}    }}\n"),
                call: |ty, name| format!("    {ty}.{name}(r)\n"),
            },
        },
        Lang {
            name: "swift",
            path: "tests/AppTests/ApiTests.swift",
            check: |i| format!("    XCTAssertEqual(r.f{i}, {i})\n"),
            helper: |name, body| format!("func {name}(_ r: R) {{\n{body}}}\n\n"),
            call: |name| format!("    {name}(r)\n"),
            other: "    log(r)\n",
            exit: |c| format!("    if {c} {{\n        fatalError(\"f1\")\n    }}\n"),
            conditions: ["r.f1 != 1", "r.f1 < 1", "r.bad"],
            looped_call: |name| format!("    for x in rs {{\n        {name}(x)\n    }}\n"),
            looped_body: |checks| format!("    for r in items {{\n{checks}    }}\n"),
            file: |top, helpers, body| {
                format!("import XCTest\n\n{top}{helpers}final class ApiTests: XCTestCase {{\n    func testCreate() {{\n    let r = make()\n{body}    }}\n}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("struct {ty} {{\n{methods}}}\n\n"),
                method: |name, body| format!("    func {name}(_ r: R) {{\n{body}    }}\n"),
                call: |ty, name| format!("    {ty}().{name}(r)\n"),
            },
        },
        Lang {
            name: "scala",
            path: "src/test/scala/ApiSpec.scala",
            check: |i| format!("    assert(r.f{i} == {i})\n"),
            helper: |name, body| format!("  def {name}(r: R): Unit = {{\n{body}  }}\n\n"),
            call: |name| format!("    {name}(r)\n"),
            other: "    log(r)\n",
            exit: |c| format!("    if ({c}) {{\n      throw new AssertionError(\"f1\")\n    }}\n"),
            conditions: ["r.f1 != 1", "r.f1 < 1", "r.bad"],
            looped_call: |name| format!("    for (x <- rs) {{\n      {name}(x)\n    }}\n"),
            looped_body: |checks| format!("    for (r <- items) {{\n{checks}    }}\n"),
            file: |top, helpers, body| {
                format!("{top}class ApiSpec extends AnyFunSuite {{\n{helpers}  test(\"create\") {{\n    val r = make()\n{body}  }}\n}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("object {ty} {{\n{methods}}}\n\n"),
                method: |name, body| format!("  def {name}(r: R): Unit = {{\n{body}  }}\n\n"),
                call: |ty, name| format!("    {ty}.{name}(r)\n"),
            },
        },
        Lang {
            name: "ruby",
            path: "test/api_test.rb",
            check: |i| format!("    assert_equal {i}, r.f{i}\n"),
            helper: |name, body| format!("  def {name}(r)\n{body}  end\n\n"),
            call: |name| format!("    {name}(r)\n"),
            other: "    log(r)\n",
            exit: |c| format!("    raise \"f1\" if {c}\n"),
            conditions: ["r.f1 != 1", "r.f1 < 1", "r.bad"],
            looped_call: |name| format!("    rs.each do |x|\n      {name}(x)\n    end\n"),
            looped_body: |checks| format!("    items.each do |r|\n{checks}    end\n"),
            file: |top, helpers, body| {
                format!("{top}class ApiTest < Minitest::Test\n{helpers}  def test_create\n    r = make\n{body}  end\nend\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("module {ty}\n{methods}end\n\n"),
                method: |name, body| format!("  def {name}(r)\n{body}  end\n\n"),
                call: |ty, name| format!("    {ty}.{name}(r)\n"),
            },
        },
        Lang {
            name: "php",
            path: "tests/ApiTest.php",
            check: |i| format!("        $this->assertSame({i}, $r->f{i});\n"),
            helper: |name, body| {
                format!("    protected function {name}($r): void\n    {{\n{body}    }}\n\n")
            },
            call: |name| format!("        $this->{name}($r);\n"),
            other: "        log($r);\n",
            exit: |c| {
                format!("        if ({c}) {{\n            throw new \\RuntimeException('f1');\n        }}\n")
            },
            conditions: ["$r->f1 !== 1", "$r->f1 < 1", "!$r->f1"],
            looped_call: |name| {
                format!(
                    "        foreach ($rs as $x) {{\n            $this->{name}($x);\n        }}\n"
                )
            },
            looped_body: |checks| {
                format!("        foreach ($items as $r) {{\n{checks}        }}\n")
            },
            file: |top, helpers, body| {
                format!("<?php\n\nuse PHPUnit\\Framework\\TestCase;\n\n{top}class ApiTest extends TestCase\n{{\n{helpers}    public function testCreate(): void\n    {{\n        $r = make();\n{body}    }}\n}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("class {ty} extends TestCase\n{{\n{methods}}}\n\n"),
                method: |name, body| {
                    format!("    public function {name}($r): void\n    {{\n{body}    }}\n\n")
                },
                call: |ty, name| format!("        (new {ty}())->{name}($r);\n"),
            },
        },
        Lang {
            name: "cpp",
            path: "tests/api_test.cc",
            check: |i| format!("  EXPECT_EQ(r.f{i}, {i});\n"),
            helper: |name, body| format!("void {name}(const R& r) {{\n{body}}}\n\n"),
            call: |name| format!("  {name}(r);\n"),
            other: "  Log(r);\n",
            exit: |c| format!("  if ({c}) {{\n    throw std::runtime_error(\"f1\");\n  }}\n"),
            conditions: ["r.f1 != 1", "r.f1 < 1", "r.bad"],
            looped_call: |name| format!("  for (const R& x : rs) {{\n    {name}(x);\n  }}\n"),
            looped_body: |checks| format!("  for (const R& r : items) {{\n{checks}  }}\n"),
            file: |top, helpers, body| {
                format!("#include <gtest/gtest.h>\n\n{top}{helpers}TEST(Api, Create) {{\n  R r = Make();\n{body}}}\n")
            },
            receiver: Receiver {
                types: |ty, methods| {
                    format!(
                        "struct {ty} {{\n  void check(const R& r);\n}};\n\n{}",
                        methods.replace("Checker::", &format!("{ty}::"))
                    )
                },
                method: |name, body| format!("void Checker::{name}(const R& r) {{\n{body}}}\n\n"),
                call: |ty, name| format!("  {ty}().{name}(r);\n"),
            },
        },
        Lang {
            name: "objc",
            path: "tests/ApiTests.m",
            check: |i| format!("    XCTAssertEqual(r.f{i}, {i});\n"),
            helper: |name, body| format!("void {name}(R *r) {{\n{body}}}\n\n"),
            call: |name| format!("    {name}(r);\n"),
            other: "    NSLog(@\"%@\", r);\n",
            exit: |c| format!("    if ({c}) {{\n        @throw [NSException new];\n    }}\n"),
            conditions: ["r.f1 != 1", "r.f1 < 1", "r.bad"],
            looped_call: |name| {
                format!("    for (int i = 0; i < n; i++) {{\n        {name}(rs[i]);\n    }}\n")
            },
            looped_body: |checks| format!("    for (int i = 0; i < n; i++) {{\n{checks}    }}\n"),
            file: |top, helpers, body| {
                format!("#import <XCTest/XCTest.h>\n\n{top}{helpers}@interface ApiTests : XCTestCase\n@end\n\n@implementation ApiTests\n\n- (void)testCreate {{\n    R *r = make();\n{body}}}\n\n@end\n")
            },
            receiver: Receiver {
                types: |ty, methods| format!("@implementation {ty}\n\n{methods}@end\n\n"),
                method: |name, body| format!("- (void){name}:(R *)r {{\n{body}}}\n\n"),
                call: |ty, name| format!("    [[{ty} new] {name}:r];\n"),
            },
        },
    ]
}

/// Commits `base` on `main`, then `head` on a branch with `body` in the commit message,
/// and runs `check`.
fn change(base: &[(&str, &str)], head: &[(&str, &str)], body: &str) -> Run {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", CONFIG_HEAD);
    for (path, content) in base {
        repo.write(path, content);
    }
    repo.commit("test: base");
    repo.git(&["checkout", "-q", "-B", "work"]);
    for (path, content) in head {
        repo.write(path, content);
    }
    repo.commit(&format!("refactor: change\n\n{body}"));
    repo.check(&["--base", "main"])
}

/// One file changed from `base` to `head`.
fn edit(path: &str, base: &str, head: &str) -> Run {
    change(&[(path, base)], &[(path, head)], "")
}

/// What `gate` reported: `(title, file)` of each violation.
fn reported_by(run: &Run, gate: &str) -> Vec<(String, String)> {
    run.violations(gate)
        .iter()
        .map(|v| {
            (
                v["title"].as_str().unwrap().to_string(),
                v["file"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn reported(run: &Run) -> Vec<(String, String)> {
    reported_by(run, "assertion-reduction")
}

fn messages(run: &Run, gate: &str) -> Vec<String> {
    run.violations(gate)
        .iter()
        .map(|v| v["message"].as_str().unwrap_or("").to_string())
        .collect()
}

fn notes(run: &Run) -> String {
    run.outcome("assertion-reduction")["notes"].to_string()
}

/// Collects one line per case whose verdict is not the expected one, so one run names
/// every language a shape fails in. Each test asserts the list is empty itself.
#[derive(Default)]
struct Verdicts(Vec<String>);

impl Verdicts {
    fn clean(&mut self, case: &str, run: &Run) {
        let got = reported(run);
        if run.code != 0 || !got.is_empty() {
            self.0.push(format!(
                "{case}: expected nothing reported, got exit {} {got:?}",
                run.code
            ));
        }
    }

    /// Nothing reported, and the gate's notes say why.
    fn clean_with_note(&mut self, case: &str, run: &Run, note: &str) {
        self.clean(case, run);
        let notes = notes(run);
        if !notes.contains(note) {
            self.0.push(format!("{case}: no `{note}` in {notes}"));
        }
    }

    fn reports(&mut self, case: &str, run: &Run, title: &str, file: &str) {
        let got = reported(run);
        let expected = vec![(title.to_string(), file.to_string())];
        if run.code != 1 || got != expected {
            self.0.push(format!(
                "{case}: expected {expected:?}, got exit {} {got:?}",
                run.code
            ));
        }
    }

    /// The one violation is a count drop of the test from `from` to `to`.
    fn drops(&mut self, case: &str, run: &Run, file: &str, from: usize, to: usize) {
        self.reports(case, run, DECREASED, file);
        let got = messages(run, "assertion-reduction").join(" / ");
        let want = format!("effective assertions dropped from {from} to {to}");
        if !got.contains(&want) {
            self.0.push(format!("{case}: no `{want}` in `{got}`"));
        }
    }

    fn done(self) {
        assert!(self.0.is_empty(), "\n{}\n", self.0.join("\n"));
    }
}

// ---- a same-file helper stands for what it holds ------------------------------------------

/// Three assertions replaced by one call to a new same-file helper that holds one were
/// excused whole, in every pack: the growth in calls to helpers that fail was read as a
/// refactor whatever the helper held.
#[test]
fn a_same_file_helper_holding_fewer_checks_than_were_dropped_is_reported() {
    let mut v = Verdicts::default();
    for l in langs() {
        let head = (l.file)("", &(l.helper)("check", &l.checks(1)), &(l.call)("check"));
        v.drops(l.name, &edit(l.path, &l.inline(3), &head), l.path, 3, 1);
    }
    v.done();
}

/// Controls: a helper that holds the three assertions, and a helper that holds one and is
/// called three times, account for all that was dropped.
#[test]
fn a_same_file_helper_holding_what_was_dropped_reports_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let whole = (l.file)("", &(l.helper)("check", &l.checks(3)), &(l.call)("check"));
        v.clean(
            &format!("{} one call", l.name),
            &edit(l.path, &l.inline(3), &whole),
        );
        let thrice = (l.file)(
            "",
            &(l.helper)("check", &l.checks(1)),
            &(l.call)("check").repeat(3),
        );
        v.clean(
            &format!("{} three calls", l.name),
            &edit(l.path, &l.inline(3), &thrice),
        );
    }
    v.done();
}

/// The documented case stays: one check in a loop stands for many inline assertions. A
/// helper called in a loop of the test, and a helper whose check sits in a loop of its
/// own, excuse the drop, with the note.
#[test]
fn a_same_file_helper_that_checks_in_a_loop_still_excuses_the_drop() {
    let mut v = Verdicts::default();
    for l in langs() {
        let called_in_loop = (l.file)(
            "",
            &(l.helper)("check", &l.checks(1)),
            &(l.looped_call)("check"),
        );
        v.clean_with_note(
            &format!("{} call in a loop", l.name),
            &edit(l.path, &l.inline(3), &called_in_loop),
            LOOPED_NOTE,
        );
        let loops_itself = (l.file)(
            "",
            &(l.helper)("check", &(l.looped_body)(&l.checks(1))),
            &(l.call)("check"),
        );
        v.clean_with_note(
            &format!("{} loop in the helper", l.name),
            &edit(l.path, &l.inline(3), &loops_itself),
            LOOPED_NOTE,
        );
    }
    v.done();
}

/// A loop that holds no check does not make a helper a looping one: the helper's one
/// assertion runs once.
#[test]
fn a_loop_beside_the_helpers_check_does_not_excuse_the_drop() {
    let mut v = Verdicts::default();
    for l in langs() {
        let body = format!("{}{}", (l.looped_body)(""), l.checks(1));
        let head = (l.file)("", &(l.helper)("check", &body), &(l.call)("check"));
        v.drops(l.name, &edit(l.path, &l.inline(3), &head), l.path, 3, 1);
    }
    v.done();
}

/// An equality assertion rewritten in a same-file helper as a failure exit under an
/// equality comparison (`if r.f1 != 1 { panic!() }`) is still an equality check: the
/// count holds and nothing is weakened. A pack counts a failure exit as a check and
/// never as an equality one, so this move read as a strength drop.
#[test]
fn an_equality_check_rewritten_as_a_guarded_failure_exit_is_not_weakened() {
    let mut v = Verdicts::default();
    for l in langs() {
        let body = format!("{}{}", l.checks(1), (l.exit)(l.conditions[0]));
        let head = (l.file)("", &(l.helper)("check", &body), &(l.call)("check"));
        v.clean(l.name, &edit(l.path, &l.inline(2), &head));
    }
    v.done();
}

/// Controls: an exit under an ordering comparison, or under a truthiness test, checks
/// something else than the equality the test dropped, and is reported as weakened. The
/// packs named below count their plain assertion as no equality check, or count the exit
/// as one, so the fixture has no strength to lose there.
#[test]
fn a_failure_exit_under_another_condition_does_not_stand_for_an_equality_check() {
    let mut v = Verdicts::default();
    for l in langs() {
        if NO_STRENGTH_TO_LOSE.contains(&l.name) {
            continue;
        }
        for condition in &l.conditions[1..] {
            let body = format!("{}{}", l.checks(1), (l.exit)(condition));
            let head = (l.file)("", &(l.helper)("check", &body), &(l.call)("check"));
            let run = edit(l.path, &l.inline(2), &head);
            let case = format!("{} `{condition}`", l.name);
            v.reports(&case, &run, DECREASED, l.path);
            let message = messages(&run, "assertion-reduction").join(" / ");
            if !message.contains("equality / pattern assertions dropped from 2 to 1") {
                v.0.push(format!("{case}: {message}"));
            }
        }
    }
    v.done();
}

// ---- helpers are followed three calls deep ------------------------------------------------

/// `check` calls `check_body`, which holds the assertions: a lossless move through two
/// same-file helpers was reported in every pack but Python and C/C++.
#[test]
fn a_move_through_two_same_file_helpers_reports_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let chain = |held: usize| {
            let helpers = format!(
                "{}{}",
                (l.helper)("check", &format!("{}{}", l.other, (l.call)("check_body"))),
                (l.helper)("check_body", &l.checks(held))
            );
            (l.file)("", &helpers, &(l.call)("check"))
        };
        v.clean(l.name, &edit(l.path, &l.inline(3), &chain(3)));
        // Control: the second helper holds two of the three.
        v.reports(
            &format!("{} two of three", l.name),
            &edit(l.path, &l.inline(3), &chain(2)),
            DECREASED,
            l.path,
        );
    }
    v.done();
}

/// Three calls from the test are followed and a fourth is not; two helpers that call each
/// other are each counted once.
#[test]
fn helpers_are_followed_three_calls_deep_and_a_cycle_stops() {
    let mut v = Verdicts::default();
    for l in langs() {
        let chain = |names: &[&str]| {
            let mut helpers = String::new();
            for (i, name) in names.iter().enumerate() {
                let body = match names.get(i + 1) {
                    Some(next) => format!("{}{}", l.other, (l.call)(next)),
                    None => l.checks(3),
                };
                helpers.push_str(&(l.helper)(name, &body));
            }
            (l.file)("", &helpers, &(l.call)(names[0]))
        };
        v.clean(
            &format!("{} three deep", l.name),
            &edit(l.path, &l.inline(3), &chain(&["one", "two", "three"])),
        );
        v.reports(
            &format!("{} four deep", l.name),
            &edit(
                l.path,
                &l.inline(3),
                &chain(&["one", "two", "three", "four"]),
            ),
            DECREASED,
            l.path,
        );
        // `check` and `check_body` call each other: what `check_body` holds counts once.
        let cycle = |held: usize| {
            let helpers = format!(
                "{}{}",
                (l.helper)("check", &format!("{}{}", l.other, (l.call)("check_body"))),
                (l.helper)(
                    "check_body",
                    &format!("{}{}", l.checks(held), (l.call)("check"))
                )
            );
            (l.file)("", &helpers, &(l.call)("check"))
        };
        v.clean(
            &format!("{} cycle", l.name),
            &edit(l.path, &l.inline(3), &cycle(3)),
        );
        v.reports(
            &format!("{} cycle holding two", l.name),
            &edit(l.path, &l.inline(3), &cycle(2)),
            DECREASED,
            l.path,
        );
    }
    v.done();
}

// ---- a call through a type or an object ---------------------------------------------------

/// `Checker.check(r)`: a lossless move into a method of a type declared beside the test,
/// called through the type or an object, was reported where the pack reads only bare and
/// `this` calls.
#[test]
fn a_move_behind_a_receiver_call_to_a_same_file_method_reports_nothing() {
    let mut v = Verdicts::default();
    for l in langs() {
        let head = (l.file)(
            &l.checker("Checker", 3),
            "",
            &(l.receiver.call)("Checker", "check"),
        );
        v.clean(l.name, &edit(l.path, &l.inline(3), &head));
    }
    v.done();
}

/// Controls: the method holds one of the three assertions; and of two same-file methods
/// of that name the one that checks least counts, because the receiver's type is not
/// known.
#[test]
fn a_receiver_call_stands_for_what_the_least_checking_method_holds() {
    let mut v = Verdicts::default();
    for l in langs() {
        let call = (l.receiver.call)("Checker", "check");
        let fewer = (l.file)(&l.checker("Checker", 1), "", &call);
        v.reports(
            &format!("{} one of three", l.name),
            &edit(l.path, &l.inline(3), &fewer),
            DECREASED,
            l.path,
        );
        // The second type's `check` does something and checks nothing.
        let unchecked = (l.receiver.types)("Other", &(l.receiver.method)("check", l.other));
        let two = format!("{}{unchecked}", l.checker("Checker", 3));
        v.reports(
            &format!("{} two of one name", l.name),
            &edit(l.path, &l.inline(3), &(l.file)(&two, "", &call)),
            DECREASED,
            l.path,
        );
    }
    v.done();
}

/// A receiver call the base test already made stands for nothing new: the assertions the
/// test drops beside it are a drop.
#[test]
fn a_receiver_call_the_test_already_made_stands_for_no_dropped_assertion() {
    let mut v = Verdicts::default();
    for l in langs() {
        let call = (l.receiver.call)("Checker", "check");
        let with = |own: usize| {
            (l.file)(
                &l.checker("Checker", 3),
                "",
                &format!("{call}{}", l.checks(own)),
            )
        };
        v.reports(l.name, &edit(l.path, &with(3), &with(0)), DECREASED, l.path);
    }
    v.done();
}

// ---- a same-file helper wins over one of the same name elsewhere --------------------------

const LIB_BASE: &str = "pub fn make() -> R { R::default() }\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn create() {\n        let r = &make();\n        assert_eq!(r.f0, 0);\n        assert_eq!(r.f1, 1);\n        assert_eq!(r.f2, 2);\n    }\n}\n";

fn lib_head(helper: &str) -> String {
    format!("pub fn make() -> R {{ R::default() }}\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n{helper}    #[test]\n    fn create() {{\n        let r = &make();\n        check(r);\n    }}\n}}\n")
}

const SUPPORT_CHECK: &str = "pub fn check(r: &R) {\n    assert_eq!(r.f0, 0);\n    assert_eq!(r.f1, 1);\n    assert_eq!(r.f2, 2);\n}\n";

/// A unit test under `src/` drops its three assertions for a call to a same-file `check`
/// that checks nothing, while the change adds an unrelated `check` holding three to a
/// test-support file: the call was credited with the other file's helper.
#[test]
fn a_same_file_helper_is_not_credited_with_a_same_named_helper_of_another_file() {
    let local = "    fn check(r: &R) {\n        let _ = r;\n    }\n\n";
    let run = change(
        &[("src/lib.rs", LIB_BASE)],
        &[
            ("src/lib.rs", &lib_head(local)),
            ("tests/common/mod.rs", SUPPORT_CHECK),
        ],
        "",
    );
    assert_eq!(
        reported(&run),
        vec![(DECREASED.to_string(), "src/lib.rs".to_string())],
        "{}",
        run.stdout
    );
    assert!(!notes(&run).contains(MOVED_NOTE), "{}", notes(&run));
}

/// Control: with no `check` in the calling file, the call names the helper the change
/// adds to the test-support file, which holds what the test dropped.
#[test]
fn a_call_with_no_same_file_helper_is_credited_with_the_helper_of_another_file() {
    let run = change(
        &[("src/lib.rs", LIB_BASE)],
        &[
            ("src/lib.rs", &lib_head("")),
            ("tests/common/mod.rs", SUPPORT_CHECK),
        ],
        "",
    );
    assert_eq!(reported(&run), Vec::new(), "{}", run.stdout);
    assert!(notes(&run).contains(MOVED_NOTE), "{}", notes(&run));
}

// ---- Go testify suites --------------------------------------------------------------------

const SUITE_PATH: &str = "calc_test.go";

/// A testify suite file: `methods` of `Suite`, and the runner function.
fn suite_file(methods: &str) -> String {
    format!("package calc\n\nimport (\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/assert\"\n\t\"github.com/stretchr/testify/require\"\n\t\"github.com/stretchr/testify/suite\"\n)\n\ntype Suite struct {{\n\tsuite.Suite\n}}\n\n{methods}func TestSuite(t *testing.T) {{\n\tsuite.Run(t, new(Suite))\n}}\n")
}

fn suite_method(name: &str, body: &str) -> String {
    format!("func (s *Suite) {name}() {{\n{body}}}\n\n")
}

/// The reproduction of #595: a suite method going from three `s.Equal` calls to one gave
/// no finding. The method was read as a helper named `Suite.TestAdd`, and no assertion
/// made on the suite was counted. Every way testify spells an assertion in a suite is
/// counted now.
#[test]
fn a_go_suite_method_that_loses_assertions_is_reported() {
    let forms: [(&str, Spelled); 8] = [
        ("s.Equal", |i| format!("\ts.Equal({i}, Add({i}, 0))\n")),
        ("s.Require().NoError", |i| {
            format!("\ts.Require().NoError(run({i}))\n")
        }),
        ("s.Assert().True", |i| {
            format!("\ts.Assert().True(ok({i}))\n")
        }),
        ("s.NoError", |i| format!("\ts.NoError(run({i}))\n")),
        ("s.Len", |i| format!("\ts.Len(items({i}), {i})\n")),
        ("s.Equalf", |i| {
            format!("\ts.Equalf({i}, Add({i}, 0), \"case %d\", {i})\n")
        }),
        ("assert.Equal(s.T(), ..)", |i| {
            format!("\tassert.Equal(s.T(), {i}, Add({i}, 0))\n")
        }),
        ("require.Equal(s.T(), ..)", |i| {
            format!("\trequire.Equal(s.T(), {i}, Add({i}, 0))\n")
        }),
    ];
    let mut v = Verdicts::default();
    for (name, form) in forms {
        let body = |n: usize| -> String { (1..=n).map(form).collect() };
        let run = edit(
            SUITE_PATH,
            &suite_file(&suite_method("TestAdd", &body(3))),
            &suite_file(&suite_method("TestAdd", &body(1))),
        );
        v.drops(name, &run, SUITE_PATH, 3, 1);
        let message = messages(&run, "assertion-reduction").join(" / ");
        if !message.starts_with("Test `Suite.TestAdd`: ") {
            v.0.push(format!("{name}: named `{message}`"));
        }
        // Control: the same file with a comment added loses nothing.
        let same = suite_file(&suite_method("TestAdd", &body(3)));
        v.clean(
            &format!("{name} unchanged"),
            &edit(SUITE_PATH, &same, &format!("{same}// reviewed\n")),
        );
    }
    v.done();
}

/// The name a suite test is reported under is the subject its directive takes.
#[test]
fn a_go_suite_test_is_lifted_by_a_directive_naming_type_and_method() {
    let body = |n: usize| -> String {
        (1..=n)
            .map(|i| format!("\ts.Equal({i}, Add({i}, 0))\n"))
            .collect()
    };
    let base = suite_file(&suite_method("TestAdd", &body(3)));
    let head = suite_file(&suite_method("TestAdd", &body(1)));
    let run = change(
        &[(SUITE_PATH, &base)],
        &[(SUITE_PATH, &head)],
        "allow-assertion-drop: Suite.TestAdd two cases folded into the table test",
    );
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(reported(&run), Vec::new(), "{}", run.stdout);
    assert_eq!(run.json()["overrides"], 1, "{}", run.stdout);
    // Control: a directive naming another test lifts nothing.
    let run = change(
        &[(SUITE_PATH, &base)],
        &[(SUITE_PATH, &head)],
        "allow-assertion-drop: Suite.TestOther two cases folded into the table test",
    );
    assert_eq!(
        reported(&run),
        vec![(DECREASED.to_string(), SUITE_PATH.to_string())],
        "{}",
        run.stdout
    );
}

/// `s.Require()` assertions stop the test and `s.Assert()` / `s.X` ones do not, as
/// `require.X` and `assert.X` do: the same count made non-fatal is reported.
#[test]
fn go_suite_require_assertions_made_non_fatal_are_reported() {
    let body = |call: &str| -> String {
        (1..=2)
            .map(|i| format!("\t{call}({i}, Add({i}, 0))\n"))
            .collect()
    };
    let file = |call: &str| suite_file(&suite_method("TestAdd", &body(call)));
    let mut v = Verdicts::default();
    for weaker in ["s.Equal", "s.Assert().Equal"] {
        let run = edit(SUITE_PATH, &file("s.Require().Equal"), &file(weaker));
        // A warning: the run passes and the finding is reported.
        let got = reported(&run);
        if got != vec![(FATAL_WEAKENED.to_string(), SUITE_PATH.to_string())] {
            v.0.push(format!("{weaker}: {got:?}"));
        }
        let message = messages(&run, "assertion-reduction").join(" / ");
        if !message.contains("`Suite.TestAdd`: fatal assertions dropped from 2 to 0") {
            v.0.push(format!("{weaker}: {message}"));
        }
    }
    // Control: the other direction strengthens the test.
    v.clean(
        "made fatal",
        &edit(SUITE_PATH, &file("s.Equal"), &file("s.Require().Equal")),
    );
    v.done();
}

/// A new suite test with no assertion is vacuous; the suite's lifecycle methods, and a
/// `Test*` method of a type that embeds no suite, are not tests.
#[test]
fn a_new_go_suite_test_is_read_by_vacuous_tests_and_lifecycle_methods_are_not() {
    let base = suite_file(&suite_method("TestAdd", "\ts.Equal(2, Add(1, 1))\n"));
    let with = |method: &str| format!("{base}\n{method}");
    let vacuous = |run: &Run| reported_by(run, "vacuous-tests");
    let empty = "\t_ = Add(1, 1)\n";

    let run = edit(SUITE_PATH, &base, &with(&suite_method("TestSub", empty)));
    assert_eq!(
        vacuous(&run),
        vec![("Vacuous Test Added".to_string(), SUITE_PATH.to_string())],
        "{}",
        run.stdout
    );
    assert!(
        messages(&run, "vacuous-tests")[0].contains("`Suite.TestSub`"),
        "{}",
        run.stdout
    );

    // Controls: a suite test that asserts, the lifecycle methods testify calls, a method
    // with a parameter (testify runs none), and a method of a plain type.
    let mut clean = vec![with(&suite_method("TestSub", "\ts.Equal(0, Sub(1, 1))\n"))];
    for hook in [
        "SetupSuite",
        "SetupTest",
        "TearDownTest",
        "TearDownSuite",
        "SetupSubTest",
    ] {
        clean.push(with(&suite_method(hook, empty)));
    }
    clean.push(with(
        "func (s *Suite) BeforeTest(suiteName, testName string) {\n\t_ = Add(1, 1)\n}\n",
    ));
    clean.push(with(
        "func (s *Suite) TestWith(n int) {\n\t_ = Add(n, 1)\n}\n",
    ));
    clean.push(with(
        "type Plain struct{ n int }\n\nfunc (p *Plain) TestConnection() {\n\t_ = Add(p.n, 1)\n}\n",
    ));
    for head in clean {
        let run = edit(SUITE_PATH, &base, &head);
        assert_eq!(vacuous(&run), Vec::new(), "{head}\n{}", run.stdout);
        assert_eq!(run.code, 0, "{head}\n{}", run.stdout);
    }
}

/// `s.T().Skip()` in a suite test is a skip of that test.
#[test]
fn a_skip_added_to_a_go_suite_test_is_read_by_ignored_tests() {
    let base = suite_file(&suite_method("TestAdd", "\ts.Equal(2, Add(1, 1))\n"));
    let head = suite_file(&suite_method(
        "TestAdd",
        "\ts.T().Skip(\"flaky\")\n\ts.Equal(2, Add(1, 1))\n",
    ));
    let run = edit(SUITE_PATH, &base, &head);
    assert_eq!(
        reported_by(&run, "ignored-tests"),
        vec![("Existing Test Skipped".to_string(), SUITE_PATH.to_string())],
        "{}",
        run.stdout
    );
    assert!(
        messages(&run, "ignored-tests")[0].contains("Suite.TestAdd"),
        "{}",
        run.stdout
    );
}

/// The static count of `test-floor` counts suite tests: deleting two of three is a drop
/// in the count. Before, the file counted its runner function alone on both sides.
#[test]
fn deleted_go_suite_tests_lower_the_static_test_count() {
    let method = |name: &str| suite_method(name, "\ts.Equal(2, Add(1, 1))\n");
    let three = format!("{}{}{}", method("TestA"), method("TestB"), method("TestC"));
    let go_mod = "module example.com/calc\n\ngo 1.22\n";
    let run = change(
        &[("go.mod", go_mod), (SUITE_PATH, &suite_file(&three))],
        &[(SUITE_PATH, &suite_file(&method("TestA")))],
        "",
    );
    assert_eq!(
        reported_by(&run, "test-floor")
            .iter()
            .map(|(title, _)| title.as_str())
            .collect::<Vec<_>>(),
        vec!["Test Count Below Floor"],
        "{}",
        run.stdout
    );
    // The fixture repository holds three tests of its own. With them: the runner
    // function and three suite tests, then the runner function and one.
    let message = messages(&run, "test-floor").join(" / ");
    assert!(
        message.contains("Workspace test count (5) dropped below base ref count (7)"),
        "{message}"
    );
}

/// The struct is declared in another file of the package. A type that this file gives a
/// method of the shape testify runs (`Test*`, no parameter, no result) is read as a
/// suite; a type with no such method is not, and its methods stay helpers.
#[test]
fn a_go_suite_declared_in_another_file_is_read_by_the_shape_of_its_test_methods() {
    let file = |methods: &str| format!("package calc\n\n{methods}");
    let body = |n: usize| -> String {
        (1..=n)
            .map(|i| format!("\ts.Equal({i}, Add({i}, 0))\n"))
            .collect()
    };
    let run = edit(
        "add_test.go",
        &file(&suite_method("TestAdd", &body(3))),
        &file(&suite_method("TestAdd", &body(1))),
    );
    let mut v = Verdicts::default();
    v.drops("declared elsewhere", &run, "add_test.go", 3, 1);
    // Control: a type declared in the file that embeds no suite. Its `Test*` method is a
    // helper, and a call on its receiver is not an assertion.
    let plain = |n: usize| {
        file(&format!(
            "type Suite struct{{ n int }}\n\n{}",
            suite_method("TestAdd", &body(n))
        ))
    };
    v.clean(
        "no suite embedded",
        &edit("add_test.go", &plain(3), &plain(1)),
    );
    v.done();
}

/// A suite embedded through a second struct of the file is a suite.
#[test]
fn a_go_suite_embedded_through_a_struct_of_the_file_is_read() {
    let file = |n: usize| {
        let body: String = (1..=n)
            .map(|i| format!("\ts.Equal({i}, Add({i}, 0))\n"))
            .collect();
        format!("package calc\n\nimport \"github.com/stretchr/testify/suite\"\n\ntype Base struct {{\n\t*suite.Suite\n\tdb DB\n}}\n\ntype ApiSuite struct {{\n\tBase\n}}\n\nfunc (s *ApiSuite) TestAdd() {{\n{body}}}\n")
    };
    let mut v = Verdicts::default();
    v.drops(
        "embedded twice",
        &edit("api_test.go", &file(3), &file(1)),
        "api_test.go",
        3,
        1,
    );
    v.done();
}

/// A helper method of the suite counts the assertions it makes on the suite, so a
/// lifecycle method or a shared check that loses them is reported as a helper is.
#[test]
fn a_go_suite_helper_method_that_loses_its_assertions_is_reported() {
    let file = |setup: &str| {
        suite_file(&format!(
            "{}{}",
            suite_method("SetupTest", setup),
            suite_method("TestAdd", "\ts.Equal(2, Add(1, 1))\n")
        ))
    };
    let run = edit(
        SUITE_PATH,
        &file("\ts.Require().NoError(open())\n"),
        &file("\t_ = open()\n"),
    );
    assert_eq!(
        reported(&run),
        vec![(WEAKENED.to_string(), SUITE_PATH.to_string())],
        "{}",
        run.stdout
    );
    assert!(
        messages(&run, "assertion-reduction")[0].contains("`Suite.SetupTest`"),
        "{}",
        run.stdout
    );
}

// ---- tests that are methods of a class, in the other packs --------------------------------

/// Pin: the packs whose tests are methods of a class a runner collects read them as tests
/// already. Each loses two of three assertions.
#[test]
fn a_test_method_of_a_class_that_loses_assertions_is_reported() {
    let cases: [(&str, &str, Spelled); 7] = [
        ("python unittest", "tests/test_calc.py", |n| {
            let body: String = (0..n)
                .map(|i| format!("        self.assertEqual(add({i}, 0), {i})\n"))
                .collect();
            format!("import unittest\n\nclass CalcTest(unittest.TestCase):\n    def test_add(self):\n{body}")
        }),
        ("java junit", "src/test/java/CalcTest.java", |n| {
            let body: String = (0..n)
                .map(|i| format!("        assertEquals({i}, Calc.add({i}, 0));\n"))
                .collect();
            format!("import org.junit.jupiter.api.Test;\nimport static org.junit.jupiter.api.Assertions.*;\n\nclass CalcTest {{\n    @Test\n    void add() {{\n{body}    }}\n}}\n")
        }),
        ("csharp xunit", "tests/CalcTests.cs", |n| {
            let body: String = (0..n)
                .map(|i| format!("        Assert.Equal({i}, Calc.Add({i}, 0));\n"))
                .collect();
            format!("using Xunit;\n\npublic class CalcTests\n{{\n    [Fact]\n    public void Add()\n    {{\n{body}    }}\n}}\n")
        }),
        ("kotlin junit", "src/test/kotlin/CalcTest.kt", |n| {
            let body: String = (0..n)
                .map(|i| format!("        assertEquals({i}, add({i}, 0))\n"))
                .collect();
            format!("import kotlin.test.Test\nimport kotlin.test.assertEquals\n\nclass CalcTest {{\n    @Test\n    fun add() {{\n{body}    }}\n}}\n")
        }),
        ("ruby minitest", "test/calc_test.rb", |n| {
            let body: String = (0..n)
                .map(|i| format!("    assert_equal {i}, add({i}, 0)\n"))
                .collect();
            format!("class CalcTest < Minitest::Test\n  def test_add\n{body}  end\nend\n")
        }),
        ("swift xctest", "tests/CalcTests/CalcTests.swift", |n| {
            let body: String = (0..n)
                .map(|i| format!("        XCTAssertEqual(add({i}, 0), {i})\n"))
                .collect();
            format!("import XCTest\n\nfinal class CalcTests: XCTestCase {{\n    func testAdd() {{\n{body}    }}\n}}\n")
        }),
        ("php phpunit", "tests/CalcTest.php", |n| {
            let body: String = (0..n)
                .map(|i| format!("        $this->assertSame({i}, add({i}, 0));\n"))
                .collect();
            format!("<?php\n\nuse PHPUnit\\Framework\\TestCase;\n\nclass CalcTest extends TestCase\n{{\n    public function testAdd(): void\n    {{\n{body}    }}\n}}\n")
        }),
    ];
    let mut v = Verdicts::default();
    for (name, path, file) in cases {
        v.drops(name, &edit(path, &file(3), &file(1)), path, 3, 1);
    }
    v.done();
}

/// Pin of a limit: a gocheck suite method (`func (s *S) TestX(c *C)`, run by
/// `check.Suite`) is not read as a test, and `c.Assert` is not counted, so a method that
/// loses its checks is not reported.
#[test]
fn a_gocheck_suite_method_is_not_read_as_a_test() {
    let file = |n: usize| {
        let body: String = (1..=n)
            .map(|i| format!("\tc.Assert(Add({i}, 0), Equals, {i})\n"))
            .collect();
        format!("package calc\n\nimport (\n\t\"testing\"\n\n\t. \"gopkg.in/check.v1\"\n)\n\nfunc Test(t *testing.T) {{ TestingT(t) }}\n\ntype S struct{{}}\n\nvar _ = Suite(&S{{}})\n\nfunc (s *S) TestAdd(c *C) {{\n{body}}}\n")
    };
    let run = edit(SUITE_PATH, &file(3), &file(1));
    assert_eq!(reported(&run), Vec::new(), "{}", run.stdout);
    assert_eq!(run.code, 0, "{}", run.stdout);
}
