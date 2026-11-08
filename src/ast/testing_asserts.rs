//! Python testing-library assertion functions (#1038): `numpy.testing.assert_allclose`,
//! `pandas.testing.assert_frame_equal`, `xarray.testing.assert_identical` and the other
//! `assert_*` functions of those modules.
//!
//! A call is one of them only through the file's own imports: `from numpy.testing import
//! assert_allclose`, `import numpy.testing as npt` then `npt.assert_allclose(..)`, or
//! `import numpy as np` then `np.testing.assert_allclose(..)`. A bare `assert_allclose`
//! the file does not import, or one it defines itself (a `def` or `class` of that name
//! anywhere in the file), is not one: it is whatever the file made it.

use std::collections::{HashMap, HashSet};
use tree_sitter::Node;

/// The packages whose `testing` module holds the assertion functions.
const PACKAGES: &[&str] = &["numpy", "pandas", "xarray"];

/// The testing-library assertion functions a file can call, from its imports.
#[derive(Debug, Default)]
pub struct TestingAsserts {
    /// Names bound by `from <pkg>.testing import assert_x [as y]`: bound name -> `assert_x`.
    bare: HashMap<String, String>,
    /// A `from <pkg>.testing import *`: any `assert_*` name the file does not define.
    star: bool,
    /// Dotted names bound to a testing module: `npt`, `numpy.testing`, `np.testing`.
    modules: HashSet<String>,
    /// Names the file defines with `def` or `class`; they shadow an import.
    defined: HashSet<String>,
}

impl TestingAsserts {
    /// Reads every import statement of the file, wherever it is.
    pub fn read(root: Node, src: &[u8]) -> Self {
        let mut out = Self::default();
        let text = |n: Node| n.utf8_text(src).unwrap_or("").to_string();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "import_statement" => {
                    let mut cursor = node.walk();
                    for name in node.children_by_field_name("name", &mut cursor) {
                        let (module, alias) = match name.kind() {
                            "aliased_import" => (
                                name.child_by_field_name("name").map(text),
                                name.child_by_field_name("alias").map(text),
                            ),
                            _ => (Some(text(name)), None),
                        };
                        let Some(module) = module else { continue };
                        if is_testing_module(&module) {
                            // `import numpy.testing` binds `numpy`; the dotted name reaches it.
                            out.modules.insert(alias.unwrap_or(module));
                        } else if PACKAGES.contains(&module.as_str()) {
                            let bound = alias.unwrap_or(module);
                            out.modules.insert(format!("{bound}.testing"));
                        }
                    }
                }
                "import_from_statement" => {
                    let Some(module) = node.child_by_field_name("module_name").map(text) else {
                        continue;
                    };
                    let testing = is_testing_module(&module);
                    let package = PACKAGES.contains(&module.as_str());
                    if !testing && !package {
                        continue;
                    }
                    let mut cursor = node.walk();
                    if testing
                        && node
                            .children(&mut cursor)
                            .any(|c| c.kind() == "wildcard_import")
                    {
                        out.star = true;
                    }
                    let mut cursor = node.walk();
                    for name in node.children_by_field_name("name", &mut cursor) {
                        let (item, bound) = match name.kind() {
                            "aliased_import" => (
                                name.child_by_field_name("name").map(text),
                                name.child_by_field_name("alias").map(text),
                            ),
                            _ => (Some(text(name)), None),
                        };
                        let Some(item) = item else { continue };
                        let bound = bound.unwrap_or_else(|| item.clone());
                        if testing && item.starts_with("assert_") {
                            out.bare.insert(bound, item);
                        } else if package && item == "testing" {
                            // `from numpy import testing as npt`
                            out.modules.insert(bound);
                        }
                    }
                }
                "function_definition" | "class_definition" => {
                    if let Some(name) = node.child_by_field_name("name") {
                        out.defined.insert(text(name));
                    }
                    stack.extend(node.named_children(&mut node.walk()));
                }
                _ => stack.extend(node.named_children(&mut node.walk())),
            }
        }
        out
    }

    /// The function a callee names (`assert_allclose`), when it is a testing-library
    /// assertion function under the file's imports.
    pub fn function<'s>(&'s self, callee: Node, src: &'s [u8]) -> Option<&'s str> {
        match callee.kind() {
            "identifier" => {
                let name = callee.utf8_text(src).ok()?;
                if self.defined.contains(name) {
                    return None;
                }
                if let Some(item) = self.bare.get(name) {
                    return Some(item);
                }
                (self.star && name.starts_with("assert_")).then_some(name)
            }
            "attribute" => {
                let attr = callee
                    .child_by_field_name("attribute")?
                    .utf8_text(src)
                    .ok()?;
                let object = dotted(callee.child_by_field_name("object")?, src)?;
                (attr.starts_with("assert_") && self.modules.contains(&object)).then_some(attr)
            }
            _ => None,
        }
    }
}

/// Whether a testing-library assertion function compares a value with an expected one
/// (`assert_allclose`, `assert_frame_equal`, `assert_identical`): its first two
/// positional arguments are the value and what it must equal.
pub fn compares(function: &str) -> bool {
    ["equal", "close", "identical"]
        .iter()
        .any(|w| function.contains(w))
}

/// Whether a testing-library assertion function pins a value or a failure, the way
/// `self.assertEqual` and `self.assertRaises` do; `assert_` (truthiness) and
/// `assert_array_less` (an ordering) do not.
pub fn is_strong(function: &str) -> bool {
    compares(function) || function.contains("raises") || function.contains("warns")
}

fn is_testing_module(module: &str) -> bool {
    module
        .strip_suffix(".testing")
        .is_some_and(|p| PACKAGES.contains(&p))
}

/// `a.b.c` of a plain dotted name, without the whitespace or parentheses a source may
/// put between its parts.
fn dotted(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => Some(node.utf8_text(src).ok()?.to_string()),
        "attribute" => {
            let object = dotted(node.child_by_field_name("object")?, src)?;
            let attr = node.child_by_field_name("attribute")?.utf8_text(src).ok()?;
            Some(format!("{object}.{attr}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The function each top-level call of `src` names, in order (`None` where it is not one).
    fn resolved(src: &str) -> Vec<Option<String>> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let tree = crate::ast::source_text::parse(&mut parser, src).unwrap();
        let asserts = TestingAsserts::read(tree.root_node(), src.as_bytes());
        let mut out = Vec::new();
        let mut stack = vec![tree.root_node()];
        let mut calls = Vec::new();
        while let Some(n) = stack.pop() {
            if n.kind() == "call" {
                calls.push(n);
            }
            stack.extend(n.named_children(&mut n.walk()));
        }
        calls.sort_by_key(|c| c.start_byte());
        for call in calls {
            let callee = call.child_by_field_name("function").unwrap();
            out.push(asserts.function(callee, src.as_bytes()).map(str::to_string));
        }
        out
    }

    #[test]
    fn each_import_form_resolves_its_calls() {
        let some = |s: &str| Some(s.to_string());
        let cases: &[(&str, Option<String>)] = &[
            (
                "from numpy.testing import assert_allclose\nassert_allclose(a, 1)\n",
                some("assert_allclose"),
            ),
            (
                "from numpy.testing import assert_allclose as aac\naac(a, 1)\n",
                some("assert_allclose"),
            ),
            (
                "import numpy.testing as npt\nnpt.assert_array_equal(a, 1)\n",
                some("assert_array_equal"),
            ),
            (
                "import numpy.testing\nnumpy.testing.assert_equal(a, 1)\n",
                some("assert_equal"),
            ),
            (
                "import numpy as np\nnp.testing.assert_allclose(a, 1)\n",
                some("assert_allclose"),
            ),
            (
                "import pandas as pd\npd.testing.assert_frame_equal(a, b)\n",
                some("assert_frame_equal"),
            ),
            (
                "from pandas.testing import assert_series_equal\nassert_series_equal(a, b)\n",
                some("assert_series_equal"),
            ),
            (
                "import xarray as xr\nxr.testing.assert_identical(a, b)\n",
                some("assert_identical"),
            ),
            (
                "from xarray import testing as xrt\nxrt.assert_allclose(a, b)\n",
                some("assert_allclose"),
            ),
            (
                "from xarray.testing import *\nassert_identical(a, b)\n",
                some("assert_identical"),
            ),
            (
                "def test_t():\n    from numpy.testing import assert_allclose\n    assert_allclose(a, 1)\n",
                some("assert_allclose"),
            ),
        ];
        for (src, want) in cases {
            assert_eq!(resolved(src), vec![want.clone()], "{src}");
        }
    }

    #[test]
    fn a_name_the_file_does_not_import_or_defines_itself_is_not_resolved() {
        for src in [
            // Not imported at all.
            "assert_allclose(a, 1)\n",
            // A helper of the file's own.
            "def assert_allclose(a, b):\n    pass\n\nassert_allclose(a, 1)\n",
            // Imported, then shadowed by a local definition.
            "from numpy.testing import assert_allclose\n\ndef assert_allclose(a, b):\n    pass\n\nassert_allclose(a, 1)\n",
            // Another library's function of the same name.
            "from mylib.testing import assert_allclose\nassert_allclose(a, 1)\n",
            // A module the file does not bind to a testing module.
            "import numpy as np\nnpt.assert_allclose(a, 1)\n",
            "import numpy as np\nnp.assert_allclose(a, 1)\n",
            // A non-assertion function of a testing module.
            "import numpy.testing as npt\nnpt.suppress_warnings(a)\n",
            // A star import of another module.
            "from mylib import *\nassert_identical(a, b)\n",
        ] {
            assert_eq!(resolved(src), vec![None], "{src}");
        }
    }

    #[test]
    fn strength_and_comparison_follow_the_function() {
        for f in ["assert_allclose", "assert_array_equal", "assert_identical"] {
            assert!(compares(f) && is_strong(f), "{f}");
        }
        assert!(is_strong("assert_raises") && !compares("assert_raises"));
        for f in ["assert_", "assert_array_less", "assert_no_warnings"] {
            assert!(!is_strong(f) && !compares(f), "{f}");
        }
    }
}
