//! The ancestors and siblings of a node, read without the parser library's
//! `Node::parent`.
//!
//! The library keeps no link from a node to its parent: `Node::parent` starts at the root
//! and descends to the node, so one call costs the node's depth, and a loop that climbs
//! from a node to the root with it costs the depth squared. The four sibling methods
//! (`prev_sibling`, `next_sibling` and their named forms) call it first, so each costs
//! the depth too. A walker that climbed once for each nested function did work cubic in
//! the nesting, on a source of a few kilobytes: neither the parse budget nor the depth
//! limit of `source_text` bounds that, and a run that does not finish reports nothing
//! (#669).
//!
//! [`Ancestry`] is made once for a tree and keeps the chain of nodes from the root to the
//! node it was last asked about. A question about a node on that chain is answered from
//! it; a question about a node below it extends it; a question about a node elsewhere
//! cuts it back to the deepest node that holds the asked one and extends it from there.
//! A walker that asks in source order therefore pays for each node of the tree once, and
//! a climb from a node pays one step for each level it climbs. A walker that descends
//! from the root can say where it stands ([`Ancestry::stand_at`]) and pay nothing for
//! the chain.
//!
//! A climb that reads every ancestor of every node still costs the depth for each node.
//! [`Ancestry::nearest`] is for those: it keeps what each entry of the chain answered to
//! a question ([`Above`]), so an ancestor that stays on the chain is asked once however
//! many nodes below it are read.
//!
//! Every answer is the library's own. The chain is built by the descent `Node::parent`
//! makes (`Node::child_with_descendant`); a node that descent does not reach gets the
//! node the descent ended at, as the library answers; a sibling is read from the
//! children of the parent, by the library's rule for a node of no width. The test
//! `every_answer_is_the_librarys` asks about every node of trees with and without
//! errors, in three orders, and compares.
//!
//! The library's five methods are not called under `src/ast/` outside this file:
//! `clippy.toml` disallows them.
//!
//! Work is counted in steps ([`steps`], in test builds only): one for each question,
//! each level descended, each chain entry compared, each child listed, and, in the walks
//! that count there, each node visited and each byte read. The count depends on the
//! source and on nothing else, so a test holds the steps for a nested source to a
//! multiple of the steps for one a quarter as deep, on any machine.

use std::cell::{Cell, RefCell};
use tree_sitter::Node;

#[cfg(test)]
thread_local! {
    static STEPS: Cell<u64> = const { Cell::new(0) };
}

/// Adds `n` to the count of steps of walker work (test builds only).
#[inline]
pub(crate) fn count(n: usize) {
    #[cfg(test)]
    STEPS.with(|s| s.set(s.get().saturating_add(n as u64)));
    #[cfg(not(test))]
    let _ = n;
}

/// The steps counted on this thread while `work` ran.
#[cfg(test)]
pub(crate) fn steps<T>(work: impl FnOnce() -> T) -> (T, u64) {
    let before = STEPS.with(Cell::get);
    let out = work();
    (out, STEPS.with(Cell::get) - before)
}

/// The chain from the root of one tree to the node last asked about.
pub struct Ancestry<'t> {
    /// The root first; each entry is the parent of the next.
    path: RefCell<Vec<Node<'t>>>,
    /// Where the last answer stands in `path`: a climb asks about it next.
    hint: Cell<usize>,
    /// The children of the node whose siblings were last asked about, by its id.
    family: RefCell<(usize, Vec<Node<'t>>)>,
    /// For each question of [`Above`], what the entries of `path` answered, each for the
    /// entry below it: the id of that entry below, and one more than the place of the
    /// nearest entry at or above the asked one that said yes (0 when none did).
    answers: RefCell<Vec<Vec<(usize, u32)>>>,
    /// What readers keep for this tree ([`Ancestry::kept`]), by the type of what is kept.
    kept: RefCell<std::collections::HashMap<std::any::TypeId, std::rc::Rc<dyn std::any::Any>>>,
}

/// A question a walker asks of every ancestor of a node, for [`Ancestry::nearest`]. The
/// chain keeps each ancestor's answer, so one that stays on the chain is asked once
/// however many nodes below it are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Above {
    /// A macro definition (`self_comparison`).
    MacroDefinition,
    /// A loop whose body, or a closure run for each element, holds the node
    /// (`helper_loops`).
    Loop,
    /// A conditional (`helper_loops`).
    Conditional,
    /// A function (`ci_condition`).
    Function,
    /// A function or a block that scopes names (`ci_condition::more`).
    SkipScope,
    /// A Rust module under `#[cfg(test)]`.
    RustTestModule,
    /// A JS/TS declaration with no body to judge: ambient or interface.
    JsDeclaration,
    /// A JS/TS declaration with no body to judge, or an abstract class.
    JsDeclarationOrAbstractClass,
    /// A call of a JS/TS test runner.
    JsRunnerCall,
    /// A C# interface.
    CsharpInterface,
    /// A Python class.
    PythonClass,
}

impl<'t> Ancestry<'t> {
    /// The ancestry of the nodes of the tree whose root is `root`.
    pub fn new(root: Node<'t>) -> Self {
        Self {
            path: RefCell::new(vec![root]),
            hint: Cell::new(0),
            family: RefCell::new((0, Vec::new())),
            answers: RefCell::new(Vec::new()),
            kept: RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// What a reader keeps for this tree: `make()` the first time a `T` is asked for,
    /// and that same value each time after. A predicate that is handed a node and this
    /// ancestry and nothing else reads the file once here, and not once for each node
    /// it is asked about. What is kept is for the tree of this ancestry alone: another
    /// tree has another ancestry, and nothing of this one.
    pub fn kept<T: 'static>(&self, make: impl FnOnce() -> T) -> std::rc::Rc<T> {
        count(1);
        let key = std::any::TypeId::of::<T>();
        let known = self.kept.borrow().get(&key).cloned();
        let value = match known {
            Some(value) => value,
            None => {
                // `make` may itself ask for something kept: nothing is borrowed here.
                let made: std::rc::Rc<dyn std::any::Any> = std::rc::Rc::new(make());
                self.kept.borrow_mut().entry(key).or_insert(made).clone()
            }
        };
        value
            .downcast::<T>()
            .unwrap_or_else(|_| unreachable!("a value is kept under its own type"))
    }

    /// The root of the tree.
    pub fn root(&self) -> Node<'t> {
        self.path.borrow()[0]
    }

    /// The parent of `node`, as `Node::parent` answers: `None` for the root, and for a
    /// node no descent from the root reaches, the deepest node the descent did reach.
    pub fn parent(&self, node: Node<'t>) -> Option<Node<'t>> {
        count(1);
        let mut path = self.path.borrow_mut();
        let hint = self.hint.get();
        let at = match path.get(hint) {
            Some(known) if known.id() == node.id() => hint,
            _ => match seek(&mut path, node) {
                Some(at) => at,
                // The library's own answer for a node its descent does not reach.
                None => return path.last().copied(),
            },
        };
        let parent = at.checked_sub(1)?;
        self.hint.set(parent);
        Some(path[parent])
    }

    /// The nearest ancestor of `node` that `holds` says yes to, with its child on the
    /// way down to `node`: what a climb from `node` that asks `holds(ancestor, child)`
    /// at each level stops at. `holds` must answer for the two nodes alone, the same
    /// each time, since an answer is kept under `what` and not asked again.
    pub fn nearest(
        &self,
        node: Node<'t>,
        what: Above,
        holds: impl Fn(Node<'t>, Node<'t>) -> bool,
    ) -> Option<(Node<'t>, Node<'t>)> {
        count(1);
        let Some(at) = self.place_on_chain(node) else {
            return self.climb(node, &holds);
        };
        // The entry above `node` answers for every ancestor of `node`.
        let target = at.checked_sub(1)?;
        let key = what as usize;
        // The answers kept for the chain as it stands now. An answer is for an entry and
        // the entry below it; those still on the chain are a prefix of the answers, since
        // an entry that changed has other nodes below it.
        let mut known = {
            let path = self.path.borrow();
            let mut answers = self.answers.borrow_mut();
            if answers.len() <= key {
                answers.resize(key + 1, Vec::new());
            }
            let kept = &mut answers[key];
            let (mut low, mut high) = (0, kept.len().min(path.len().saturating_sub(1)));
            count(usize::BITS as usize - high.leading_zeros() as usize);
            while low < high {
                let mid = (low + high) / 2;
                if kept[mid].0 == path[mid + 1].id() {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            kept.truncate(low);
            kept.len()
        };
        while known <= target {
            let (ancestor, child) = {
                let path = self.path.borrow();
                match (path.get(known), path.get(known + 1)) {
                    (Some(a), Some(c)) => (*a, *c),
                    _ => return self.climb(node, &holds),
                }
            };
            count(1);
            let yes = holds(ancestor, child);
            // `holds` may have asked about another node and moved the chain.
            let path = self.path.borrow();
            let mut answers = self.answers.borrow_mut();
            let kept = &mut answers[key];
            let unmoved = kept.len() == known
                && path.get(known).is_some_and(|a| a.id() == ancestor.id())
                && path.get(known + 1).is_some_and(|c| c.id() == child.id())
                && path.get(at).is_some_and(|n| n.id() == node.id());
            if !unmoved {
                drop(answers);
                drop(path);
                return self.climb(node, &holds);
            }
            let above = known.checked_sub(1).map_or(0, |i| kept[i].1);
            kept.push((child.id(), if yes { known as u32 + 1 } else { above }));
            known += 1;
        }
        let path = self.path.borrow();
        let answers = self.answers.borrow();
        let found = answers[key][target].1.checked_sub(1)? as usize;
        Some((path[found], path[found + 1]))
    }

    /// [`Self::nearest`] by a climb that keeps nothing.
    fn climb(
        &self,
        node: Node<'t>,
        holds: &impl Fn(Node<'t>, Node<'t>) -> bool,
    ) -> Option<(Node<'t>, Node<'t>)> {
        let mut child = node;
        while let Some(ancestor) = self.parent(child) {
            if holds(ancestor, child) {
                return Some((ancestor, child));
            }
            child = ancestor;
        }
        None
    }

    /// Where `node` stands on the chain, once the chain holds it. `None` for a node no
    /// descent from the root reaches.
    fn place_on_chain(&self, node: Node<'t>) -> Option<usize> {
        let mut path = self.path.borrow_mut();
        let hint = self.hint.get();
        match path.get(hint) {
            Some(known) if known.id() == node.id() => Some(hint),
            _ => {
                let at = seek(&mut path, node)?;
                self.hint.set(at);
                Some(at)
            }
        }
    }

    /// The ancestors of `node`, the nearest first and the root last.
    pub fn ancestors(&self, node: Node<'t>) -> Ancestors<'_, 't> {
        Ancestors {
            ancestry: self,
            node: Some(node),
        }
    }

    /// The node before `node` among the children of its parent, as
    /// `Node::prev_sibling` answers.
    pub fn prev_sibling(&self, node: Node<'t>) -> Option<Node<'t>> {
        match self.place(node) {
            Some(at) => at.checked_sub(1).map(|i| self.family.borrow().1[i]),
            None => unplaced(node, Sibling::Prev),
        }
    }

    /// The node after `node` among the children of its parent, as `Node::next_sibling`
    /// answers: the first that ends past the end of `node`, so not a node of no width
    /// that stands where `node` ends (one the parser put in for a missing token).
    pub fn next_sibling(&self, node: Node<'t>) -> Option<Node<'t>> {
        match self.place(node) {
            Some(at) => {
                let family = self.family.borrow();
                let found = family.1[at + 1..]
                    .iter()
                    .find(|c| c.end_byte() > node.end_byte());
                count(1);
                found.copied()
            }
            None => unplaced(node, Sibling::Next),
        }
    }

    /// The nearest named node before `node` among the children of its parent, as
    /// `Node::prev_named_sibling` answers.
    pub fn prev_named_sibling(&self, node: Node<'t>) -> Option<Node<'t>> {
        match self.place(node) {
            Some(at) => {
                let family = self.family.borrow();
                let found = family.1[..at].iter().rev().find(|c| c.is_named());
                count(1);
                found.copied()
            }
            None => unplaced(node, Sibling::PrevNamed),
        }
    }

    /// The nearest named node after `node` among the children of its parent, as
    /// `Node::next_named_sibling` answers.
    pub fn next_named_sibling(&self, node: Node<'t>) -> Option<Node<'t>> {
        match self.place(node) {
            Some(at) => {
                let family = self.family.borrow();
                let found = family.1[at + 1..]
                    .iter()
                    .find(|c| c.is_named() && c.end_byte() > node.end_byte());
                count(1);
                found.copied()
            }
            None => unplaced(node, Sibling::NextNamed),
        }
    }

    /// Makes `family` the children of the parent of `node` and returns where `node`
    /// stands among them. `None` for the root and for a node that is not one of the
    /// children its parent lists.
    fn place(&self, node: Node<'t>) -> Option<usize> {
        let parent = self.parent(node)?;
        let mut family = self.family.borrow_mut();
        if family.0 != parent.id() || family.1.is_empty() {
            let mut cursor = parent.walk();
            family.1.clear();
            family.1.extend(parent.children(&mut cursor));
            family.0 = parent.id();
            count(family.1.len());
        }
        // The children stand in the order of their bytes.
        let from = family
            .1
            .partition_point(|c| c.start_byte() < node.start_byte());
        count(usize::BITS as usize - family.1.len().leading_zeros() as usize);
        family.1[from..]
            .iter()
            .take_while(|c| c.start_byte() == node.start_byte())
            .position(|c| c.id() == node.id())
            .map(|at| from + at)
    }

    /// A walker that descends from the root says where it stands: `node` is `depth`
    /// levels below the root and a child of the node whose id is `parent`. When the
    /// chain holds that parent at that depth, `node` is put below it, and a question
    /// about `node` or its ancestors is then answered with no descent. When it does not
    /// (a question about another node moved the chain), nothing changes and the next
    /// question finds its own way.
    pub fn stand_at(&self, depth: usize, parent: usize, node: Node<'t>) {
        count(1);
        let mut path = self.path.borrow_mut();
        let Some(above) = depth.checked_sub(1) else {
            return;
        };
        if path.get(above).is_some_and(|p| p.id() == parent) {
            path.truncate(depth);
            path.push(node);
            self.hint.set(depth);
        }
    }
}

/// The climb from a node to the root ([`Ancestry::ancestors`]).
pub struct Ancestors<'a, 't> {
    ancestry: &'a Ancestry<'t>,
    node: Option<Node<'t>>,
}

impl<'t> Iterator for Ancestors<'_, 't> {
    type Item = Node<'t>;

    fn next(&mut self) -> Option<Node<'t>> {
        self.node = self.ancestry.parent(self.node?);
        self.node
    }
}

/// Which relative of a node is asked for.
#[derive(Clone, Copy)]
enum Sibling {
    #[cfg(test)]
    Parent,
    Prev,
    Next,
    PrevNamed,
    NextNamed,
}

/// The library's own answer about a sibling of `node`, for a node that is not among the
/// children its parent lists (the root, or a node the library reaches another way). It
/// costs the node's depth, so it is counted as the deepest tree read.
fn unplaced(node: Node<'_>, which: Sibling) -> Option<Node<'_>> {
    count(crate::ast::source_text::TREE_DEPTH_LIMIT);
    library(node, which)
}

/// What the library itself answers. This is the one place under `src/ast/` that calls
/// the methods `clippy.toml` disallows: [`unplaced`] needs the library's answer for a
/// node its own descent does not place, and the tests compare every answer with it.
#[allow(clippy::disallowed_methods)]
fn library(node: Node<'_>, which: Sibling) -> Option<Node<'_>> {
    match which {
        #[cfg(test)]
        Sibling::Parent => node.parent(),
        Sibling::Prev => node.prev_sibling(),
        Sibling::Next => node.next_sibling(),
        Sibling::PrevNamed => node.prev_named_sibling(),
        Sibling::NextNamed => node.next_named_sibling(),
    }
}

fn holds(outer: Node, inner: Node) -> bool {
    outer.start_byte() <= inner.start_byte() && inner.end_byte() <= outer.end_byte()
}

/// Makes `path` hold `node` and returns where. `None` when no descent from the root
/// reaches it: `path` is then the chain to the deepest node that descent reached.
fn seek<'t>(path: &mut Vec<Node<'t>>, node: Node<'t>) -> Option<usize> {
    // The entries nest, so those whose bytes hold the node's are a prefix of the chain.
    let holding = path.partition_point(|p| holds(*p, node));
    count(usize::BITS as usize - path.len().leading_zeros() as usize);
    // The node is on the chain where the entries have exactly its bytes, if at all.
    let mut at = holding;
    while at > 0 && path[at - 1].byte_range() == node.byte_range() {
        at -= 1;
        count(1);
        if path[at].id() == node.id() {
            return Some(at);
        }
    }
    // Otherwise it is below the deepest entry that holds it, or reached another way.
    if holding > 1 {
        path.truncate(holding);
        if let Some(found) = descend(path, node) {
            return Some(found);
        }
    }
    path.truncate(1);
    if path[0].id() == node.id() {
        return Some(0);
    }
    descend(path, node)
}

/// Extends `path` from its last entry down to `node`, as `Node::parent` descends, and
/// returns where `node` stands in it; `None` when the descent ends before `node`.
fn descend<'t>(path: &mut Vec<Node<'t>>, node: Node<'t>) -> Option<usize> {
    let mut top = *path.last()?;
    loop {
        count(1);
        let child = top.child_with_descendant(node)?;
        path.push(child);
        if child.id() == node.id() {
            return Some(path.len() - 1);
        }
        top = child;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{default_registry, AssertVocabulary};

    /// `open` written `n` times, `core`, then `close` written `n` times.
    fn nest(n: usize, open: impl Fn(usize) -> String, core: &str, close: &str) -> String {
        let mut out: String = (0..n).map(open).collect();
        out.push_str(core);
        out.push_str(&close.repeat(n));
        out
    }

    /// A name, the path that picks the pack, and the source at a nesting.
    type NestedSource = (&'static str, &'static str, fn(usize) -> String);

    /// The nested sources the scaling tests read: a name, the path that picks the pack,
    /// and the source at nesting `n`. Each is the construct that cost its pack the most
    /// before the walkers stopped asking the library for a parent (#669).
    fn nested_sources() -> Vec<NestedSource> {
        vec![
            ("Rust functions", "tests/t.rs", |n| {
                nest(n, |i| format!("fn f{i}() {{ "), "", "}\n")
            }),
            ("Rust test functions", "tests/t.rs", |n| {
                nest(
                    n,
                    |i| format!("#[test] fn f{i}() {{ assert!(true); "),
                    "",
                    "}\n",
                )
            }),
            ("Rust modules of tests", "src/lib.rs", |n| {
                nest(
                    n,
                    |i| {
                        format!(
                            "#[cfg(test)] mod m{i} {{ #[test] fn t{i}() {{ assert_eq!(1, 1); }} "
                        )
                    },
                    "",
                    "}\n",
                )
            }),
            ("Rust chained calls", "tests/t.rs", |n| {
                format!("#[test] fn t() {{ x{}; }}\n", ".a()".repeat(n))
            }),
            ("JavaScript functions", "src/m.test.js", |n| {
                nest(n, |i| format!("function f{i}() {{ "), "", "}\n")
            }),
            ("JavaScript suites", "src/m.test.js", |n| {
                nest(
                    n,
                    |i| {
                        format!("describe('d{i}', () => {{ it('t{i}', () => {{ expect(1).toBe(1); }}); ")
                    },
                    "",
                    "});\n",
                )
            }),
            ("TypeScript arrow functions", "src/m.test.ts", |n| {
                nest(n, |i| format!("const f{i} = () => {{ "), "", "};\n")
            }),
            ("Java chained calls", "src/test/java/MTest.java", |n| {
                format!(
                    "class MTest {{ @Test void t() {{ x{}; }} }}\n",
                    ".a()".repeat(n)
                )
            }),
            ("Java classes", "src/test/java/MTest.java", |n| {
                nest(
                    n,
                    |i| format!("class C{i} {{ @Test void t{i}() {{ assertEquals(1, 1); }} "),
                    "",
                    "}\n",
                )
            }),
            ("Go nested calls", "m_test.go", |n| {
                format!(
                    "package p\n\nfunc TestT(t *testing.T) {{\nx := {}\n}}\n",
                    nest(n, |_| "g(".to_string(), "1", ")")
                )
            }),
            ("Go subtests", "m_test.go", |n| {
                format!(
                    "package p\n\nfunc TestT(t *testing.T) {{\n{}\n}}\n",
                    nest(
                        n,
                        |i| format!("t.Run(\"s{i}\", func(t *testing.T) {{\n"),
                        "t.Fatal()\n",
                        "})\n"
                    )
                )
            }),
            ("Python chained calls", "tests/test_m.py", |n| {
                format!("def test_t():\n x{}\n", ".a()".repeat(n))
            }),
            ("PHP anonymous classes", "tests/MTest.php", |n| {
                format!(
                    "<?php\nclass MTest extends TestCase {{ public function testT() {{ {} }} }}\n",
                    nest(
                        n,
                        |_| "$o = new class { public function testM() { ".to_string(),
                        "$this->assertSame(1, 1);",
                        "} };\n"
                    )
                )
            }),
            ("C nested calls", "tests/test_m.c", |n| {
                format!(
                    "void test_t(void) {{ int x = {}; }}\n",
                    nest(n, |_| "g(".to_string(), "1", ")")
                )
            }),
            ("C++ classes", "tests/test_m.cpp", |n| {
                nest(
                    n,
                    |i| format!("class C{i} {{ void m{i}() {{ g(); }} "),
                    "",
                    "};\n",
                )
            }),
            ("C# classes", "tests/MTests.cs", |n| {
                nest(
                    n,
                    |i| {
                        format!(
                            "class C{i} {{ [Fact] public void T{i}() {{ Assert.Equal(1, 1); }} "
                        )
                    },
                    "",
                    "}\n",
                )
            }),
            ("Ruby contexts", "spec/m_spec.rb", |n| {
                nest(
                    n,
                    |i| format!("context 'd{i}' do\nit 't{i}' do\nexpect(1).to eq(1)\nend\n"),
                    "",
                    "end\n",
                )
            }),
            ("Ruby suites with no parenthesis", "spec/m_spec.rb", |n| {
                nest(
                    n,
                    |i| format!("describe 'd{i}' do\n"),
                    "it 't' do\nexpect(1).to eq(1)\nend\n",
                    "end\n",
                )
            }),
            ("Kotlin chained calls", "src/test/kotlin/MTest.kt", |n| {
                format!(
                    "class MTest {{ @Test fun t() {{ x{} }} }}\n",
                    ".a()".repeat(n)
                )
            }),
            ("Kotlin classes", "src/test/kotlin/MTest.kt", |n| {
                nest(
                    n,
                    |i| format!("class C{i} {{ @Test fun t{i}() {{ assertEquals(1, 1) }}\n"),
                    "",
                    "}\n",
                )
            }),
            ("Swift classes", "Tests/MTests.swift", |n| {
                nest(
                    n,
                    |i| {
                        format!("class C{i}: XCTestCase {{ func testT{i}() {{ XCTAssertEqual(1, 1) }}\n")
                    },
                    "",
                    "}\n",
                )
            }),
            ("Scala chained calls", "src/test/scala/MTest.scala", |n| {
                format!(
                    "class MTest extends AnyFunSuite {{ test(\"t\") {{ x{} }} }}\n",
                    ".a()".repeat(n)
                )
            }),
            ("Objective-C nested calls", "Tests/MTests.m", |n| {
                format!(
                    "@implementation MTests\n- (void)testT {{ int x = {}; }}\n@end\n",
                    nest(n, |_| "g(".to_string(), "1", ")")
                )
            }),
        ]
    }

    /// The steps one extraction of `src` under `path` counts.
    fn steps_of(path: &str, src: &str) -> u64 {
        let registry = default_registry();
        let pack = registry.find_pack(path).expect("a pack for the path");
        let (facts, counted) = steps(|| pack.extract(path, src, &AssertVocabulary::default()));
        facts.unwrap_or_else(|e| panic!("{path} is read: {e:#}"));
        counted
    }

    /// A source nested four times as deep costs under five times the steps: the work
    /// the walkers count grows with the size of the source, not with its square. Before
    /// #669 a parent was asked of the library, which descends from the root for each
    /// answer, and the same extractions cost 16 to 64 times as much at four times the
    /// depth.
    ///
    /// What is counted is every question put to an [`Ancestry`], every level it
    /// descends, every node the walks that feed it visit, every byte and node read to
    /// blank the text of strings and comments (`calls::code_text`), and every byte of a
    /// callee and a head the call counter reads for a counted word (`calls::count`).
    /// What a pack's own walker reads again for each nested function (the body of every
    /// function for the calls it makes) is not counted here.
    #[test]
    fn a_nested_source_costs_steps_in_proportion_to_its_size() {
        crate::deep_stack::on_deep_stack(|| {
            for (name, path, source) in nested_sources() {
                let (shallow, deep) = (steps_of(path, &source(40)), steps_of(path, &source(160)));
                assert!(shallow > 0, "{name}: nothing was counted");
                assert!(
                    deep < 5 * shallow,
                    "{name}: {shallow} steps at depth 40, {deep} at depth 160"
                );
            }
        })
        .unwrap();
    }

    /// The fuzzing seeds of nested sources (`fuzz/corpus/language_packs/nested_*` and
    /// `chained_*`), each the construct that cost its pack the most, 300 deep.
    const NESTED_SEEDS: &[&str] = &[
        "nested_rust_functions",
        "nested_rust_test_modules",
        "chained_python_calls",
        "nested_js_functions",
        "nested_ts_suites",
        "chained_java_calls",
        "nested_go_calls",
        "nested_php_anonymous_classes",
        "nested_cpp_classes",
        "nested_csharp_classes",
        "nested_ruby_contexts",
        "nested_kotlin_classes",
        "nested_swift_classes",
        "chained_scala_calls",
        "nested_objc_calls",
    ];

    /// The paths the fuzzing target reads a seed under, by the low five bits of its
    /// first byte (`fuzz/fuzz_targets/language_packs.rs`).
    const SEED_PATHS: &[&str] = &[
        "lib.rs", "m.py", "m.js", "m.ts", "m.tsx", "M.java", "m.go", "m.php", "m.c", "m.h",
        "m.cpp", "m.hpp", "M.cs", "m.rb", "m.kt", "m.swift", "m.scala", "m.m", "m.mm", "m.phpt",
    ];

    /// Each seed of a nested source is read, as the fuzzing target reads it, in at most
    /// [`STEPS_PER_BYTE`] steps for each byte of it.
    #[test]
    fn the_seeds_of_nested_sources_are_read_in_steps_bounded_by_their_size() {
        /// Four times the most any of them takes: 15.4 for each byte of the chained
        /// Python calls, and between 2.5 and 9.7 for the others.
        const STEPS_PER_BYTE: u64 = 64;
        crate::deep_stack::on_deep_stack(|| {
            for seed in NESTED_SEEDS {
                let bytes = std::fs::read(format!(
                    "{}/fuzz/corpus/language_packs/{seed}",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .unwrap_or_else(|e| panic!("{seed}: {e}"));
                let (selector, body) = bytes.split_first().expect("a selector byte");
                let name = SEED_PATHS[(selector & 0x1f) as usize % SEED_PATHS.len()];
                assert_ne!(selector & 0x20, 0, "{seed}: a test path");
                let path = format!("tests/{name}");
                let src = String::from_utf8(body.to_vec()).expect("text");
                let counted = steps_of(&path, &src);
                assert!(counted > 0, "{seed}: nothing was counted");
                assert!(
                    counted <= STEPS_PER_BYTE * src.len() as u64,
                    "{seed}: {counted} steps for {} bytes",
                    src.len()
                );
            }
        })
        .unwrap();
    }

    /// The grammars the packs read with, for the trees the answers are checked on. The
    /// Swift grammar is not among them: it is named in one place, behind the parser that
    /// guards its scanner (`swift.rs`).
    fn grammars() -> Vec<(&'static str, tree_sitter::Language)> {
        vec![
            ("rust", tree_sitter_rust::LANGUAGE.into()),
            ("python", tree_sitter_python::LANGUAGE.into()),
            ("javascript", tree_sitter_javascript::LANGUAGE.into()),
            ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into()),
            ("java", tree_sitter_java::LANGUAGE.into()),
            ("go", tree_sitter_go::LANGUAGE.into()),
            ("php", tree_sitter_php::LANGUAGE_PHP.into()),
            ("cpp", tree_sitter_cpp::LANGUAGE.into()),
            ("csharp", tree_sitter_c_sharp::LANGUAGE.into()),
            ("ruby", tree_sitter_ruby::LANGUAGE.into()),
            ("kotlin", tree_sitter_kotlin_ng::LANGUAGE.into()),
            ("scala", tree_sitter_scala::LANGUAGE.into()),
            ("objc", tree_sitter_objc::LANGUAGE.into()),
        ]
    }

    fn tree_of(language: &tree_sitter::Language, src: &str) -> Option<tree_sitter::Tree> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(language).ok()?;
        crate::ast::source_text::parse(&mut parser, src).ok()
    }

    /// Every node of the tree, in source order.
    fn every_node(root: Node<'_>) -> Vec<Node<'_>> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            out.push(node);
            let mut cursor = node.walk();
            let children: Vec<Node> = node.children(&mut cursor).collect();
            stack.extend(children.into_iter().rev());
        }
        out
    }

    /// The sources the answers are checked on: the nested ones, and code with strings,
    /// comments and errors in it. Each is read with every grammar, most of them as text
    /// the grammar has no rule for, which is where a tree has nodes of no width (a token
    /// the parser put in because it was missing) and nodes repaired around an error.
    fn checked_sources() -> Vec<String> {
        let mut out: Vec<String> = nested_sources()
            .into_iter()
            .map(|(_, _, source)| source(12))
            .collect();
        for written in [
            "fn f() { let s = \"a\"; // c\n g(h(1), [2, 3]); }\n#[cfg(test)]\nmod t { #[test] fn t() {} }\n",
            "class A:\n  def f(self):\n    try:\n      return g(x)[0]\n    except E:\n      pass\n",
            "fn broken( { ) ] let = ; }} ((\n",
            "int f(void) { return g(1, 2) + h(3) }\nint k(void) { if (a) { b(); } else c(); }\n",
            "const f = () => { try { g(`a${h(1)}b`); } catch (e) {} };\nclass C { m() { return 1 } }\n",
            "package p\n\nfunc TestT(t *testing.T) {\n\tif err != nil {\n\t\tt.Fatal(err)\n\t}\n}\n",
            "x = [1, 2,\ny = (3\nz = {4: 5\n",
            ";;;\n",
            "",
        ] {
            out.push(written.to_string());
        }
        out
    }

    /// The library's own answers, which cost the depth of the node each time.
    fn library_answers<'t>(node: Node<'t>) -> [Option<Node<'t>>; 5] {
        [
            Sibling::Parent,
            Sibling::Prev,
            Sibling::Next,
            Sibling::PrevNamed,
            Sibling::NextNamed,
        ]
        .map(|which| library(node, which))
    }

    fn asked<'t>(ancestry: &Ancestry<'t>, node: Node<'t>) -> [Option<Node<'t>>; 5] {
        [
            ancestry.parent(node),
            ancestry.prev_sibling(node),
            ancestry.next_sibling(node),
            ancestry.prev_named_sibling(node),
            ancestry.next_named_sibling(node),
        ]
    }

    /// What is kept for a tree is made once, is kept by its type, and is not what
    /// another tree keeps.
    #[test]
    fn what_is_kept_is_kept_once_by_type_and_by_tree() {
        struct Words(Vec<String>);
        struct Count(usize);
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        let (one, two) = (
            crate::ast::source_text::parse(&mut parser, "a\n").unwrap(),
            crate::ast::source_text::parse(&mut parser, "b\nc\n").unwrap(),
        );
        let (first, second) = (
            Ancestry::new(one.root_node()),
            Ancestry::new(two.root_node()),
        );
        let made = Cell::new(0);
        let words = |anc: &Ancestry, word: &str| {
            anc.kept(|| {
                made.set(made.get() + 1);
                Words(vec![word.to_string()])
            })
        };
        assert_eq!(words(&first, "one").0, ["one"]);
        // Asked again, what was made is handed back and nothing is made.
        assert_eq!(words(&first, "other").0, ["one"]);
        assert_eq!(made.get(), 1);
        // Another type in the same tree, and the same type in another tree.
        assert_eq!(first.kept(|| Count(1)).0, 1);
        assert_eq!(words(&first, "other").0, ["one"]);
        assert_eq!(words(&second, "two").0, ["two"]);
        assert_eq!(second.kept(|| Count(2)).0, 2);
        assert_eq!(first.kept(|| Count(3)).0, 1);
        assert_eq!(made.get(), 2);
    }

    /// A parent and the four siblings are what the library answers, for every node of
    /// every tree, whatever order the nodes are asked about in.
    #[test]
    fn every_answer_is_the_librarys() {
        crate::deep_stack::on_deep_stack(|| {
            let mut asked_about = 0usize;
            for src in checked_sources() {
                for (name, language) in grammars() {
                    let Some(tree) = tree_of(&language, &src) else {
                        continue;
                    };
                    let nodes = every_node(tree.root_node());
                    let backwards: Vec<Node> = nodes.iter().rev().copied().collect();
                    // A fixed scatter: every node once, far from the one before it.
                    let scattered: Vec<Node> = (0..nodes.len())
                        .map(|i| nodes[(i * 7919 + 13) % nodes.len()])
                        .collect();
                    for order in [&nodes, &backwards, &scattered] {
                        let ancestry = Ancestry::new(tree.root_node());
                        for node in order {
                            assert_eq!(
                                asked(&ancestry, *node),
                                library_answers(*node),
                                "{name}: {} at {:?} of {src:?}",
                                node.kind(),
                                node.byte_range(),
                            );
                            asked_about += 1;
                        }
                    }
                }
            }
            assert!(
                asked_about > 100_000,
                "{asked_about} nodes were asked about"
            );
        })
        .unwrap();
    }

    /// A walker that says where it stands gets the same answers as one that does not,
    /// and a wrong claim about where it stands changes none.
    #[test]
    fn a_walker_that_says_where_it_stands_changes_no_answer() {
        let src = "fn f() { g(h(1), [2, 3]); { { k(); } } }\nmod m { fn t() { a.b().c(); } }\n";
        let tree = tree_of(&tree_sitter_rust::LANGUAGE.into(), src).unwrap();
        let root = tree.root_node();
        let ancestry = Ancestry::new(root);
        let mut stack = vec![(root, 0usize, root.id())];
        let mut visited = 0;
        while let Some((node, depth, above)) = stack.pop() {
            ancestry.stand_at(depth, above, node);
            // A claim that does not hold: the node is not a child of itself.
            ancestry.stand_at(depth + 1, node.id().wrapping_add(1), node);
            assert_eq!(
                asked(&ancestry, node),
                library_answers(node),
                "{}",
                node.kind()
            );
            // A question about a node elsewhere moves the chain under the walk.
            if visited % 3 == 0 {
                assert_eq!(ancestry.parent(root), None);
                let far = every_node(root)[visited % 5];
                assert_eq!(asked(&ancestry, far), library_answers(far));
            }
            visited += 1;
            let mut cursor = node.walk();
            let children: Vec<Node> = node.children(&mut cursor).collect();
            stack.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|c| (c, depth + 1, node.id())),
            );
        }
        assert!(visited > 60, "{visited}");
    }

    /// What a climb with `Node::parent` stops at, for [`Ancestry::nearest`].
    fn climbed<'t>(
        node: Node<'t>,
        holds: impl Fn(Node<'t>, Node<'t>) -> bool,
    ) -> Option<(Node<'t>, Node<'t>)> {
        let mut child = node;
        while let Some(above) = library(child, Sibling::Parent) {
            if holds(above, child) {
                return Some((above, child));
            }
            child = above;
        }
        None
    }

    /// The nearest ancestor that holds is the one a climb stops at, with the child the
    /// climb came from, whether the question is about the ancestor alone or about the
    /// ancestor and that child, and in whatever order the nodes are asked about.
    #[test]
    fn the_nearest_ancestor_is_the_one_a_climb_stops_at() {
        crate::deep_stack::on_deep_stack(|| {
            let sources = [
                nest(30, |i| format!("fn f{i}() {{ if a {{ g(); "), "", "} }\n"),
                "fn f() { for x in xs { if a { g(h(1)) } else { k() } } }\nmod m { fn t() { while b { if c { d(); } } } }\n".to_string(),
            ];
            for src in sources {
                let tree = tree_of(&tree_sitter_rust::LANGUAGE.into(), &src).unwrap();
                let nodes = every_node(tree.root_node());
                let backwards: Vec<Node> = nodes.iter().rev().copied().collect();
                let scattered: Vec<Node> = (0..nodes.len())
                    .map(|i| nodes[(i * 7919 + 13) % nodes.len()])
                    .collect();
                let function = |above: Node, _: Node| above.kind() == "function_item";
                // About the ancestor and the child the climb came from: the child is
                // the consequence of the `if`, not its condition.
                let consequence = |above: Node, child: Node| {
                    above.kind() == "if_expression"
                        && above.child_by_field_name("consequence") == Some(child)
                };
                let never = |_: Node, _: Node| false;
                for order in [&nodes, &backwards, &scattered] {
                    let ancestry = Ancestry::new(tree.root_node());
                    for node in order {
                        assert_eq!(
                            ancestry.nearest(*node, Above::Function, function),
                            climbed(*node, function),
                            "{}",
                            node.kind()
                        );
                        assert_eq!(
                            ancestry.nearest(*node, Above::Conditional, consequence),
                            climbed(*node, consequence),
                            "{}",
                            node.kind()
                        );
                        assert_eq!(ancestry.nearest(*node, Above::Loop, never), None);
                    }
                }
            }
        })
        .unwrap();
    }

    /// An ancestor that stays on the chain is asked once, however many nodes below it
    /// are read: the questions put to the ancestors of a nested source grow with its
    /// size.
    #[test]
    fn an_ancestor_is_asked_once_while_it_stays_on_the_chain() {
        let asked_for = |depth: usize| {
            let src = nest(depth, |i| format!("fn f{i}() {{ "), "g();", "}\n");
            let tree = tree_of(&tree_sitter_rust::LANGUAGE.into(), &src).unwrap();
            let ancestry = Ancestry::new(tree.root_node());
            let asked = Cell::new(0usize);
            for node in every_node(tree.root_node()) {
                ancestry.nearest(node, Above::Function, |above, _| {
                    asked.set(asked.get() + 1);
                    above.kind() == "mod_item"
                });
            }
            (asked.get(), every_node(tree.root_node()).len())
        };
        crate::deep_stack::on_deep_stack(|| {
            let (asked, nodes) = asked_for(200);
            assert!(asked <= 2 * nodes, "{asked} questions for {nodes} nodes");
        })
        .unwrap();
    }
}
