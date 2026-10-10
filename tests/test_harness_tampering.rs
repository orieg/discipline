//! `harness-tampering` (#481, D1): a file the test runner loads must not make a failing
//! run pass. Every case drives the real binary on a throwaway repository.
//!
//! [`EXACT_FORMS`] lists each form the gate reports, with its code and severity.
//! [`NEGATIVE_CONTROLS`] is the enumerated list of near misses it must not report: the
//! same construct written so that the result still reaches the runner, a conditional
//! exit, and the exact form in a file no runner loads. The severity matrix runs both
//! lists in one change and holds every case to its row.

mod common;
use common::{Repo, Run, CONFIG_HEAD};

const GATE: &str = "harness-tampering";
const TEST_MAIN: &str = "harness-tampering/test-main-result-discarded";
const PYTEST: &str = "harness-tampering/pytest-hook-masks-results";
const UNITTEST: &str = "harness-tampering/unittest-result-method-replaced";
const EXIT_ZERO: &str = "harness-tampering/harness-exits-zero";
const SCOPE_LIMIT: &str = "Exact form only; no value-flow analysis.";

/// One change, in a directory of its own.
struct Case {
    name: &'static str,
    /// The files of the change, relative to the case's directory.
    files: &'static [(&'static str, &'static str)],
    /// What the gate reports: `(file, text of the reported line, code, severity)`.
    expect: &'static [(&'static str, &'static str, &'static str, &'static str)],
}

macro_rules! go {
    ($body:literal) => {
        concat!(
            "package p\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\n",
            $body,
            "\nfunc TestAdds(t *testing.T) {\n\tif 1+1 != 2 {\n\t\tt.Fatal(\"sum\")\n\t}\n}\n"
        )
    };
}

const CARGO_PACKAGE: &str = "[package]\nname = \"case\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const CARGO_OWN_MAIN: &str = "[package]\nname = \"case\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[test]]\nname = \"own\"\npath = \"checks/own.rs\"\nharness = false\n";
const CARGO_NOT_TESTED: &str = "[package]\nname = \"case\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautotests = false\n\n[[test]]\nname = \"own\"\npath = \"checks/own.rs\"\ntest = false\n";

/// Every form the gate reports.
const EXACT_FORMS: &[Case] = &[
    // ---- D1a: Go `TestMain` ----
    Case {
        name: "go: TestMain never calls m.Run()",
        files: &[("p_test.go", go!("func TestMain(m *testing.M) {\n\tos.Exit(0)\n}\n"))],
        expect: &[("p_test.go", "func TestMain(m *testing.M) {", TEST_MAIN, "error")],
    },
    Case {
        name: "go: m.Run() as a statement, then os.Exit(0)",
        files: &[("p_test.go", go!("func TestMain(m *testing.M) {\n\tm.Run()\n\tos.Exit(0)\n}\n"))],
        expect: &[("p_test.go", "\tm.Run()", TEST_MAIN, "error")],
    },
    Case {
        name: "go: result assigned to the blank identifier",
        files: &[("p_test.go", go!("func TestMain(m *testing.M) {\n\t_ = m.Run()\n\tos.Exit(0)\n}\n"))],
        expect: &[("p_test.go", "\t_ = m.Run()", TEST_MAIN, "error")],
    },
    Case {
        name: "go: result bound and never read",
        files: &[(
            "p_test.go",
            go!("func TestMain(m *testing.M) {\n\tcode := m.Run()\n\t_ = code\n\tos.Exit(0)\n}\n"),
        )],
        expect: &[("p_test.go", "\tcode := m.Run()", TEST_MAIN, "error")],
    },
    Case {
        name: "go: a deferred exit that does not carry the result",
        files: &[(
            "p_test.go",
            go!("func TestMain(m *testing.M) {\n\tdefer os.Exit(0)\n\tm.Run()\n}\n"),
        )],
        expect: &[("p_test.go", "\tm.Run()", TEST_MAIN, "error")],
    },
    // ---- D1b: pytest ----
    Case {
        name: "pytest: a hookwrapper assigns report.outcome",
        files: &[(
            "conftest.py",
            "import pytest\n\n\n@pytest.hookimpl(hookwrapper=True)\ndef pytest_runtest_makereport(item, call):\n    outcome = yield\n    report = outcome.get_result()\n    if report.failed:\n        report.outcome = \"passed\"\n",
        )],
        expect: &[("conftest.py", "        report.outcome = \"passed\"", PYTEST, "error")],
    },
    Case {
        name: "pytest: logreport sets the outcome through setattr",
        files: &[(
            "conftest.py",
            "def pytest_runtest_logreport(report):\n    setattr(report, \"outcome\", \"passed\")\n",
        )],
        expect: &[("conftest.py", "    setattr(report, \"outcome\", \"passed\")", PYTEST, "error")],
    },
    Case {
        name: "pytest: a hookwrapper calls force_result",
        files: &[(
            "conftest.py",
            "import pytest\n\n\n@pytest.hookimpl(hookwrapper=True)\ndef pytest_runtest_makereport(item, call):\n    outcome = yield\n    outcome.force_result(None)\n",
        )],
        expect: &[("conftest.py", "    outcome.force_result(None)", PYTEST, "error")],
    },
    Case {
        name: "pytest: sessionfinish assigns exitstatus",
        files: &[(
            "conftest.py",
            "def pytest_sessionfinish(session, exitstatus):\n    session.exitstatus = 0\n",
        )],
        expect: &[("conftest.py", "    session.exitstatus = 0", PYTEST, "error")],
    },
    Case {
        name: "pytest: a plugin module named by conftest.py defines a report hook",
        files: &[
            ("conftest.py", "pytest_plugins = ['tamper_mod']\n"),
            (
                "tamper_mod.py",
                "def pytest_runtest_logreport(report):\n    report.outcome = \"passed\"\n",
            ),
        ],
        expect: &[(
            "tamper_mod.py",
            "    report.outcome = \"passed\"",
            PYTEST,
            "error",
        )],
    },
    Case {
        name: "pytest: the collection hook empties items",
        files: &[(
            "conftest.py",
            "def pytest_collection_modifyitems(config, items):\n    items[:] = []\n",
        )],
        expect: &[("conftest.py", "    items[:] = []", PYTEST, "error")],
    },
    Case {
        name: "pytest: the collection hook deselects by name",
        files: &[(
            "conftest.py",
            "def pytest_collection_modifyitems(session, config, items):\n    dropped = [i for i in items if \"flaky\" in i.name]\n    config.hook.pytest_deselected(items=dropped)\n    for item in dropped:\n        items.remove(item)\n",
        )],
        expect: &[
            ("conftest.py", "    config.hook.pytest_deselected(items=dropped)", PYTEST, "error"),
            ("conftest.py", "        items.remove(item)", PYTEST, "error"),
        ],
    },
    Case {
        name: "pytest: the collection hook deletes a slice of items",
        files: &[(
            "conftest.py",
            "def pytest_collection_modifyitems(config, items):\n    del items[1:]\n",
        )],
        expect: &[("conftest.py", "    del items[1:]", PYTEST, "error")],
    },
    // ---- D1d: unittest ----
    Case {
        name: "unittest: addFailure assigned on the class",
        files: &[(
            "conftest.py",
            "import unittest\n\nunittest.TestResult.addFailure = lambda self, test, err: None\n",
        )],
        expect: &[(
            "conftest.py",
            "unittest.TestResult.addFailure = lambda self, test, err: None",
            UNITTEST,
            "error",
        )],
    },
    Case {
        name: "unittest: a result subclass in a test file that records nothing",
        files: &[(
            "tests/test_quiet.py",
            "import unittest\n\n\nclass Quiet(unittest.TextTestResult):\n    def addError(self, test, err):\n        pass\n\n    def wasSuccessful(self):\n        return True\n",
        )],
        expect: &[
            ("tests/test_quiet.py", "    def addError(self, test, err):", UNITTEST, "error"),
            ("tests/test_quiet.py", "    def wasSuccessful(self):", UNITTEST, "error"),
        ],
    },
    Case {
        name: "unittest: wasSuccessful set through setattr in a start-up file",
        files: &[(
            "sitecustomize.py",
            "import unittest\n\nsetattr(unittest.TestResult, \"wasSuccessful\", lambda self: True)\n",
        )],
        expect: &[(
            "sitecustomize.py",
            "setattr(unittest.TestResult, \"wasSuccessful\", lambda self: True)",
            UNITTEST,
            "error",
        )],
    },
    // ---- D1c: an exit with status zero ----
    Case {
        name: "exit: os._exit(0) in a pytest hook",
        files: &[(
            "conftest.py",
            "import os\n\n\ndef pytest_sessionfinish(session, exitstatus):\n    os._exit(0)\n",
        )],
        expect: &[("conftest.py", "    os._exit(0)", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: sys.exit(0) at the top of a start-up file",
        files: &[("sitecustomize.py", "import sys\n\nsys.exit(0)\n")],
        expect: &[("sitecustomize.py", "sys.exit(0)", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: os._exit(0) registered to run at exit",
        files: &[(
            "conftest.py",
            "import atexit\nimport os\n\natexit.register(lambda: os._exit(0))\n",
        )],
        expect: &[("conftest.py", "atexit.register(lambda: os._exit(0))", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: the global tear-down a jest.config.json names",
        files: &[
            ("jest.config.json", "{\n  \"globalTeardown\": \"./test/teardown.js\"\n}\n"),
            (
                "test/teardown.js",
                "module.exports = async () => {\n  await Promise.resolve();\n  process.exit(0);\n};\n",
            ),
        ],
        expect: &[("test/teardown.js", "  process.exit(0);", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: a set-up file the jest key of package.json names",
        files: &[
            (
                "package.json",
                "{\n  \"name\": \"case\",\n  \"jest\": {\n    \"setupFiles\": [\"<rootDir>/test/setup.js\"]\n  }\n}\n",
            ),
            ("test/setup.js", "process.exit(0);\n"),
        ],
        expect: &[("test/setup.js", "process.exit(0);", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: the global set-up a vitest.config.ts names",
        files: &[
            (
                "vitest.config.ts",
                "import { defineConfig } from 'vitest/config';\n\nexport default defineConfig({\n  test: {\n    globalSetup: './vitest.global.ts',\n  },\n});\n",
            ),
            (
                "vitest.global.ts",
                "export default async function setup(): Promise<void> {\n  process.exit(0);\n}\n",
            ),
        ],
        expect: &[("vitest.global.ts", "  process.exit(0);", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: in the runner configuration itself",
        files: &[("jest.config.js", "process.exit(0);\n\nmodule.exports = {};\n")],
        expect: &[("jest.config.js", "process.exit(0);", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: a set-up file named through a bound configuration object",
        files: &[
            (
                "jest.config.ts",
                "import type { Config } from 'jest';\n\nconst config: Config = {\n  setupFilesAfterEnv: ['./jest.setup'],\n};\n\nexport default config;\n",
            ),
            ("jest.setup.ts", "process.exit(0);\n"),
        ],
        expect: &[("jest.setup.ts", "process.exit(0);", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: the main of a Cargo test target with harness = false",
        files: &[
            ("Cargo.toml", CARGO_OWN_MAIN),
            (
                "checks/own.rs",
                "fn main() {\n    let _ = run();\n    std::process::exit(0);\n}\n\nfn run() -> Result<(), String> {\n    Err(\"failed\".to_string())\n}\n",
            ),
        ],
        expect: &[("checks/own.rs", "    std::process::exit(0);", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: inside a test of a Cargo test target",
        files: &[
            ("Cargo.toml", CARGO_PACKAGE),
            (
                "tests/it.rs",
                "use std::process;\n\n#[test]\nfn first() {\n    process::exit(0);\n}\n\n#[test]\nfn second() {\n    assert_eq!(1 + 1, 3);\n}\n",
            ),
        ],
        expect: &[("tests/it.rs", "    process::exit(0);", EXIT_ZERO, "warning")],
    },
    Case {
        name: "exit: a module of a Cargo test target",
        files: &[
            ("Cargo.toml", CARGO_PACKAGE),
            (
                "tests/it.rs",
                "mod common;\n\n#[test]\nfn it() {\n    common::exit();\n}\n",
            ),
            (
                "tests/common/mod.rs",
                "pub fn exit() {\n    std::process::exit(0);\n}\n",
            ),
        ],
        expect: &[("tests/common/mod.rs", "    std::process::exit(0);", EXIT_ZERO, "warning")],
    },
];

/// The enumerated negative controls: each is a near miss of a form above and must
/// report nothing.
const NEGATIVE_CONTROLS: &[Case] = &[
    // ---- D1a ----
    Case {
        name: "go: os.Exit(m.Run())",
        files: &[("p_test.go", go!("func TestMain(m *testing.M) {\n\tos.Exit(m.Run())\n}\n"))],
        expect: &[],
    },
    Case {
        name: "go: the result is kept across a tear-down and exits with it",
        files: &[(
            "p_test.go",
            go!("func TestMain(m *testing.M) {\n\tcode := m.Run()\n\tteardown()\n\tos.Exit(code)\n}\n\nfunc teardown() {}\n"),
        )],
        expect: &[],
    },
    Case {
        name: "go: TestMain returns after m.Run() and the test binary exits with its result",
        files: &[(
            "p_test.go",
            go!("func TestMain(m *testing.M) {\n\tdefer teardown()\n\tm.Run()\n}\n\nfunc teardown() {}\n"),
        )],
        expect: &[],
    },
    Case {
        name: "go: a non-zero exit on a failed set-up, before the tests",
        files: &[(
            "p_test.go",
            go!("func TestMain(m *testing.M) {\n\tif err := setup(); err != nil {\n\t\tos.Exit(1)\n\t}\n\tm.Run()\n}\n\nfunc setup() error { return nil }\n"),
        )],
        expect: &[],
    },
    Case {
        name: "go: a non-zero exit on a failed tear-down, after the tests",
        files: &[(
            "p_test.go",
            go!("func TestMain(m *testing.M) {\n\tm.Run()\n\tif err := teardown(); err != nil {\n\t\tos.Exit(1)\n\t}\n}\n\nfunc teardown() error { return nil }\n"),
        )],
        expect: &[],
    },
    Case {
        name: "go: a TestMain in a package whose test files declare benchmarks only",
        files: &[
            (
                "p_test.go",
                "package p\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\nfunc TestMain(m *testing.M) {\n\tm.Run()\n\tos.Exit(0)\n}\n",
            ),
            (
                "bench_test.go",
                "package p\n\nimport \"testing\"\n\nfunc BenchmarkSum(b *testing.B) {\n\tfor i := 0; i < b.N; i++ {\n\t\t_ = i + 1\n\t}\n}\n",
            ),
        ],
        expect: &[],
    },
    Case {
        name: "go: a function named TestMain that is not the go tool's",
        files: &[(
            "p_test.go",
            go!("func TestMain(t *testing.T) {\n\tif false {\n\t\tos.Exit(0)\n\t}\n\tif 2 != 2 {\n\t\tt.Fatal(\"two\")\n\t}\n}\n"),
        )],
        expect: &[],
    },
    Case {
        name: "go: the exact form in a directory the go tool ignores",
        files: &[(
            "testdata/p_test.go",
            go!("func TestMain(m *testing.M) {\n\tm.Run()\n\tos.Exit(0)\n}\n"),
        )],
        expect: &[],
    },
    Case {
        name: "go: the exact form in a file that is not a test file",
        files: &[(
            "main_testing.go",
            go!("func TestMain(m *testing.M) {\n\tm.Run()\n\tos.Exit(0)\n}\n"),
        )],
        expect: &[],
    },
    // ---- D1b ----
    Case {
        name: "pytest: a hookwrapper that keeps the report on the item",
        files: &[(
            "conftest.py",
            "import pytest\n\n\n@pytest.hookimpl(hookwrapper=True)\ndef pytest_runtest_makereport(item, call):\n    outcome = yield\n    rep = outcome.get_result()\n    setattr(item, \"rep_\" + rep.when, rep)\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: a hook that only reads the outcome",
        files: &[(
            "conftest.py",
            "FAILED = []\n\n\ndef pytest_runtest_logreport(report):\n    if report.outcome == \"failed\":\n        FAILED.append(report.nodeid)\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: an outcome assigned outside a report hook",
        files: &[(
            "conftest.py",
            "def make_report(rep):\n    rep.outcome = \"passed\"\n    return rep\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: force_result called outside a report hook",
        files: &[(
            "conftest.py",
            "def helper(res):\n    res.force_result(None)\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: sessionfinish only reads exitstatus",
        files: &[(
            "conftest.py",
            "def pytest_sessionfinish(session, exitstatus):\n    if session.exitstatus != 0:\n        print(\"failed\")\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: the collection hook sorts items",
        files: &[(
            "conftest.py",
            "def pytest_collection_modifyitems(config, items):\n    items.sort(key=lambda item: item.nodeid)\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: the collection hook reverses items",
        files: &[(
            "conftest.py",
            "def pytest_collection_modifyitems(config, items):\n    items.reverse()\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: the collection hook assigns items their sorted order",
        files: &[(
            "conftest.py",
            "def pytest_collection_modifyitems(config, items):\n    items[:] = sorted(items, key=lambda item: item.nodeid)\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: deselection by a marker and an option",
        files: &[(
            "conftest.py",
            "def pytest_collection_modifyitems(config, items):\n    if config.getoption(\"--runslow\"):\n        return\n    kept = [i for i in items if not i.get_closest_marker(\"slow\")]\n    config.hook.pytest_deselected(items=[i for i in items if i not in kept])\n    items[:] = kept\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: deselection by the -m expression read from the configuration",
        files: &[(
            "conftest.py",
            "def pytest_collection_modifyitems(config, items):\n    expression = config.getoption(\"-m\")\n    items[:] = [i for i in items if selected(i, expression)]\n\n\ndef selected(item, expression):\n    return expression in item.nodeid\n",
        )],
        expect: &[],
    },
    Case {
        name: "pytest: the exact form in a module that is not a conftest.py",
        files: &[(
            "support/hooks.py",
            "def pytest_collection_modifyitems(config, items):\n    items[:] = []\n\n\ndef pytest_runtest_logreport(report):\n    report.outcome = \"passed\"\n",
        )],
        expect: &[],
    },
    // ---- D1d ----
    Case {
        name: "unittest: a result subclass that extends addFailure",
        files: &[(
            "tests/test_loud.py",
            "import unittest\n\n\nclass Loud(unittest.TestResult):\n    def addFailure(self, test, err):\n        super().addFailure(test, err)\n        print(test)\n\n    def wasSuccessful(self):\n        return super().wasSuccessful() and not self.skipped\n",
        )],
        expect: &[],
    },
    Case {
        name: "unittest: a result that is only read",
        files: &[(
            "tests/test_report.py",
            "def summary(result):\n    if not result.wasSuccessful():\n        return len(result.failures) + len(result.errors)\n    return 0\n",
        )],
        expect: &[],
    },
    Case {
        name: "unittest: a class with the method names that is not a result class",
        files: &[(
            "tests/test_recorder.py",
            "class Recorder(Base):\n    def addFailure(self, test, err):\n        pass\n\n    def addError(self, test, err):\n        pass\n",
        )],
        expect: &[],
    },
    Case {
        name: "unittest: the exact form in a file no runner loads",
        files: &[(
            "app/result.py",
            "import unittest\n\nunittest.TestResult.addFailure = lambda self, test, err: None\n",
        )],
        expect: &[],
    },
    // ---- D1c ----
    Case {
        name: "exit: a non-zero exit on a failure path",
        files: &[(
            "conftest.py",
            "import sys\n\n\ndef pytest_configure(config):\n    if not database_ready():\n        sys.exit(1)\n\n\ndef database_ready():\n    return True\n",
        )],
        expect: &[],
    },
    Case {
        name: "exit: a non-zero exit with no condition around it, in each language",
        files: &[
            ("conftest.py", "import os\nimport sys\n\n\ndef pytest_sessionfinish(session, exitstatus):\n    os._exit(3)\n\n\ndef refuse():\n    sys.exit(1)\n"),
            ("jest.config.json", "{\n  \"globalTeardown\": \"./test/teardown.js\"\n}\n"),
            ("test/teardown.js", "module.exports = async () => {\n  process.exit(1);\n};\n"),
            ("Cargo.toml", CARGO_OWN_MAIN),
            ("checks/own.rs", "fn main() {\n    std::process::exit(1);\n}\n"),
        ],
        expect: &[],
    },
    Case {
        name: "exit: a zero exit behind a condition",
        files: &[(
            "conftest.py",
            "import sys\n\n\ndef pytest_configure(config):\n    if config.getoption(\"--list-fixtures-only\"):\n        sys.exit(0)\n",
        )],
        expect: &[],
    },
    Case {
        name: "exit: a zero exit after a guard clause",
        files: &[(
            "conftest.py",
            "import os\n\n\ndef pytest_sessionfinish(session, exitstatus):\n    if exitstatus != 0:\n        return\n    os._exit(0)\n",
        )],
        expect: &[],
    },
    Case {
        name: "exit: a zero exit in an exception handler",
        files: &[(
            "sitecustomize.py",
            "import os\n\ntry:\n    import coverage\nexcept ImportError:\n    os._exit(0)\n",
        )],
        expect: &[],
    },
    Case {
        name: "exit: sys.exit(0) in a test file, where only the unittest forms are read",
        files: &[("tests/test_cli.py", "import sys\n\n\ndef test_exits():\n    sys.exit(0)\n")],
        expect: &[],
    },
    Case {
        name: "exit: a non-zero exit in a global set-up",
        files: &[
            ("jest.config.json", "{\n  \"globalSetup\": \"./test/setup.js\"\n}\n"),
            (
                "test/setup.js",
                "module.exports = async () => {\n  if (!process.env.DATABASE_URL) {\n    process.exit(1);\n  }\n};\n",
            ),
        ],
        expect: &[],
    },
    Case {
        name: "exit: a zero exit behind a condition and in a signal handler",
        files: &[
            ("jest.config.json", "{\n  \"globalTeardown\": \"./test/teardown.js\"\n}\n"),
            (
                "test/teardown.js",
                "process.on('SIGINT', () => process.exit(0));\n\nmodule.exports = async () => {\n  if (process.env.WATCH) {\n    process.exit(0);\n  }\n};\n",
            ),
        ],
        expect: &[],
    },
    Case {
        name: "exit: the exact form in a script no configuration names",
        files: &[
            ("jest.config.json", "{\n  \"testEnvironment\": \"node\"\n}\n"),
            ("scripts/setup.js", "process.exit(0);\n"),
            ("test/globalSetup.js", "process.exit(0);\n"),
        ],
        expect: &[],
    },
    Case {
        name: "exit: a harness = false main that exits with the status of the run",
        files: &[
            ("Cargo.toml", CARGO_OWN_MAIN),
            (
                "checks/own.rs",
                "fn main() {\n    match run() {\n        Ok(()) => std::process::exit(0),\n        Err(_) => std::process::exit(1),\n    }\n}\n\nfn run() -> Result<(), String> {\n    Ok(())\n}\n",
            ),
        ],
        expect: &[],
    },
    Case {
        name: "exit: a zero exit that a failure leaves before reaching",
        files: &[
            ("Cargo.toml", CARGO_OWN_MAIN),
            (
                "checks/own.rs",
                "fn main() -> Result<(), String> {\n    run()?;\n    std::process::exit(0)\n}\n\nfn run() -> Result<(), String> {\n    Ok(())\n}\n",
            ),
        ],
        expect: &[],
    },
    Case {
        name: "exit: a test target that re-executes itself as a child process",
        files: &[
            ("Cargo.toml", CARGO_PACKAGE),
            (
                "tests/it.rs",
                "#[test]\nfn child() {\n    if std::env::var(\"CHILD\").is_ok() {\n        std::process::exit(0);\n    }\n    assert_eq!(1 + 1, 2);\n}\n",
            ),
        ],
        expect: &[],
    },
    Case {
        name: "exit: the exact form in a binary and in a target cargo test does not run",
        files: &[
            ("Cargo.toml", CARGO_NOT_TESTED),
            ("src/main.rs", "fn main() {\n    std::process::exit(0);\n}\n"),
            ("checks/own.rs", "fn main() {\n    std::process::exit(0);\n}\n"),
            ("tests/unlisted.rs", "fn main() {\n    std::process::exit(0);\n}\n"),
        ],
        expect: &[],
    },
    Case {
        name: "exit: a module not reached from any Cargo test target",
        files: &[
            ("Cargo.toml", CARGO_PACKAGE),
            ("tests/it.rs", "#[test]\nfn it() {}\n"),
            (
                "tests/common/mod.rs",
                "pub fn exit() {\n    std::process::exit(0);\n}\n",
            ),
        ],
        expect: &[],
    },
];

fn enabled() -> String {
    format!("{CONFIG_HEAD}[gates.{GATE}]\nenabled = true\n")
}

/// A repository whose base commit turns the gate on, with `extra` appended to its table.
fn repo_with(extra: &str) -> Repo {
    let repo = Repo::new();
    repo.commit_base(
        "discipline.toml",
        &format!("{}{extra}", enabled()),
        "chore: turn the gate on",
    );
    repo
}

fn check(repo: &Repo) -> Run {
    repo.check_with_pr(&[], "no-issue: a test change")
}

fn line_of(content: &str, needle: &str) -> usize {
    let found: Vec<usize> = content
        .lines()
        .enumerate()
        .filter(|(_, line)| *line == needle)
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "`{needle}` must be one whole line of the fixture"
    );
    found[0]
}

/// `(file, line, code, severity)` of every finding of the gate, sorted.
fn findings(run: &Run) -> Vec<(String, u64, String, String)> {
    let mut all: Vec<_> = run
        .violations(GATE)
        .iter()
        .map(|v| {
            (
                v["file"].as_str().unwrap().to_string(),
                v["line"].as_u64().unwrap(),
                v["code"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    all.sort();
    all
}

fn notes(run: &Run) -> Vec<String> {
    run.outcome(GATE)["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

/// Writes every case into its own directory (`<prefix><index>/`) and returns what the
/// gate must report for the lot, sorted.
fn write_cases(repo: &Repo, prefix: &str, cases: &[Case]) -> Vec<(String, u64, String, String)> {
    let mut expected = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let dir = format!("{prefix}{i:02}");
        for (file, content) in case.files {
            repo.write(&format!("{dir}/{file}"), content);
        }
        for (file, needle, code, severity) in case.expect {
            let content = case
                .files
                .iter()
                .find(|(name, _)| name == file)
                .unwrap_or_else(|| panic!("{}: no file `{file}`", case.name))
                .1;
            expected.push((
                format!("{dir}/{file}"),
                line_of(content, needle) as u64,
                code.to_string(),
                severity.to_string(),
            ));
        }
    }
    expected.sort();
    expected
}

/// The severity matrix: every exact form is reported under its code at its severity,
/// on its line, and every near miss beside it is not reported at all.
#[test]
fn each_exact_form_is_reported_at_its_severity_and_no_near_miss_is() {
    let repo = repo_with("");
    let mut expected = write_cases(&repo, "exact", EXACT_FORMS);
    let none = write_cases(&repo, "near", NEGATIVE_CONTROLS);
    assert!(none.is_empty(), "a negative control expects no finding");
    expected.sort();
    repo.commit("feat: harness changes");
    let run = check(&repo);
    assert_eq!(run.code, 1, "{}\n{}", run.stdout, run.stderr);

    let found = findings(&run);
    // Each case by name, so a miss says which form it is.
    for (prefix, cases) in [("exact", EXACT_FORMS), ("near", NEGATIVE_CONTROLS)] {
        for (i, case) in cases.iter().enumerate() {
            let dir = format!("{prefix}{i:02}/");
            let got: Vec<_> = found.iter().filter(|f| f.0.starts_with(&dir)).collect();
            let want: Vec<_> = expected.iter().filter(|f| f.0.starts_with(&dir)).collect();
            assert_eq!(got, want, "{}", case.name);
        }
    }
    assert_eq!(found, expected);

    // Every code is reported, the three exact slices at `error`, the exit form at
    // `warning`, and no code at both.
    for (code, severity) in [
        (TEST_MAIN, "error"),
        (PYTEST, "error"),
        (UNITTEST, "error"),
        (EXIT_ZERO, "warning"),
    ] {
        let severities: std::collections::BTreeSet<&str> = found
            .iter()
            .filter(|f| f.2 == code)
            .map(|f| f.3.as_str())
            .collect();
        assert_eq!(
            severities.into_iter().collect::<Vec<_>>(),
            vec![severity],
            "{code}"
        );
    }
    // Each finding states its scope limit and names the directive that lifts it.
    for v in run.violations(GATE) {
        let message = v["message"].as_str().unwrap();
        assert!(message.ends_with(SCOPE_LIMIT), "{message}");
        let fix = v["remediation"].as_str().unwrap();
        assert!(fix.contains("allow-harness-tampering: "), "{fix}");
    }
    // The benchmarks-only package and the function that is not the go tool's are said,
    // not passed in silence.
    let notes = notes(&run);
    assert!(
        notes.iter().any(|n| n.ends_with(
            "`TestMain` not judged, the test files of its package declare benchmarks and no test"
        )),
        "{notes:?}"
    );
}

/// The list of negative controls is checked in and has a floor: a control cannot be
/// dropped from it without this test saying so.
#[test]
fn the_negative_controls_are_enumerated() {
    assert_eq!(EXACT_FORMS.len(), 27);
    assert_eq!(NEGATIVE_CONTROLS.len(), 38);
    assert!(NEGATIVE_CONTROLS.iter().all(|c| c.expect.is_empty()));
    assert!(EXACT_FORMS.iter().all(|c| !c.expect.is_empty()));
    let mut names: Vec<&str> = EXACT_FORMS
        .iter()
        .chain(NEGATIVE_CONTROLS)
        .map(|c| c.name)
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), EXACT_FORMS.len() + NEGATIVE_CONTROLS.len());
    // Each detector has near misses of its own.
    for prefix in ["go: ", "pytest: ", "unittest: ", "exit: "] {
        let controls = NEGATIVE_CONTROLS
            .iter()
            .filter(|c| c.name.starts_with(prefix))
            .count();
        assert!(controls >= 4, "{prefix}{controls}");
    }
}

/// The gate is off until a repository turns it on, and a whole-tree run does not
/// evaluate it: its rule describes a change.
#[test]
fn the_gate_is_off_by_default_and_not_evaluated_on_a_whole_tree() {
    let conftest = "def pytest_runtest_logreport(report):\n    report.outcome = \"passed\"\n";
    let off = Repo::new();
    off.write("conftest.py", conftest);
    off.commit("feat: a conftest");
    let run = check(&off);
    assert_eq!(run.outcome(GATE)["enabled"], false);
    assert!(run.violations(GATE).is_empty());

    let on = repo_with("");
    on.write("conftest.py", conftest);
    on.commit("feat: a conftest");
    let run = check(&on);
    assert_eq!(findings(&run).len(), 1, "{}", run.stdout);
    assert_eq!(run.outcome(GATE)["examined"], 1);

    // A whole-tree baseline records the change-scoped findings in reach, and none of
    // this gate's: a tree has no change for the rule to describe.
    let diff = on.run(
        &["baseline", "--write", "--all-severities", "--base", "main"],
        &[],
    );
    assert_eq!(diff.code, 0, "{}{}", diff.stdout, diff.stderr);
    let recorded = std::fs::read_to_string(on.file("discipline-baseline.toml")).unwrap();
    assert!(recorded.contains(PYTEST), "{recorded}");
    std::fs::remove_file(on.file("discipline-baseline.toml")).unwrap();
    let whole = on.run(
        &["baseline", "--write", "--all-severities", "--whole-tree"],
        &[],
    );
    assert_eq!(whole.code, 0, "{}{}", whole.stdout, whole.stderr);
    let recorded = std::fs::read_to_string(on.file("discipline-baseline.toml")).unwrap();
    assert!(!recorded.contains(GATE), "{recorded}");
}

/// The notes say which harness files were read and under which version of the forms,
/// and a change with none says that.
#[test]
fn the_notes_name_the_examined_harness_files_and_the_pattern_version() {
    let repo = repo_with("");
    repo.write("pkg/conftest.py", "import pytest\n");
    repo.write("pkg/p_test.go", "package p\n");
    repo.write("pkg/lib.py", "import sys\n\nsys.exit(0)\n");
    repo.commit("feat: files");
    let run = check(&repo);
    assert!(findings(&run).is_empty(), "{}", run.stdout);
    assert_eq!(run.outcome(GATE)["examined"], 2);
    assert_eq!(
        notes(&run),
        vec!["pattern version 2; examined 2 harness file(s): pkg/conftest.py, pkg/p_test.go"]
    );

    let none = repo_with("");
    none.write("src/extra.rs", "pub fn two() -> u8 {\n    2\n}\n");
    none.commit("feat: a function");
    let run = check(&none);
    assert_eq!(run.outcome(GATE)["examined"], 0);
    assert_eq!(
        notes(&run),
        vec!["pattern version 2; no harness file among the changed files"]
    );
}

/// A configuration at the root of the repository resolves its entries against the root.
#[test]
fn a_configuration_at_the_repository_root_names_its_set_up_files() {
    let repo = repo_with("");
    repo.write(
        "package.json",
        "{\n  \"name\": \"root\",\n  \"jest\": {\n    \"globalTeardown\": \"<rootDir>/test/teardown.js\",\n    \"setupFiles\": [\"./test/env\"]\n  }\n}\n",
    );
    repo.write("test/teardown.js", "process.exit(0);\n");
    repo.write("test/env.ts", "process.exit(0);\n");
    repo.write("test/unnamed.js", "process.exit(0);\n");
    repo.commit("feat: set-up files");
    let run = check(&repo);
    assert_eq!(
        findings(&run),
        vec![
            (
                "test/env.ts".to_string(),
                1,
                EXIT_ZERO.to_string(),
                "warning".to_string()
            ),
            (
                "test/teardown.js".to_string(),
                1,
                EXIT_ZERO.to_string(),
                "warning".to_string()
            ),
        ],
        "{}",
        run.stdout
    );
    assert_eq!(run.outcome(GATE)["examined"], 2);
}

const TAMPERED_CONFTEST: &str = "import os\n\n\ndef pytest_sessionfinish(session, exitstatus):\n    os._exit(0)\n\n\ndef pytest_runtest_logreport(report):\n    report.outcome = \"passed\"\n";

/// Only what the change adds is reported: a form the base side of the harness file
/// already held is not, whether the file is untouched, edited elsewhere or moved.
#[test]
fn a_form_the_base_side_already_held_is_not_reported_again() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &enabled()),
            ("conftest.py", TAMPERED_CONFTEST),
            ("pkg/conftest.py", TAMPERED_CONFTEST),
        ],
        "chore: base",
    );
    // Untouched: not a changed file, not examined.
    repo.write("src/extra.rs", "pub fn two() -> u8 {\n    2\n}\n");
    repo.commit("feat: a function");
    let run = check(&repo);
    assert!(findings(&run).is_empty());
    assert_eq!(run.outcome(GATE)["examined"], 0);

    // Edited elsewhere, and moved: examined, nothing new.
    repo.write(
        "conftest.py",
        &format!("\"\"\"Shared fixtures.\"\"\"\n{TAMPERED_CONFTEST}"),
    );
    std::fs::create_dir_all(repo.file("pkg/sub")).unwrap();
    repo.git(&["mv", "pkg/conftest.py", "pkg/sub/conftest.py"]);
    repo.commit("refactor: move a conftest");
    let run = check(&repo);
    assert!(findings(&run).is_empty(), "{:?}", findings(&run));
    assert_eq!(run.outcome(GATE)["examined"], 2);

    // A second form of a kind the file already held, in another hook: that one is new.
    repo.write(
        "conftest.py",
        &format!("{TAMPERED_CONFTEST}\n\ndef pytest_runtest_makereport(item, call):\n    report = (yield).get_result()\n    report.outcome = \"passed\"\n"),
    );
    repo.commit("feat: another hook");
    let run = check(&repo);
    assert_eq!(
        findings(&run),
        vec![(
            "conftest.py".to_string(),
            14,
            PYTEST.to_string(),
            "error".to_string()
        )]
    );
}

/// A file that was not a harness file on the base side held nothing the runner ran: a
/// rename into a harness name, and a configuration that now names an unchanged file,
/// report every form in it.
#[test]
fn a_file_that_becomes_a_harness_file_is_read_as_added() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &enabled()),
            ("support/hooks.py", TAMPERED_CONFTEST),
            ("web/jest.config.json", "{\n  \"testEnvironment\": \"node\"\n}\n"),
            ("web/test/teardown.js", "module.exports = async () => {\n  process.exit(0);\n};\n"),
            (
                "crate/Cargo.toml",
                "[package]\nname = \"c\"\nversion = \"0.1.0\"\n\n[[test]]\nname = \"own\"\npath = \"checks/own.rs\"\ntest = false\n",
            ),
            ("crate/checks/own.rs", "fn main() {\n    std::process::exit(0);\n}\n"),
        ],
        "chore: base",
    );
    // The control: nothing changed, so nothing is examined.
    repo.write("src/extra.rs", "pub fn two() -> u8 {\n    2\n}\n");
    repo.commit("feat: a function");
    assert!(findings(&check(&repo)).is_empty());

    repo.git(&["mv", "support/hooks.py", "support/conftest.py"]);
    repo.write(
        "web/jest.config.json",
        "{\n  \"testEnvironment\": \"node\",\n  \"globalTeardown\": \"<rootDir>/test/teardown.js\"\n}\n",
    );
    repo.write(
        "crate/Cargo.toml",
        "[package]\nname = \"c\"\nversion = \"0.1.0\"\n\n[[test]]\nname = \"own\"\npath = \"checks/own.rs\"\nharness = false\n",
    );
    repo.commit("chore: wire the harness");
    let run = check(&repo);
    assert_eq!(
        findings(&run),
        vec![
            (
                "crate/checks/own.rs".to_string(),
                2,
                EXIT_ZERO.to_string(),
                "warning".to_string()
            ),
            (
                "support/conftest.py".to_string(),
                5,
                EXIT_ZERO.to_string(),
                "warning".to_string()
            ),
            (
                "support/conftest.py".to_string(),
                9,
                PYTEST.to_string(),
                "error".to_string()
            ),
            (
                "web/test/teardown.js".to_string(),
                2,
                EXIT_ZERO.to_string(),
                "warning".to_string()
            ),
        ],
        "{}",
        run.stdout
    );
    let message = run
        .violations(GATE)
        .iter()
        .find(|v| v["file"] == "web/test/teardown.js")
        .unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.contains("a set-up file that `web/jest.config.json` names"),
        "{message}"
    );
    assert_eq!(run.outcome(GATE)["examined"], 3);
}

/// `allow-harness-tampering` lifts by file or by `path:line`, on a line of its own with
/// a reason, and no other way.
#[test]
fn the_directive_lifts_by_file_or_by_line_and_only_when_it_is_a_directive() {
    let repo = repo_with("");
    repo.write("pkg/conftest.py", TAMPERED_CONFTEST);
    repo.commit("feat: a conftest");
    let both = vec![
        (
            "pkg/conftest.py".to_string(),
            5,
            EXIT_ZERO.to_string(),
            "warning".to_string(),
        ),
        (
            "pkg/conftest.py".to_string(),
            9,
            PYTEST.to_string(),
            "error".to_string(),
        ),
    ];
    let with = |body: &str| repo.check_with_pr(&[], &format!("no-issue: a test change\n{body}\n"));

    assert_eq!(findings(&with("")), both);
    // The whole file, by its path or by its name.
    for subject in ["pkg/conftest.py", "conftest.py"] {
        let run = with(&format!(
            "allow-harness-tampering: {subject} the suite is run by an outer process that reads the junit file"
        ));
        assert!(findings(&run).is_empty(), "{subject}: {:?}", findings(&run));
        let overrides = run.outcome(GATE)["overrides"].as_array().unwrap().clone();
        assert_eq!(overrides.len(), 2, "{subject}");
        assert!(overrides
            .iter()
            .all(|o| o["directive"] == "allow-harness-tampering"));
        assert_eq!(run.code, 0, "{}", run.stdout);
    }
    // One finding, by its line: the other stays.
    let run = with(
        "allow-harness-tampering: pkg/conftest.py:9 the outcome is rewritten for a known upstream bug, tracked in #12",
    );
    assert_eq!(findings(&run), both[..1].to_vec());
    assert_eq!(run.outcome(GATE)["overrides"].as_array().unwrap().len(), 1);
    // Not a directive, or not this file, or no reason worth the name: nothing lifts.
    for body in [
        "We could write allow-harness-tampering: pkg/conftest.py if the outer process were trusted.",
        "| `allow-harness-tampering: pkg/conftest.py the outer process reads junit` |",
        "allow-harness-tampering: other/conftest.py the outer process reads the junit file",
        "allow-harness-tampering: pkg/conftest.py:7 the outer process reads the junit file",
        "allow-harness-tampering: pkg/conftest.py todo",
        "allow-harness-tampering: pkg/conftest.py",
        "allow-stub: pkg/conftest.py the outer process reads the junit file",
    ] {
        assert_eq!(findings(&with(body)), both, "{body}");
    }
}

/// The severity key lowers the exact forms; the exit form is never above `warning`.
/// `exempt_paths` takes a file out, and the run's exit status follows the severities.
#[test]
fn severity_and_exempt_paths_are_read_from_the_gates_table() {
    let exit_only = repo_with("");
    exit_only.write("conftest.py", "import os\n\nos._exit(0)\n");
    exit_only.commit("feat: a conftest");
    let run = check(&exit_only);
    assert_eq!(findings(&run).len(), 1);
    assert_eq!(findings(&run)[0].3, "warning");
    assert_eq!(
        run.code, 0,
        "a warning alone does not fail the run: {}",
        run.stdout
    );

    let lowered = repo_with("severity = \"warning\"\n");
    lowered.write("conftest.py", TAMPERED_CONFTEST);
    lowered.commit("feat: a conftest");
    let run = check(&lowered);
    let severities: Vec<String> = findings(&run).into_iter().map(|f| f.3).collect();
    assert_eq!(severities, vec!["warning", "warning"]);
    assert_eq!(run.code, 0, "{}", run.stdout);

    let exempt = repo_with("exempt_paths = [\"vendor/**\"]\n");
    exempt.write("vendor/pkg/conftest.py", TAMPERED_CONFTEST);
    exempt.write("pkg/conftest.py", TAMPERED_CONFTEST);
    exempt.commit("feat: two conftests");
    let run = check(&exempt);
    assert!(
        findings(&run).iter().all(|f| f.0 == "pkg/conftest.py"),
        "{:?}",
        findings(&run)
    );
    assert_eq!(findings(&run).len(), 2);
    assert_eq!(run.outcome(GATE)["examined"], 1);
    assert_eq!(run.code, 1);
}

/// The shortest text found that the Rust grammar does not finish (`test_bounded_parse`).
const UNFINISHED: &str = "(>\u{fffd}t(0(.t();}";

/// A harness file that cannot be parsed is named as not analysed and is not counted as
/// examined; one that parses with errors is read in part, and says so.
#[test]
fn a_harness_file_that_does_not_parse_is_named_never_passed_in_silence() {
    // The gates that stop on a file with no tree are off, so the run reaches this one.
    let mut config = enabled();
    for gate in [
        "assertion-reduction",
        "vacuous-tests",
        "ignored-tests",
        "unsafe-safety-comment",
        "deletion-rationale",
    ] {
        config.push_str(&format!("[gates.{gate}]\nenabled = false\n"));
    }
    let repo = Repo::new();
    repo.commit_base_files(
        &[("discipline.toml", &config), ("Cargo.toml", CARGO_PACKAGE)],
        "chore: base",
    );
    repo.write("tests/stuck.rs", UNFINISHED);
    repo.write(
        "conftest.py",
        "def pytest_runtest_logreport(report):\n    report.outcome = \"passed\"\n\n\ndef broken(:\n    pass\n",
    );
    repo.commit("feat: harness files");
    let run = check(&repo);
    assert!(run.json()["could_not_check"].is_null(), "{}", run.stdout);
    let notes = notes(&run);
    assert!(
        notes.contains(
            &"`tests/stuck.rs` (the root of a Cargo test target): NOT analysed, it could not be parsed (the parser did not finish within its budget of 259 steps for 15 bytes)"
                .to_string()
        ),
        "{notes:?}"
    );
    assert!(
        notes.contains(
            &"`conftest.py` (a pytest `conftest.py`): parsed with syntax errors, so it was read in part"
                .to_string()
        ),
        "{notes:?}"
    );
    // What could be read is still reported, and only the parsed file counts as examined.
    assert_eq!(
        findings(&run),
        vec![(
            "conftest.py".to_string(),
            2,
            PYTEST.to_string(),
            "error".to_string()
        )]
    );
    assert_eq!(run.outcome(GATE)["examined"], 1);
    assert!(
        notes.contains(&"pattern version 2; examined 1 harness file(s): conftest.py".to_string()),
        "{notes:?}"
    );
}

/// A configuration whose list of set-up files is computed cannot say which files the
/// runner loads: the changed files below it are counted in a note, not examined and not
/// passed in silence. A `TestMain` that hands its `*testing.M` on is noted the same way.
#[test]
fn what_cannot_be_judged_is_noted() {
    let repo = Repo::new();
    repo.commit_base_files(
        &[
            ("discipline.toml", &enabled()),
            (
                "web/jest.config.js",
                "const base = require('./base');\n\nmodule.exports = { ...base, globalSetup: require.resolve('./test/setup') };\n",
            ),
        ],
        "chore: base",
    );
    repo.write("web/test/setup.js", "process.exit(0);\n");
    repo.write("other/tool.js", "process.exit(0);\n");
    repo.write(
        "pkg/p_test.go",
        "package p\n\nimport (\n\t\"os\"\n\t\"testing\"\n)\n\nfunc TestMain(m *testing.M) {\n\tos.Exit(run(m))\n}\n\nfunc run(m *testing.M) int {\n\treturn 0\n}\n",
    );
    repo.commit("feat: set-up");
    let run = check(&repo);
    assert!(findings(&run).is_empty(), "{:?}", findings(&run));
    let notes = notes(&run);
    assert!(
        notes.contains(
            &"`web/jest.config.js`: the files it has the runner load could not be read (it does not parse, or the list is computed); 1 changed file(s) below it were NOT examined as harness files"
                .to_string()
        ),
        "{notes:?}"
    );
    assert!(
        notes.contains(
            &"`pkg/p_test.go`: `TestMain` hands its `*testing.M` to other code: whether the tests run is not judged"
                .to_string()
        ),
        "{notes:?}"
    );
}

/// A pre-commit run has no commit message to carry the directive yet: the findings are
/// reported, as warnings.
#[test]
fn a_staged_run_reports_the_findings_as_warnings() {
    let repo = repo_with("");
    repo.write("conftest.py", TAMPERED_CONFTEST);
    repo.git(&["add", "-A"]);
    let run = repo.check(&["--staged"]);
    let severities: Vec<String> = findings(&run).into_iter().map(|f| f.3).collect();
    assert_eq!(severities, vec!["warning", "warning"], "{}", run.stdout);
}
