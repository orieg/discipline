//! A skip marker is read from the syntax tree, never from the text of an argument (#651).
//!
//! A display name, a description, a parameter value or a comment that happens to contain
//! `Skip`, `skip:`, `.skip`, `enabled = false` or `[.` does not make a test skipped. The
//! real marker beside each case still does.
//!
//! Each case drives the real binary over a base side whose test runs and a head side
//! that changes the test's marker. The sources are fixtures built by this file, not tests
//! of this suite.

mod common;
use common::Repo;

struct Case {
    name: &'static str,
    path: &'static str,
    base: &'static str,
    head: &'static str,
    /// Whether `ignored-tests` reports one `Existing Test Skipped`.
    skipped: bool,
}

fn titles(case: &Case) -> (i32, Vec<String>) {
    let repo = Repo::new();
    repo.commit_base_files(&[(case.path, case.base)], "test: base suite");
    repo.write(case.path, case.head);
    repo.commit("test: change the marker");
    let run = repo.check(&[]);
    (run.code, run.titles("ignored-tests"))
}

fn differing(cases: &[Case]) -> Vec<String> {
    let mut wrong = Vec::new();
    for case in cases {
        let (code, titles) = titles(case);
        let ok = if case.skipped {
            code == 1 && titles == ["Existing Test Skipped"]
        } else {
            code == 0 && titles.is_empty()
        };
        if !ok {
            wrong.push(format!(
                "{}: wanted skipped={}, got exit {code} with {titles:?}",
                case.name, case.skipped
            ));
        }
    }
    wrong
}

const CS_BASE: &str = "using Xunit;\npublic class CartTests {\n    [Fact]\n    public void Total() {\n        Assert.Equal(3, Cart.Total());\n    }\n}\n";
const PY_BASE: &str =
    "import pytest\n\n\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n";
const RB_BASE: &str =
    "RSpec.describe Cart do\n  it \"sums\" do\n    expect(Cart.total).to eq(3)\n  end\nend\n";
const JS_BASE: &str = "test(\"sums\", () => {\n  expect(total()).toBe(3);\n});\n";
const JAVA_BASE: &str = "import org.testng.annotations.Test;\n\npublic class CartTest {\n    @Test\n    public void total() {\n        assertEquals(3, Cart.total());\n    }\n}\n";
const KT_BASE: &str = "import org.testng.annotations.Test\n\nclass CartTest {\n    @Test\n    fun total() {\n        assertEquals(3, Cart.total())\n    }\n}\n";
const CPP_BASE: &str = "#include <catch2/catch_test_macros.hpp>\n\nTEST_CASE(\"sums\", \"[cart]\") {\n    REQUIRE(total() == 3);\n}\n";

fn text_only_cases() -> Vec<Case> {
    vec![
        Case {
            name: "C# display name containing Skip",
            path: "tests/CartTests.cs",
            base: CS_BASE,
            head: "using Xunit;\npublic class CartTests {\n    [Fact(DisplayName = \"Skip logic\")]\n    public void Total() {\n        Assert.Equal(3, Cart.Total());\n    }\n}\n",
            skipped: false,
        },
        Case {
            name: "C# timeout beside a display name naming Skip =",
            path: "tests/CartTests.cs",
            base: CS_BASE,
            head: "using Xunit;\npublic class CartTests {\n    [Fact(DisplayName = \"Skip = later\", Timeout = 100)]\n    public void Total() {\n        Assert.Equal(3, Cart.Total());\n    }\n}\n",
            skipped: false,
        },
        Case {
            name: "Python parameter value naming skipIf",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\n\n\n@pytest.mark.parametrize(\"name\", [\"skipIf\", \"pytest.mark.skip\"])\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: false,
        },
        Case {
            name: "Python pytestmark fixture named in a string",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\n\npytestmark = pytest.mark.usefixtures(\"skip_db\")\n\n\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: false,
        },
        Case {
            name: "Python pytestmark list with a fixture named in a string",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\n\npytestmark = [pytest.mark.usefixtures(\"xfail_db\")]\n\n\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: false,
        },
        Case {
            name: "Ruby description containing skip:",
            path: "spec/cart_spec.rb",
            base: RB_BASE,
            head: "RSpec.describe Cart do\n  it \"honours the skip: option\" do\n    expect(Cart.total).to eq(3)\n  end\nend\n",
            skipped: false,
        },
        Case {
            name: "JavaScript table value containing .skip",
            path: "tests/cart.test.js",
            base: JS_BASE,
            head: "test.each([\".skip\", \"b\"])(\"sums\", (name) => {\n  expect(total(name)).toBe(3);\n});\n",
            skipped: false,
        },
        Case {
            name: "Java description containing enabled = false",
            path: "src/test/java/CartTest.java",
            base: JAVA_BASE,
            head: "import org.testng.annotations.Test;\n\npublic class CartTest {\n    @Test(description = \"note: enabled = false, twice\")\n    public void total() {\n        assertEquals(3, Cart.total());\n    }\n}\n",
            skipped: false,
        },
        Case {
            name: "Kotlin description containing enabled = false",
            path: "src/test/kotlin/CartTest.kt",
            base: KT_BASE,
            head: "import org.testng.annotations.Test\n\nclass CartTest {\n    @Test(description = \"note: enabled = false, twice\")\n    fun total() {\n        assertEquals(3, Cart.total())\n    }\n}\n",
            skipped: false,
        },
        Case {
            name: "Catch2 test name containing [.",
            path: "tests/cart_test.cpp",
            base: CPP_BASE,
            head: "#include <catch2/catch_test_macros.hpp>\n\nTEST_CASE(\"sums [.5] ranges\", \"[cart]\") {\n    REQUIRE(total() == 3);\n}\n",
            skipped: false,
        },
    ]
}

fn real_marker_cases() -> Vec<Case> {
    vec![
        Case {
            name: "C# Skip argument",
            path: "tests/CartTests.cs",
            base: CS_BASE,
            head: "using Xunit;\npublic class CartTests {\n    [Fact(DisplayName = \"Total\", Skip = \"later\")]\n    public void Total() {\n        Assert.Equal(3, Cart.Total());\n    }\n}\n",
            skipped: true,
        },
        Case {
            name: "Python skip mark with a string reason",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\n\n\n@pytest.mark.skip(reason=\"later\")\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: true,
        },
        // A decorator spelled `skip..` that another file binds is a skip by its spelling
        // (`docs/GATES.md`, the limits of `ignored-tests`): unchanged.
        Case {
            name: "Python decorator spelled skip.. from another file",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\nfrom helpers import skip_slow\n\n\n@skip_slow\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: true,
        },
        Case {
            name: "Python name the file binds to a skip mark",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\n\nskip_slow = pytest.mark.skip(reason=\"slow\")\n\n\n@skip_slow\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: true,
        },
        Case {
            name: "Python skip imported from unittest",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\nfrom unittest import skip\n\n\n@skip(\"later\")\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: true,
        },
        Case {
            name: "Python unittest.skip and a parametrized case marked skip",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\n\n\n@pytest.mark.parametrize(\"name\", [pytest.param(\"a\", marks=pytest.mark.skip)])\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: true,
        },
        Case {
            name: "Python pytestmark skip",
            path: "tests/test_cart.py",
            base: PY_BASE,
            head: "import pytest\n\npytestmark = [pytest.mark.usefixtures(\"db\"), pytest.mark.skip(\"later\")]\n\n\ndef test_total(name=\"a\"):\n    assert total(name) == 3\n",
            skipped: true,
        },
        Case {
            name: "Ruby skip metadata",
            path: "spec/cart_spec.rb",
            base: RB_BASE,
            head: "RSpec.describe Cart do\n  it \"sums\", skip: \"later\" do\n    expect(Cart.total).to eq(3)\n  end\nend\n",
            skipped: true,
        },
        Case {
            name: "Ruby skip symbol",
            path: "spec/cart_spec.rb",
            base: RB_BASE,
            head: "RSpec.describe Cart do\n  it \"sums\", :skip do\n    expect(Cart.total).to eq(3)\n  end\nend\n",
            skipped: true,
        },
        Case {
            name: "JavaScript skip before a table",
            path: "tests/cart.test.js",
            base: JS_BASE,
            head: "test.skip.each([\"a\", \"b\"])(\"sums\", (name) => {\n  expect(total(name)).toBe(3);\n});\n",
            skipped: true,
        },
        Case {
            name: "Java enabled = false with uneven spacing",
            path: "src/test/java/CartTest.java",
            base: JAVA_BASE,
            head: "import org.testng.annotations.Test;\n\npublic class CartTest {\n    @Test(description = \"total\", enabled  =  false)\n    public void total() {\n        assertEquals(3, Cart.total());\n    }\n}\n",
            skipped: true,
        },
        Case {
            name: "Kotlin enabled = false",
            path: "src/test/kotlin/CartTest.kt",
            base: KT_BASE,
            head: "import org.testng.annotations.Test\n\nclass CartTest {\n    @Test(description = \"total\", enabled = false)\n    fun total() {\n        assertEquals(3, Cart.total())\n    }\n}\n",
            skipped: true,
        },
        Case {
            name: "Catch2 hidden tag",
            path: "tests/cart_test.cpp",
            base: CPP_BASE,
            head: "#include <catch2/catch_test_macros.hpp>\n\nTEST_CASE(\"sums\", \"[.][cart]\") {\n    REQUIRE(total() == 3);\n}\n",
            skipped: true,
        },
    ]
}

#[test]
fn text_inside_an_argument_is_not_a_skip_marker() {
    let wrong = differing(&text_only_cases());
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_marker_itself_is_still_a_skip() {
    let wrong = differing(&real_marker_cases());
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
