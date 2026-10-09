//! Conditional skips in C#, Ruby, PHP, Swift, Scala, C / C++ and Objective-C, and what the
//! `ignored-tests` gate reports for each (#629).
//!
//! A skip call under an `if`, `unless`, `guard` or ternary is a conditional skip, and so
//! is a call or attribute that takes its condition (`Assume.That(..)`, `XCTSkipIf(..)`,
//! `assume(..)`, `.disabled(if: ..)`). A condition that reads a CI variable through the
//! language's environment read and holds in CI makes it a CI-conditional skip; one that
//! holds only outside CI, or reads no CI variable, is a note. A skip in the `else`
//! branch runs under the negated condition.
//!
//! Each case drives the real binary over a base side whose test runs and a head side
//! whose test carries the skip. The sources under test are fixtures built by this file,
//! not tests of this suite.

mod common;
use common::Repo;

/// What `ignored-tests` reports for one change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Want {
    /// One `Test Conditionally Skipped` at `error`, exit 1: the test stops running in CI.
    CiSkip,
    /// One `Test Conditionally Skipped` at `note`, exit 0.
    Note,
    /// One `Existing Test Skipped` at `error`, exit 1: an unconditional skip.
    Unconditional,
    /// No finding, exit 0.
    Nothing,
}

struct Case {
    name: String,
    path: &'static str,
    base: String,
    head: String,
    config: &'static str,
    want: Want,
    /// Text the message of the finding carries, or `""`.
    message: &'static str,
}

struct Seen {
    code: i32,
    findings: Vec<(String, String, String)>,
}

fn run_change(case: &Case) -> Seen {
    let repo = Repo::new();
    let mut files = vec![(case.path, case.base.as_str())];
    if !case.config.is_empty() {
        files.push(("discipline.toml", case.config));
    }
    repo.commit_base_files(&files, "test: base suite");
    repo.write(case.path, &case.head);
    repo.commit("test: change the test");
    let run = repo.check(&[]);
    let findings = run
        .violations("ignored-tests")
        .iter()
        .map(|v| {
            (
                v["title"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
                v["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    Seen {
        code: run.code,
        findings,
    }
}

fn matches_want(seen: &Seen, want: Want) -> bool {
    let one = |title: &str, severity: &str| {
        seen.findings.len() == 1 && seen.findings[0].0 == title && seen.findings[0].1 == severity
    };
    match want {
        Want::CiSkip => one("Test Conditionally Skipped", "error") && seen.code == 1,
        Want::Note => one("Test Conditionally Skipped", "note") && seen.code == 0,
        Want::Unconditional => one("Existing Test Skipped", "error") && seen.code == 1,
        Want::Nothing => seen.findings.is_empty() && seen.code == 0,
    }
}

/// The cases that differ from what they want, one line each.
fn differing(cases: &[Case]) -> Vec<String> {
    let mut wrong = Vec::new();
    for case in cases {
        let seen = run_change(case);
        let message_ok = case.message.is_empty()
            || seen
                .findings
                .first()
                .is_some_and(|f| f.2.contains(case.message));
        if !matches_want(&seen, case.want) || !message_ok {
            wrong.push(format!(
                "{}: want {:?} `{}`, got exit {} {:?}",
                case.name, case.want, case.message, seen.code, seen.findings
            ));
        }
    }
    wrong
}

fn check(cases: Vec<Case>) {
    assert!(!cases.is_empty());
    let wrong = differing(&cases);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A file of one language: what goes beside the test, on it, and at the top of its body.
type Template = fn(&str, &str, &str) -> String;

/// Cases whose head side puts `body` at the top of the test body.
fn bodies(path: &'static str, file: Template, rows: &[(&str, Want)]) -> Vec<Case> {
    rows.iter()
        .map(|(body, want)| Case {
            name: format!("{path}: {body}"),
            path,
            base: file("", "", ""),
            head: file("", "", body),
            config: "",
            want: *want,
            message: "",
        })
        .collect()
}

/// Cases whose head side adds members beside the test, marks on it and a body line.
fn shaped(path: &'static str, file: Template, rows: &[(&str, &str, &str, Want)]) -> Vec<Case> {
    rows.iter()
        .map(|(members, marks, body, want)| Case {
            name: format!("{path}: {members} | {marks} | {body}"),
            path,
            base: file("", "", ""),
            head: file(members, marks, body),
            config: "",
            want: *want,
            message: "",
        })
        .collect()
}

use Want::{CiSkip, Note, Nothing, Unconditional};

// --- C# ---------------------------------------------------------------------------

const CS: &str = "tests/QTests.cs";

fn cs(members: &str, marks: &str, body: &str) -> String {
    let attribute = if marks.is_empty() { "[Test]" } else { marks };
    format!(
        "using System;\nusing NUnit.Framework;\nusing Xunit;\n\npublic class QTests\n{{\n    {members}\n    {attribute}\n    public void Adds()\n    {{\n        {body}\n        Assert.AreEqual(2, 1 + 1);\n    }}\n}}\n"
    )
}

#[test]
fn csharp_a_skip_under_an_if_is_read_by_its_condition() {
    check(bodies(
        CS,
        cs,
        &[
            (
                "if (Environment.GetEnvironmentVariable(\"CI\") != null) { Assert.Ignore(\"x\"); }",
                CiSkip,
            ),
            (
                "if (Environment.GetEnvironmentVariable(\"CI\") == null) { Assert.Ignore(\"x\"); }",
                Note,
            ),
            (
                "if (System.Environment.GetEnvironmentVariable(\"GITHUB_ACTIONS\") == \"true\") Assert.Ignore(\"x\");",
                CiSkip,
            ),
            (
                "if (!string.IsNullOrEmpty(Environment.GetEnvironmentVariable(\"CI\"))) Assert.Ignore(\"x\");",
                CiSkip,
            ),
            (
                "if (string.IsNullOrEmpty(Environment.GetEnvironmentVariable(\"CI\"))) Assert.Ignore(\"x\");",
                Note,
            ),
            (
                "if (Environment.GetEnvironmentVariable(\"CI\") is not null) Assert.Ignore(\"x\");",
                CiSkip,
            ),
            (
                "if (Environment.GetEnvironmentVariable(\"CI\") is null) Assert.Ignore(\"x\");",
                Note,
            ),
            (
                "var ci = Environment.GetEnvironmentVariable(\"CI\");\n        if (ci != null) { Assert.Ignore(\"x\"); }",
                CiSkip,
            ),
            (
                "if (OperatingSystem.IsWindows()) { Assert.Ignore(\"x\"); }",
                Note,
            ),
            (
                "if (Environment.GetEnvironmentVariable(\"HOME\") != null) { Assert.Ignore(\"x\"); }",
                Note,
            ),
            ("Assert.Ignore(\"x\");", Unconditional),
            ("if (true) { Assert.Ignore(\"x\"); }", Unconditional),
            ("if (false) { Assert.Ignore(\"x\"); }", Nothing),
        ],
    ));
}

#[test]
fn csharp_a_skip_in_an_else_branch_runs_under_the_negated_condition() {
    check(bodies(
        CS,
        cs,
        &[
            (
                "if (Environment.GetEnvironmentVariable(\"CI\") == null) { Console.WriteLine(1); } else { Assert.Ignore(\"x\"); }",
                CiSkip,
            ),
            (
                "if (Environment.GetEnvironmentVariable(\"CI\") != null) { Console.WriteLine(1); } else { Assert.Ignore(\"x\"); }",
                Note,
            ),
            (
                "if (OperatingSystem.IsLinux()) { Console.WriteLine(1); } else { Assert.Ignore(\"x\"); }",
                Note,
            ),
        ],
    ));
}

#[test]
fn csharp_calls_that_take_a_condition_are_read_by_it() {
    check(bodies(
        CS,
        cs,
        &[
            (
                "Assume.That(Environment.GetEnvironmentVariable(\"CI\") == null);",
                CiSkip,
            ),
            (
                "Assume.That(Environment.GetEnvironmentVariable(\"CI\") != null, \"only in CI\");",
                Note,
            ),
            ("Assume.That(OperatingSystem.IsLinux());", Note),
            (
                "Assume.That(Environment.GetEnvironmentVariable(\"CI\"), Is.EqualTo(\"1\"));",
                CiSkip,
            ),
            (
                "Assume.That(Environment.GetEnvironmentVariable(\"CI\"), Is.Null);",
                CiSkip,
            ),
            (
                "Assume.That(Environment.GetEnvironmentVariable(\"CI\"), Is.Not.Null);",
                Note,
            ),
            ("Assume.That(false);", Unconditional),
            ("Assume.That(true);", Nothing),
            (
                "Skip.If(Environment.GetEnvironmentVariable(\"CI\") != null, \"x\");",
                CiSkip,
            ),
            (
                "Skip.If(Environment.GetEnvironmentVariable(\"CI\") == null, \"x\");",
                Note,
            ),
            (
                "Skip.IfNot(Environment.GetEnvironmentVariable(\"CI\") == null);",
                CiSkip,
            ),
            (
                "Skip.IfNot(Environment.GetEnvironmentVariable(\"CI\") != null);",
                Note,
            ),
            (
                "Assert.SkipWhen(Environment.GetEnvironmentVariable(\"CI\") != null, \"x\");",
                CiSkip,
            ),
            (
                "Assert.SkipUnless(Environment.GetEnvironmentVariable(\"CI\") != null, \"x\");",
                Note,
            ),
            (
                "if (OperatingSystem.IsLinux()) { Assume.That(Environment.GetEnvironmentVariable(\"CI\") == null); }",
                CiSkip,
            ),
        ],
    ));
}

const CS_ON_CI: &str =
    "public static bool OnCi => Environment.GetEnvironmentVariable(\"CI\") != null;";
const CS_OFF_CI: &str =
    "public static bool OffCi { get { return Environment.GetEnvironmentVariable(\"CI\") == null; } }";

#[test]
fn csharp_a_skip_attribute_with_a_condition_member_of_the_file_is_read_by_it() {
    check(shaped(
        CS,
        cs,
        &[
            (
                CS_ON_CI,
                "[Fact(Skip = \"x\", SkipWhen = nameof(OnCi))]",
                "",
                CiSkip,
            ),
            (
                CS_OFF_CI,
                "[Fact(Skip = \"x\", SkipWhen = nameof(OffCi))]",
                "",
                Note,
            ),
            (
                CS_OFF_CI,
                "[Fact(Skip = \"x\", SkipUnless = nameof(OffCi))]",
                "",
                CiSkip,
            ),
            (
                CS_ON_CI,
                "[Fact(Skip = \"x\", SkipUnless = nameof(OnCi))]",
                "",
                Note,
            ),
            // The member is in another file: a conditional skip on no CI variable.
            (
                "",
                "[Fact(Skip = \"x\", SkipWhen = nameof(Elsewhere.Flaky))]",
                "",
                Note,
            ),
            ("", "[Fact(Skip = \"x\")]", "", Unconditional),
            (CS_ON_CI, "[Test]", "if (OnCi) { Assert.Ignore(\"x\"); }", CiSkip),
            (
                "private static bool InCi() { return Environment.GetEnvironmentVariable(\"CI\") == \"true\"; }",
                "[Test]",
                "if (InCi()) { Assert.Ignore(\"x\"); }",
                CiSkip,
            ),
        ],
    ));
}

// --- Ruby -------------------------------------------------------------------------

const RB: &str = "test/q_test.rb";
const RSPEC: &str = "spec/q_spec.rb";

fn rb(members: &str, _marks: &str, body: &str) -> String {
    format!(
        "require \"minitest/autorun\"\n\n{members}\nclass QTest < Minitest::Test\n  def test_adds\n    {body}\n    assert_equal 2, 1 + 1\n  end\nend\n"
    )
}

fn rspec(members: &str, _marks: &str, body: &str) -> String {
    format!(
        "RSpec.describe \"q\" do\n  {members}\n  it \"adds\" do\n    {body}\n    expect(1 + 1).to eq(2)\n  end\nend\n"
    )
}

#[test]
fn ruby_a_skip_under_a_modifier_or_an_if_is_read_by_its_condition() {
    check(bodies(
        RB,
        rb,
        &[
            ("skip \"x\" if ENV[\"CI\"]", CiSkip),
            ("skip \"x\" unless ENV[\"CI\"]", Note),
            ("skip if ENV[\"CI\"].nil?", Note),
            ("skip unless ENV[\"CI\"].nil?", CiSkip),
            ("skip(\"x\") if ENV.fetch(\"CI\", nil)", CiSkip),
            ("skip if ENV[\"GITHUB_ACTIONS\"] == \"true\"", CiSkip),
            ("skip if ENV[\"CI\"] != \"true\"", Note),
            ("skip if !ENV[\"CI\"]", Note),
            ("if ENV.key?(\"CI\")\n      skip \"x\"\n    end", CiSkip),
            ("unless ENV.key?(\"CI\")\n      skip \"x\"\n    end", Note),
            ("ENV[\"CI\"] ? skip(\"x\") : nil", CiSkip),
            ("ENV[\"CI\"] ? nil : skip(\"x\")", Note),
            ("ENV[\"CI\"] && skip(\"x\")", CiSkip),
            ("ENV[\"CI\"] || skip(\"x\")", Note),
            ("ci = ENV[\"CI\"]\n    skip \"x\" if ci", CiSkip),
            ("skip \"x\" if RUBY_PLATFORM.include?(\"mingw\")", Note),
            ("skip \"x\" if ENV[\"HOME\"]", Note),
            ("pending \"x\" if ENV[\"CI\"]", CiSkip),
            ("pending \"x\" unless ENV[\"CI\"]", Note),
            ("omit_if(ENV[\"CI\"], \"x\")", CiSkip),
            ("omit_unless(ENV[\"CI\"], \"x\")", Note),
            ("skip \"x\"", Unconditional),
            ("skip \"x\" if true", Unconditional),
            ("skip \"x\" if false", Nothing),
        ],
    ));
}

#[test]
fn ruby_a_skip_in_an_else_branch_runs_under_the_negated_condition() {
    check(bodies(
        RB,
        rb,
        &[
            (
                "if ENV[\"CI\"].nil?\n      puts 1\n    else\n      skip \"x\"\n    end",
                CiSkip,
            ),
            (
                "if ENV[\"CI\"]\n      puts 1\n    else\n      skip \"x\"\n    end",
                Note,
            ),
            (
                "unless ENV[\"CI\"]\n      puts 1\n    else\n      skip \"x\"\n    end",
                CiSkip,
            ),
            (
                "if File.exist?(\"db\")\n      puts 1\n    else\n      skip \"x\"\n    end",
                Note,
            ),
            (
                "if File.exist?(\"db\")\n      puts 1\n    elsif ENV[\"CI\"]\n      skip \"x\"\n    end",
                CiSkip,
            ),
        ],
    ));
}

#[test]
fn ruby_a_condition_reached_through_a_name_of_the_file_is_followed() {
    check(shaped(
        RB,
        rb,
        &[
            (
                "ON_CI = ENV[\"CI\"] == \"true\"",
                "",
                "skip \"x\" if ON_CI",
                CiSkip,
            ),
            (
                "OFF_CI = ENV[\"CI\"].nil?",
                "",
                "skip \"x\" if OFF_CI",
                Note,
            ),
            (
                "def ci?\n  ENV.key?(\"CI\")\nend",
                "",
                "skip \"x\" if ci?",
                CiSkip,
            ),
            (
                "def local?\n  return true if ENV[\"CI\"].nil?\n  false\nend",
                "",
                "skip \"x\" if local?",
                CiSkip,
            ),
        ],
    ));
    check(shaped(
        RSPEC,
        rspec,
        &[
            ("", "", "skip(\"x\") if ENV[\"CI\"]", CiSkip),
            ("", "", "skip(\"x\") unless ENV[\"CI\"]", Note),
            ("", "", "pending(\"x\") if ENV[\"CI\"]", CiSkip),
            (
                "def on_ci?\n    ENV[\"CI\"]\n  end",
                "",
                "skip(\"x\") if on_ci?",
                CiSkip,
            ),
            ("", "", "skip \"x\"", Unconditional),
        ],
    ));
}

// --- PHP --------------------------------------------------------------------------

const PHP: &str = "tests/QTest.php";

fn php(members: &str, marks: &str, body: &str) -> String {
    format!(
        "<?php\n\nuse PHPUnit\\Framework\\TestCase;\n\nclass QTest extends TestCase\n{{\n    {members}\n    {marks}\n    public function testAdds(): void\n    {{\n        {body}\n        $this->assertSame(2, 1 + 1);\n    }}\n}}\n"
    )
}

#[test]
fn php_a_skip_under_an_if_is_read_by_its_condition() {
    check(bodies(
        PHP,
        php,
        &[
            ("if (getenv('CI')) { $this->markTestSkipped('x'); }", CiSkip),
            ("if (!getenv('CI')) { $this->markTestSkipped('x'); }", Note),
            (
                "if (getenv('CI') !== false) { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "if (getenv('CI') === false) { $this->markTestSkipped('x'); }",
                Note,
            ),
            (
                "if (\\getenv(\"GITHUB_ACTIONS\") === 'true') { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "if (isset($_ENV['CI'])) { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "if (!isset($_ENV['CI'])) { $this->markTestSkipped('x'); }",
                Note,
            ),
            (
                "if (!empty($_SERVER['CI'])) { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "if (empty($_SERVER[\"CI\"])) { $this->markTestSkipped('x'); }",
                Note,
            ),
            (
                "if (getenv('CI')):\n            $this->markTestSkipped('x');\n        endif;",
                CiSkip,
            ),
            (
                "$ci = getenv('CI');\n        if ($ci) { $this->markTestIncomplete('x'); }",
                CiSkip,
            ),
            ("getenv('CI') && $this->markTestSkipped('x');", CiSkip),
            ("getenv('CI') || $this->markTestSkipped('x');", Note),
            (
                "if (PHP_OS_FAMILY === 'Windows') { $this->markTestSkipped('x'); }",
                Note,
            ),
            ("if (getenv('HOME')) { $this->markTestSkipped('x'); }", Note),
            ("$this->markTestSkipped('x');", Unconditional),
            ("if (true) { $this->markTestSkipped('x'); }", Unconditional),
        ],
    ));
}

#[test]
fn php_a_skip_in_an_else_branch_runs_under_the_negated_condition() {
    check(bodies(
        PHP,
        php,
        &[
            (
                "if (!getenv('CI')) { echo 1; } else { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "if (getenv('CI')) { echo 1; } else { $this->markTestSkipped('x'); }",
                Note,
            ),
            (
                "if (extension_loaded('pdo')) { echo 1; } else { $this->markTestSkipped('x'); }",
                Note,
            ),
            (
                "if (extension_loaded('pdo')) { echo 1; } elseif (getenv('CI')) { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "if (extension_loaded('pdo')) { echo 1; } elseif (!getenv('CI')) { echo 2; } else { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "if (extension_loaded('pdo')) { echo 1; } elseif (getenv('CI')) { echo 2; } else { $this->markTestSkipped('x'); }",
                Note,
            ),
        ],
    ));
}

#[test]
fn php_requires_attributes_are_conditional_skips_on_no_ci_variable() {
    check(shaped(
        PHP,
        php,
        &[
            ("", "#[RequiresOperatingSystem('Linux')]", "", Note),
            ("", "#[RequiresPhp('>= 8.2')]", "", Note),
            (
                "",
                "#[\\PHPUnit\\Framework\\Attributes\\RequiresPhpExtension('pdo')]",
                "",
                Note,
            ),
            // An attribute that is not a requirement is no skip.
            ("", "#[Group('requires-db')]", "", Nothing),
            (
                "private function inCi(): bool { return getenv('CI') !== false; }",
                "",
                "if ($this->inCi()) { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "private static function local(): bool { return getenv('CI') === false; }",
                "",
                "if (self::local()) { $this->markTestSkipped('x'); }",
                Note,
            ),
        ],
    ));
}

// --- Swift ------------------------------------------------------------------------

const SWIFT: &str = "Tests/QTests/QTests.swift";
const SWIFT_TESTING: &str = "Tests/QTests/QSuite.swift";

fn swift(members: &str, _marks: &str, body: &str) -> String {
    format!(
        "import XCTest\n\nfinal class QTests: XCTestCase {{\n    {members}\n    func testAdds() throws {{\n        {body}\n        XCTAssertEqual(1 + 1, 2)\n    }}\n}}\n"
    )
}

fn swift_testing(members: &str, marks: &str, _body: &str) -> String {
    let attribute = if marks.is_empty() { "@Test" } else { marks };
    format!(
        "import Testing\nimport Foundation\n\n{members}\n{attribute} func adds() {{\n    #expect(1 + 1 == 2)\n}}\n"
    )
}

const SWIFT_ENV: &str = "ProcessInfo.processInfo.environment[\"CI\"]";

#[test]
fn swift_a_thrown_skip_under_an_if_or_a_guard_is_read_by_its_condition() {
    let rows: Vec<(String, Want)> = vec![
        (
            format!("if {SWIFT_ENV} != nil {{ throw XCTSkip(\"x\") }}"),
            CiSkip,
        ),
        (
            format!("if {SWIFT_ENV} == nil {{ throw XCTSkip(\"x\") }}"),
            Note,
        ),
        (
            format!("if {SWIFT_ENV} == \"true\" {{ throw XCTSkip(\"x\") }}"),
            CiSkip,
        ),
        (
            format!("if let _ = {SWIFT_ENV} {{ throw XCTSkip(\"x\") }}"),
            CiSkip,
        ),
        (
            format!("guard {SWIFT_ENV} == nil else {{ throw XCTSkip(\"x\") }}"),
            CiSkip,
        ),
        (
            format!("guard {SWIFT_ENV} != nil else {{ throw XCTSkip(\"x\") }}"),
            Note,
        ),
        (
            format!("let ci = {SWIFT_ENV}\n        if ci != nil {{ throw XCTSkip(\"x\") }}"),
            CiSkip,
        ),
        (
            format!("if {SWIFT_ENV} == nil {{ print(1) }} else {{ throw XCTSkip(\"x\") }}"),
            CiSkip,
        ),
        (
            format!("if {SWIFT_ENV} != nil {{ print(1) }} else {{ throw XCTSkip(\"x\") }}"),
            Note,
        ),
        (
            "if FileManager.default.fileExists(atPath: \"db\") { print(1) } else { throw XCTSkip(\"x\") }".to_string(),
            Note,
        ),
        (
            "if ProcessInfo.processInfo.environment[\"HOME\"] != nil { throw XCTSkip(\"x\") }"
                .to_string(),
            Note,
        ),
        ("throw XCTSkip(\"x\")".to_string(), Unconditional),
        ("if true { throw XCTSkip(\"x\") }".to_string(), Unconditional),
    ];
    let rows: Vec<(&str, Want)> = rows.iter().map(|(b, w)| (b.as_str(), *w)).collect();
    check(bodies(SWIFT, swift, &rows));
}

#[test]
fn swift_xctskipif_and_xctskipunless_are_read_by_their_condition() {
    let rows: Vec<(String, Want)> = vec![
        (format!("try XCTSkipIf({SWIFT_ENV} != nil, \"x\")"), CiSkip),
        (format!("try XCTSkipIf({SWIFT_ENV} == nil, \"x\")"), Note),
        (format!("try XCTSkipUnless({SWIFT_ENV} == nil)"), CiSkip),
        (format!("try XCTSkipUnless({SWIFT_ENV} != nil)"), Note),
        ("try XCTSkipIf(isSimulator)".to_string(), Note),
        ("try XCTSkipIf(true)".to_string(), Unconditional),
        ("try XCTSkipIf(false)".to_string(), Nothing),
        ("try XCTSkipUnless(false)".to_string(), Unconditional),
    ];
    let rows: Vec<(&str, Want)> = rows.iter().map(|(b, w)| (b.as_str(), *w)).collect();
    check(bodies(SWIFT, swift, &rows));
    let on_ci = format!("static let onCI = {SWIFT_ENV} != nil");
    let off_ci = format!("static var offCI: Bool {{ {SWIFT_ENV} == nil }}");
    check(shaped(
        SWIFT,
        swift,
        &[
            (&on_ci, "", "try XCTSkipIf(Self.onCI)", CiSkip),
            (&on_ci, "", "try XCTSkipUnless(Self.onCI)", Note),
            (&off_ci, "", "try XCTSkipIf(Self.offCI)", Note),
            (&off_ci, "", "try XCTSkipUnless(Self.offCI)", CiSkip),
        ],
    ));
}

#[test]
fn swift_testing_traits_with_a_condition_are_read_by_it() {
    let disabled_in_ci = format!("@Test(.disabled(if: {SWIFT_ENV} != nil, \"x\"))");
    let disabled_off_ci = format!("@Test(.disabled(if: {SWIFT_ENV} == nil))");
    let enabled_off_ci = format!("@Test(\"adds\", .enabled(if: {SWIFT_ENV} == nil))");
    let enabled_in_ci = format!("@Test(.enabled(if: {SWIFT_ENV} != nil))");
    let on_ci = format!("let onCI = {SWIFT_ENV} != nil");
    check(shaped(
        SWIFT_TESTING,
        swift_testing,
        &[
            ("", &disabled_in_ci, "", CiSkip),
            ("", &disabled_off_ci, "", Note),
            ("", &enabled_off_ci, "", CiSkip),
            ("", &enabled_in_ci, "", Note),
            (&on_ci, "@Test(.disabled(if: onCI))", "", CiSkip),
            (&on_ci, "@Test(.enabled(if: onCI))", "", Note),
            ("", "@Test(.enabled(if: hasDatabase))", "", Note),
            ("", "@Test(.disabled(\"flaky\"))", "", Unconditional),
            ("", "@Test(.disabled())", "", Unconditional),
            ("", "@Test(.disabled(if: true))", "", Unconditional),
            ("", "@Test(.enabled(if: true))", "", Nothing),
            ("", "@Test(.tags(.disabledLater))", "", Nothing),
        ],
    ));
}

// --- Scala ------------------------------------------------------------------------

const SCALA: &str = "src/test/scala/QSuite.scala";

fn scala(members: &str, _marks: &str, body: &str) -> String {
    format!(
        "import org.scalatest.funsuite.AnyFunSuite\n\nclass QSuite extends AnyFunSuite {{\n  {members}\n  test(\"adds\") {{\n    {body}\n    assert(1 + 1 == 2)\n  }}\n}}\n"
    )
}

#[test]
fn scala_assume_is_a_conditional_skip_read_by_its_condition() {
    check(bodies(
        SCALA,
        scala,
        &[
            ("assume(!sys.env.contains(\"CI\"))", CiSkip),
            ("assume(sys.env.contains(\"CI\"), \"only in CI\")", Note),
            ("assume(sys.env.get(\"CI\").isEmpty)", CiSkip),
            ("assume(sys.env.get(\"CI\").isDefined)", Note),
            ("assume(System.getenv(\"CI\") == null)", CiSkip),
            ("assume(System.getenv(\"CI\") != null)", Note),
            ("assume(databaseAvailable)", Note),
            ("assume(false)", Unconditional),
            ("assume(true)", Nothing),
        ],
    ));
}

#[test]
fn scala_cancel_and_pending_under_an_if_are_read_by_its_condition() {
    check(bodies(
        SCALA,
        scala,
        &[
            ("if (sys.env.contains(\"CI\")) cancel(\"x\")", CiSkip),
            ("if (!sys.env.contains(\"CI\")) cancel(\"x\")", Note),
            ("if (sys.env.get(\"CI\").isDefined) { cancel() }", CiSkip),
            (
                "if (sys.env.get(\"CI\").contains(\"true\")) cancel()",
                CiSkip,
            ),
            (
                "if (sys.env.getOrElse(\"GITHUB_ACTIONS\", \"\") == \"true\") cancel()",
                CiSkip,
            ),
            ("if (sys.env(\"CI\") != \"true\") cancel()", Note),
            ("if (System.getenv(\"CI\") != null) { pending }", CiSkip),
            ("if (System.getenv(\"CI\") == null) pending", Note),
            (
                "val ci = sys.env.get(\"CI\")\n    if (ci.isDefined) cancel()",
                CiSkip,
            ),
            (
                "if (sys.env.contains(\"CI\")) println(1) else cancel(\"x\")",
                Note,
            ),
            (
                "if (!sys.env.contains(\"CI\")) println(1) else cancel(\"x\")",
                CiSkip,
            ),
            (
                "if (new java.io.File(\"db\").exists) println(1) else cancel(\"x\")",
                Note,
            ),
            ("if (sys.env.contains(\"HOME\")) cancel(\"x\")", Note),
            ("cancel(\"x\")", Unconditional),
            ("pending", Unconditional),
            ("if (true) cancel(\"x\")", Unconditional),
        ],
    ));
    check(shaped(
        SCALA,
        scala,
        &[
            (
                "val onCi = sys.env.contains(\"CI\")",
                "",
                "if (onCi) cancel()",
                CiSkip,
            ),
            (
                "def offCi: Boolean = System.getenv(\"CI\") == null",
                "",
                "if (offCi) cancel()",
                Note,
            ),
            (
                "def offCi: Boolean = System.getenv(\"CI\") == null",
                "",
                "assume(offCi)",
                CiSkip,
            ),
        ],
    ));
}

// --- C and C++ --------------------------------------------------------------------

const CPP: &str = "tests/q_test.cpp";
const C: &str = "test/test_q.c";

fn cpp(members: &str, _marks: &str, body: &str) -> String {
    format!(
        "#include <cstdlib>\n#include <gtest/gtest.h>\n\n{members}\nTEST(Q, Adds) {{\n  {body}\n  EXPECT_EQ(1 + 1, 2);\n}}\n"
    )
}

fn unity(members: &str, _marks: &str, body: &str) -> String {
    format!(
        "#include <stdlib.h>\n#include \"unity.h\"\n\n{members}\nvoid test_adds(void) {{\n  {body}\n  TEST_ASSERT_EQUAL(2, 1 + 1);\n}}\n"
    )
}

#[test]
fn googletest_skip_under_an_if_is_read_by_its_condition() {
    check(bodies(
        CPP,
        cpp,
        &[
            (
                "if (std::getenv(\"CI\") != nullptr) { GTEST_SKIP() << \"x\"; }",
                CiSkip,
            ),
            (
                "if (std::getenv(\"CI\") == nullptr) { GTEST_SKIP() << \"x\"; }",
                Note,
            ),
            ("if (getenv(\"CI\")) GTEST_SKIP();", CiSkip),
            ("if (!getenv(\"CI\")) GTEST_SKIP();", Note),
            (
                "const char* ci = std::getenv(\"GITHUB_ACTIONS\");\n  if (ci != NULL) { GTEST_SKIP(); }",
                CiSkip,
            ),
            (
                "if (std::getenv(\"CI\") == nullptr) { std::puts(\"a\"); } else { GTEST_SKIP(); }",
                CiSkip,
            ),
            (
                "if (std::getenv(\"CI\") != nullptr) { std::puts(\"a\"); } else { GTEST_SKIP(); }",
                Note,
            ),
            (
                "if (sizeof(void*) == 8) { std::puts(\"a\"); } else { GTEST_SKIP(); }",
                Note,
            ),
            ("if (sizeof(void*) == 4) { GTEST_SKIP(); }", Note),
            ("if (std::getenv(\"HOME\") != nullptr) { GTEST_SKIP(); }", Note),
            ("GTEST_SKIP() << \"x\";", Unconditional),
            ("if (true) { GTEST_SKIP(); }", Unconditional),
        ],
    ));
    check(shaped(
        CPP,
        cpp,
        &[(
            "static bool InCi() { return std::getenv(\"CI\") != nullptr; }",
            "",
            "if (InCi()) { GTEST_SKIP(); }",
            CiSkip,
        )],
    ));
}

#[test]
fn unity_test_ignore_is_a_skip_and_under_an_if_a_conditional_one() {
    check(bodies(
        C,
        unity,
        &[
            ("TEST_IGNORE();", Unconditional),
            ("TEST_IGNORE_MESSAGE(\"x\");", Unconditional),
            (
                "if (getenv(\"CI\") != NULL) { TEST_IGNORE_MESSAGE(\"x\"); }",
                CiSkip,
            ),
            ("if (getenv(\"CI\") == NULL) { TEST_IGNORE(); }", Note),
            ("if (getenv(\"CI\")) TEST_IGNORE();", CiSkip),
            (
                "char *ci = getenv(\"CI\");\n  if (!ci) { TEST_IGNORE(); }",
                Note,
            ),
            (
                "if (getenv(\"CI\") == NULL) { puts(\"a\"); } else { TEST_IGNORE(); }",
                CiSkip,
            ),
            ("if (sizeof(long) == 4) { TEST_IGNORE(); }", Note),
        ],
    ));
}

// --- Objective-C ------------------------------------------------------------------

const OBJC: &str = "Tests/QTests.m";

fn objc(members: &str, _marks: &str, body: &str) -> String {
    format!(
        "#import <XCTest/XCTest.h>\n\n{members}\n@interface QTests : XCTestCase\n@end\n\n@implementation QTests\n- (void)testAdds {{\n  {body}\n  XCTAssertEqual(1 + 1, 2);\n}}\n@end\n"
    )
}

#[test]
fn objc_xctskip_under_an_if_and_the_condition_taking_forms_are_read() {
    check(bodies(
        OBJC,
        objc,
        &[
            (
                "if ([[NSProcessInfo processInfo] environment][@\"CI\"] != nil) { XCTSkip(@\"x\"); }",
                CiSkip,
            ),
            (
                "if ([[NSProcessInfo processInfo] environment][@\"CI\"] == nil) { XCTSkip(@\"x\"); }",
                Note,
            ),
            (
                "if (NSProcessInfo.processInfo.environment[@\"CI\"]) { XCTSkip(@\"x\"); }",
                CiSkip,
            ),
            (
                "NSString *ci = [[[NSProcessInfo processInfo] environment] objectForKey:@\"CI\"];\n  if ([ci isEqualToString:@\"true\"]) { XCTSkip(@\"x\"); }",
                CiSkip,
            ),
            (
                "if (getenv(\"CI\") == NULL) { NSLog(@\"a\"); } else { XCTSkip(@\"x\"); }",
                CiSkip,
            ),
            (
                "if (getenv(\"CI\") != NULL) { NSLog(@\"a\"); } else { XCTSkip(@\"x\"); }",
                Note,
            ),
            (
                "if ([NSFileManager.defaultManager fileExistsAtPath:@\"db\"]) { NSLog(@\"a\"); } else { XCTSkip(@\"x\"); }",
                Note,
            ),
            (
                "XCTSkipIf(NSProcessInfo.processInfo.environment[@\"CI\"] != nil, @\"x\");",
                CiSkip,
            ),
            (
                "XCTSkipIf(NSProcessInfo.processInfo.environment[@\"CI\"] == nil, @\"x\");",
                Note,
            ),
            ("XCTSkipUnless(getenv(\"CI\") == NULL, @\"x\");", CiSkip),
            ("XCTSkipUnless(getenv(\"CI\") != NULL, @\"x\");", Note),
            ("XCTSkipIf(TARGET_OS_SIMULATOR, @\"x\");", Note),
            (
                "if (NSProcessInfo.processInfo.environment[@\"HOME\"]) { XCTSkip(@\"x\"); }",
                Note,
            ),
            ("XCTSkip(@\"x\");", Unconditional),
            ("XCTSkipIf(YES, @\"x\");", Unconditional),
            ("XCTSkipIf(NO, @\"x\");", Nothing),
        ],
    ));
}

// --- names that are only spelled like a CI variable --------------------------------

/// `BUILD_ID` and the `CI_` prefix name a CI variable as the argument of an environment
/// read. An identifier with that spelling is not one.
#[test]
fn build_id_and_the_ci_prefix_count_in_an_environment_read_only() {
    check(bodies(
        RB,
        rb,
        &[
            ("skip \"x\" if ENV[\"BUILD_ID\"]", CiSkip),
            ("skip \"x\" if ENV[\"CI_NODE_TOTAL\"]", CiSkip),
            ("skip \"x\" if BUILD_ID", Note),
            ("skip \"x\" if CI_NODE_TOTAL", Note),
            ("skip \"x\" if build_id.nil?", Note),
        ],
    ));
    check(bodies(
        PHP,
        php,
        &[
            (
                "if (getenv('BUILD_ID')) { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            (
                "if (getenv('CI_JOB_ID')) { $this->markTestSkipped('x'); }",
                CiSkip,
            ),
            ("if ($BUILD_ID) { $this->markTestSkipped('x'); }", Note),
            ("if (CI_JOB_ID) { $this->markTestSkipped('x'); }", Note),
        ],
    ));
    check(bodies(
        CPP,
        cpp,
        &[
            ("if (std::getenv(\"BUILD_ID\")) { GTEST_SKIP(); }", CiSkip),
            (
                "if (std::getenv(\"CI_PIPELINE_ID\")) { GTEST_SKIP(); }",
                CiSkip,
            ),
            ("if (BUILD_ID > 0) { GTEST_SKIP(); }", Note),
            ("if (CI_PIPELINE_ID > 0) { GTEST_SKIP(); }", Note),
        ],
    ));
}

/// Skips inside loops, switch/case/match, and preprocessor conditionals (#if, #ifdef)
/// are conditional skips, never unconditional ignores (#651).
#[test]
fn skips_under_loops_switch_match_and_preprocessor_are_conditional() {
    check(bodies(
        CS,
        cs,
        &[
            ("#if CI\n        Assert.Ignore(\"x\");\n#endif", CiSkip),
            ("#if !CI\n        Assert.Ignore(\"x\");\n#endif", Note),
            ("while (flag) { Assert.Ignore(\"x\"); }", Note),
            (
                "for (int i = 0; i < 10; i++) { Assert.Ignore(\"x\"); }",
                Note,
            ),
            ("switch (x) { case 1: Assert.Ignore(\"x\"); break; }", Note),
        ],
    ));
    check(bodies(
        RB,
        rb,
        &[
            ("for x in xs do\n      skip \"x\"\n    end", Note),
            ("while cond\n      skip \"x\"\n    end", Note),
            ("case x\n    when 1\n      skip \"x\"\n    end", Note),
        ],
    ));
    check(bodies(
        PHP,
        php,
        &[
            ("while ($cond) { $this->markTestSkipped('x'); }", Note),
            (
                "for ($i = 0; $i < 10; $i++) { $this->markTestSkipped('x'); }",
                Note,
            ),
            (
                "switch ($x) { case 1: $this->markTestSkipped('x'); break; }",
                Note,
            ),
            (
                "match ($x) { 1 => $this->markTestSkipped('x'), default => null };",
                Note,
            ),
        ],
    ));
    check(bodies(
        SWIFT,
        swift,
        &[
            ("#if CI\n        throw XCTSkip(\"x\")\n#endif", CiSkip),
            ("#if !CI\n        throw XCTSkip(\"x\")\n#endif", Note),
            ("for x in xs { throw XCTSkip(\"x\") }", Note),
            ("while cond { throw XCTSkip(\"x\") }", Note),
            (
                "switch x { case 1: throw XCTSkip(\"x\")\ndefault: break }",
                Note,
            ),
        ],
    ));
    check(bodies(
        SCALA,
        scala,
        &[
            ("for (x <- xs) { cancel(\"x\") }", Note),
            ("while (cond) { cancel(\"x\") }", Note),
            ("x match { case 1 => cancel(\"x\") }", Note),
        ],
    ));
    check(bodies(
        CPP,
        cpp,
        &[
            ("#ifdef CI\n  GTEST_SKIP();\n#endif", CiSkip),
            ("#ifndef CI\n  GTEST_SKIP();\n#endif", Note),
            ("#if defined(CI)\n  GTEST_SKIP();\n#endif", CiSkip),
            ("while (cond) { GTEST_SKIP(); }", Note),
            ("switch (x) { case 1: GTEST_SKIP(); break; }", Note),
        ],
    ));
    check(bodies(
        C,
        unity,
        &[
            ("#ifdef CI\n  TEST_IGNORE();\n#endif", CiSkip),
            ("#ifndef CI\n  TEST_IGNORE();\n#endif", Note),
            ("while (cond) { TEST_IGNORE(); }", Note),
            ("switch (x) { case 1: TEST_IGNORE(); break; }", Note),
        ],
    ));
}

// --- severity and approval ----------------------------------------------------------

const CI_SKIP_WARNING: &str = "[gates.ignored-tests]\nci_skip_severity = \"warning\"\n";
const APPROVE_CI: &str = "[gates.ignored-tests]\napproved_predicates = [\"CI\"]\n";

/// The CI-conditional skips of these packs take `ci_skip_severity` and
/// `approved_predicates` like those of the packs read before.
#[test]
fn ci_skip_severity_and_approved_predicates_apply_to_these_packs() {
    let heads: [(&'static str, Template, &str); 7] = [
        (
            CS,
            cs,
            "if (Environment.GetEnvironmentVariable(\"CI\") != null) { Assert.Ignore(\"x\"); }",
        ),
        (RB, rb, "skip \"x\" if ENV[\"CI\"]"),
        (
            PHP,
            php,
            "if (getenv('CI')) { $this->markTestSkipped('x'); }",
        ),
        (
            SWIFT,
            swift,
            "try XCTSkipIf(ProcessInfo.processInfo.environment[\"CI\"] != nil)",
        ),
        (SCALA, scala, "assume(!sys.env.contains(\"CI\"))"),
        (
            CPP,
            cpp,
            "if (std::getenv(\"CI\") != nullptr) { GTEST_SKIP(); }",
        ),
        (OBJC, objc, "XCTSkipIf(getenv(\"CI\") != NULL, @\"x\");"),
    ];
    for (path, file, body) in heads {
        let with = |config: &'static str| Case {
            name: format!("{path}: {config}"),
            path,
            base: file("", "", ""),
            head: file("", "", body),
            config,
            want: CiSkip,
            message: "",
        };
        let warned = run_change(&with(CI_SKIP_WARNING));
        assert_eq!(warned.code, 0, "{path}: {:?}", warned.findings);
        assert_eq!(warned.findings.len(), 1, "{path}: {:?}", warned.findings);
        assert_eq!(warned.findings[0].0, "Test Conditionally Skipped", "{path}");
        assert_eq!(warned.findings[0].1, "warning", "{path}");
        assert!(
            warned.findings[0].2.contains("CI"),
            "{path}: {:?}",
            warned.findings
        );
        let approved = run_change(&with(APPROVE_CI));
        assert_eq!(approved.code, 0, "{path}: {:?}", approved.findings);
        assert!(
            approved.findings.is_empty(),
            "{path}: {:?}",
            approved.findings
        );
    }
}
