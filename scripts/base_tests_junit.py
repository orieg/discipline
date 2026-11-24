"""JUnit output for the two `base-tests-v1` (#1052) runners that write none.

The SWE-bench images of django and sympy hold neither pytest nor a JUnit reporter, and the
runs have no network, so the `base-tests` preset (which reads JUnit) would read no case from
the repositories' own runners. `scripts/base_tests_lab.py` copies this file into each
container as `/opt/base-tests/base_tests_junit.py` (with `bt_settings.py`, below) and the
per-repository runner commands use it:

- django: `./tests/runtests.py --settings=bt_settings` with this directory on PYTHONPATH.
  `bt_settings` is the repository's `test_sqlite` settings plus
  `TEST_RUNNER = "base_tests_junit.JUnitDiscoverRunner"`: Django's own runner, unchanged,
  whose result class also records each test.
- sympy: `python /opt/base-tests/base_tests_junit.py sympy <files>` runs sympy's own
  `test()` runner (what `bin/test` calls) and records each test through its reporter.

Both write `junit.xml` to `$BT_JUNIT_DIR` (the command's working directory), where the
preset and the lab runner read it. A test that errors or fails is a `<failure>` (in django an
unexpected success too); a skip is `<skipped>`; an expected failure passes, and so does a
sympy XPASS, which sympy's own runner does not count as a failure. Runs under Python 3.6.
"""

import os
import sys
from xml.sax.saxutils import escape, quoteattr

BT_SETTINGS = '''from test_sqlite import *  # noqa: F401,F403
TEST_RUNNER = "base_tests_junit.JUnitDiscoverRunner"
'''


def write_junit(cases):
    """`cases`: (classname, name, status, message) in run order; a later failure of the
    same case wins over its pass (a subtest that failed)."""
    merged = {}
    for cls, name, status, msg in cases:
        key = (cls, name)
        if key in merged and merged[key][0] == "failure":
            continue
        merged[key] = (status, msg)
    out = ["<testsuite>"]
    for (cls, name), (status, msg) in merged.items():
        head = "<testcase classname=%s name=%s" % (quoteattr(cls), quoteattr(name))
        if status == "passed":
            out.append(head + "/>")
        elif status == "skipped":
            out.append(head + "><skipped/></testcase>")
        else:
            out.append(head + "><failure>%s</failure></testcase>" % escape(msg[-2000:]))
    out.append("</testsuite>")
    path = os.path.join(os.environ.get("BT_JUNIT_DIR", "."), "junit.xml")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")


def split_id(test_id):
    cls, _, name = test_id.rpartition(".")
    return cls, name


# --- django -----------------------------------------------------------------------------

import unittest

try:
    from django.test.runner import DiscoverRunner
except ImportError:  # sympy images, or the host: no django runner to extend
    DiscoverRunner = None

if DiscoverRunner is not None:

    class JUnitResult(unittest.TextTestResult):
        def __init__(self, *a, **k):
            super().__init__(*a, **k)
            self.bt_cases = []

        def bt(self, test, status, msg=""):
            self.bt_cases.append(split_id(test.id()) + (status, msg))

        def addSuccess(self, test):
            super().addSuccess(test)
            self.bt(test, "passed")

        def addFailure(self, test, err):
            super().addFailure(test, err)
            self.bt(test, "failure", self._exc_info_to_string(err, test))

        def addError(self, test, err):
            super().addError(test, err)
            self.bt(test, "failure", self._exc_info_to_string(err, test))

        def addSkip(self, test, reason):
            super().addSkip(test, reason)
            self.bt(test, "skipped")

        def addExpectedFailure(self, test, err):
            super().addExpectedFailure(test, err)
            self.bt(test, "passed")

        def addUnexpectedSuccess(self, test):
            super().addUnexpectedSuccess(test)
            self.bt(test, "failure", "unexpected success")

        def addSubTest(self, test, subtest, err):
            super().addSubTest(test, subtest, err)
            if err is not None:
                self.bt(test, "failure", self._exc_info_to_string(err, test))

    class JUnitDiscoverRunner(DiscoverRunner):
        def get_resultclass(self):
            return JUnitResult

        def run_suite(self, suite, **kwargs):
            result = super().run_suite(suite, **kwargs)
            write_junit(getattr(result, "bt_cases", []))
            return result


# --- sympy ------------------------------------------------------------------------------


def run_sympy(files):
    sys.path.insert(0, os.getcwd())
    try:
        from sympy.testing import runtests as rt
    except ImportError:
        from sympy.utilities import runtests as rt
    cases = []
    state = {"cls": "", "name": ""}
    rep = rt.PyTestReporter

    def module_of(path):
        rel = os.path.relpath(path, os.getcwd())
        return rel[:-3].replace(os.sep, ".") if rel.endswith(".py") else rel

    def wrap(method, status):
        orig = getattr(rep, method)

        def f(self, *a, **k):
            msg = ""
            if status == "failure" and a and isinstance(a[0], tuple) and len(a[0]) == 3:
                import traceback

                msg = "".join(traceback.format_exception(*a[0]))
            cases.append((state["cls"], state["name"], status, msg))
            return orig(self, *a, **k)

        setattr(rep, method, f)

    orig_entering_file = rep.entering_filename
    orig_entering_test = rep.entering_test
    orig_import_error = rep.import_error

    def entering_filename(self, filename, n):
        state["cls"] = module_of(filename)
        return orig_entering_file(self, filename, n)

    def entering_test(self, f):
        state["name"] = f.__name__
        return orig_entering_test(self, f)

    def import_error(self, filename, exc_info):
        cases.append((module_of(filename), "<import>", "failure", str(exc_info[1])))
        return orig_import_error(self, filename, exc_info)

    rep.entering_filename = entering_filename
    rep.entering_test = entering_test
    rep.import_error = import_error
    for method, status in (("test_pass", "passed"), ("test_skip", "skipped"), ("test_xfail", "passed"), ("test_fail", "failure"), ("test_exception", "failure"), ("test_xpass", "passed")):
        wrap(method, status)
    # In-process: by default `test()` re-runs itself in a subprocess (to fix the hash seed),
    # where these reporter hooks would not be installed.
    ok = rt.test(*files, verbose=True, subprocess=False)
    write_junit(cases)
    return 0 if ok else 1


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "sympy":
        sys.exit(run_sympy(sys.argv[2:]))
    if len(sys.argv) == 2 and sys.argv[1] == "--write-settings":
        with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "bt_settings.py"), "w") as f:
            f.write(BT_SETTINGS)
        sys.exit(0)
    sys.exit("usage: base_tests_junit.py sympy <test files> | --write-settings")
