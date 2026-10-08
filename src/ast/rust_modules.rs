//! Which function of the crate a call in a Rust test names, read from the syntax trees
//! of the crate's files: the `mod` declarations that place each file in the module tree,
//! and the `use` items or the path the call is written with.
//!
//! `assertion-reduction` uses it for a test helper that lives in another file of the
//! test's crate (`#[cfg(test)] mod test_support;` beside the tests, reached through
//! `use super::test_support::*`): the helper stands for the checks it holds only when the
//! call resolves to it here. A name alone is never enough: two functions of one name in
//! two modules are two functions, and a call reaches the one its module imports.
//!
//! What is followed:
//!
//! - a path that starts with `crate`, `self`, `super`, a module of the calling module, or
//!   a name the calling module imports;
//! - a bare name: a function of the calling module, a `use` item that names it (under its
//!   own name or `as` another), a `use` item in the test's body, and glob imports, followed
//!   through the modules they name and their own imports and re-exports;
//! - `mod name;` to `name.rs` or `name/mod.rs` beside the declaring file, and inline
//!   `mod name { .. }` bodies;
//! - visibility, as far as it decides a glob import: a private item is seen from its own
//!   module and the modules inside it.
//!
//! What is not, each leaving the call unresolved: a module declared with `#[path]`, a
//! module declared twice, two imports that hold the name and do not agree, a name the
//! test binds itself (a `let`, a parameter, a closure parameter, a nested function), an
//! item a macro generates, an associated function or a method, and a file with no tree.
//! An unresolved call stands for nothing.

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use tree_sitter::Node;

/// How far an import is followed through re-exports and glob imports.
const IMPORT_DEPTH: usize = 8;
/// How many directories above a file an inline module of its parent may account for.
const INLINE_PARENT_DEPTH: usize = 3;

/// One module of the crate: a file, and the inline modules inside it down to this one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Module {
    file: String,
    inline: Vec<String>,
}

/// A function a call resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateFn {
    pub file: String,
    pub name: String,
    /// The line of its `function_item`, as a pack records a helper's.
    pub line: usize,
    /// Built for tests only: it, an inline module around it, its file (`#![cfg(test)]`),
    /// or the `mod` declaration of a module around it carries `#[cfg(test)]`.
    pub test_only: bool,
}

/// One call site of a test: the last segment of the path it is written with, and the
/// function of the crate it resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    pub leaf: String,
    pub target: Option<CrateFn>,
}

struct FnItem {
    name: String,
    line: usize,
    public: bool,
    cfg_test: bool,
}

struct ModDecl {
    name: String,
    inline: bool,
    public: bool,
    cfg_test: bool,
    path_attribute: bool,
}

#[derive(Clone)]
struct UseItem {
    /// The path the item names, without the `*` of a glob.
    path: Vec<String>,
    /// The name it binds; `None` for a glob.
    binds: Option<String>,
    public: bool,
}

#[derive(Default)]
struct Scope {
    inline: Vec<String>,
    /// The file or an inline module down to this one is built for tests only.
    cfg_test: bool,
    fns: Vec<FnItem>,
    mods: Vec<ModDecl>,
    uses: Vec<UseItem>,
}

/// What a name or a path resolved to.
enum Lookup<T> {
    /// Nothing in the crate has the name here.
    Nothing,
    Found(Vec<T>),
    /// Something has the name and what it is cannot be read.
    Unknown,
}

/// Gives the text of a file of the tree; `None` when the tree holds no such text file.
type ReadFile<'a> = dyn FnMut(&str) -> Result<Option<String>> + 'a;

/// The module tree of one side of a change, read as it is asked for.
pub struct CrateModules<'a> {
    paths: HashSet<String>,
    read: Box<ReadFile<'a>>,
    files: HashMap<String, Option<Rc<Vec<Scope>>>>,
    texts: HashMap<String, Option<Rc<str>>>,
    trees: HashMap<String, Option<Rc<tree_sitter::Tree>>>,
    parents: HashMap<String, Rc<Vec<(Module, bool)>>>,
    climbing: Vec<String>,
}

impl<'a> CrateModules<'a> {
    /// `paths` are the files of the tree; `read` gives the text of one, `None` when it is
    /// not a text file of the tree.
    pub fn new(
        paths: impl IntoIterator<Item = String>,
        read: impl FnMut(&str) -> Result<Option<String>> + 'a,
    ) -> Self {
        CrateModules {
            paths: paths.into_iter().filter(|p| p.ends_with(".rs")).collect(),
            read: Box::new(read),
            files: HashMap::new(),
            texts: HashMap::new(),
            trees: HashMap::new(),
            parents: HashMap::new(),
            climbing: Vec::new(),
        }
    }

    /// The call sites of the test whose `function_item` starts on `line` of `path`, in
    /// source order: every call written as a path, in the test's code and in the
    /// arguments of the macros it invokes. Empty when the file has no such function.
    pub fn test_calls(&mut self, path: &str, line: usize) -> Result<Vec<CallSite>> {
        let Some(text) = self.text(path)? else {
            return Ok(Vec::new());
        };
        let Some(tree) = self.tree(path)? else {
            return Ok(Vec::new());
        };
        let src = text.as_bytes();
        let Some((test, inline)) = function_on_line(tree.root_node(), src, line) else {
            return Ok(Vec::new());
        };
        let module = Module {
            file: path.to_string(),
            inline,
        };
        let Some(body) = test.child_by_field_name("body") else {
            return Ok(Vec::new());
        };
        let mut read = TestBody::default();
        if let Some(parameters) = test.child_by_field_name("parameters") {
            bound_names(parameters, src, &mut read.bound);
        }
        read.walk(body, src);
        let mut sites = Vec::new();
        for path in read.calls {
            let Some(segments) = path else {
                continue;
            };
            let Some(leaf) = segments.last().cloned() else {
                continue;
            };
            let target = if segments.len() == 1 && read.bound.contains(&leaf) {
                None
            } else {
                self.resolve_call(&module, &read.uses, &segments)?
            };
            sites.push(CallSite { leaf, target });
        }
        Ok(sites)
    }

    /// The functions declared in the modules of `path`, the file's own and its inline
    /// ones, each with whether it is built for tests only. Methods and associated
    /// functions are not listed. Empty when the file is not in the tree or has no tree
    /// without errors.
    pub fn module_fns(&mut self, path: &str) -> Result<Vec<CrateFn>> {
        let Some(scopes) = self.scopes(path)? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for scope in scopes.iter() {
            let module = Module {
                file: path.to_string(),
                inline: scope.inline.clone(),
            };
            let module_test_only = scope.cfg_test || self.test_only(&module, 0)?;
            for f in &scope.fns {
                out.push(CrateFn {
                    file: path.to_string(),
                    name: f.name.clone(),
                    line: f.line,
                    test_only: f.cfg_test || module_test_only,
                });
            }
        }
        Ok(out)
    }

    /// Whether the code on `line` of `path` is built for tests only: the module around
    /// it is, or a function declared on that line carries `#[cfg(test)]`. A method of a
    /// test-only module is test-only by its module. `false` when the file has no tree
    /// without errors.
    pub fn test_only_line(&mut self, path: &str, line: usize) -> Result<bool> {
        let (Some(text), Some(tree)) = (self.text(path)?, self.tree(path)?) else {
            return Ok(false);
        };
        let Some(scopes) = self.scopes(path)? else {
            return Ok(false);
        };
        let src = text.as_bytes();
        let mut inline: Vec<String> = Vec::new();
        let mut body = tree.root_node();
        loop {
            let mut cursor = body.walk();
            let inner = body.children(&mut cursor).find_map(|item| {
                let holds = item.start_position().row < line && line <= item.end_position().row + 1;
                let name = item.child_by_field_name("name")?;
                let inner = item.child_by_field_name("body")?;
                (item.kind() == "mod_item" && holds)
                    .then(|| (text_of(name, src).to_string(), inner))
            });
            match inner {
                Some((name, inner)) => {
                    inline.push(name);
                    body = inner;
                }
                None => break,
            }
        }
        let own = scopes.iter().find(|s| s.inline == inline);
        if own.is_some_and(|s| s.fns.iter().any(|f| f.line == line && f.cfg_test)) {
            return Ok(true);
        }
        let module = Module {
            file: path.to_string(),
            inline,
        };
        self.test_only(&module, 0)
    }

    /// The text of `path` as the tree holds it; `None` when it holds no such text file.
    pub fn file_text(&mut self, path: &str) -> Result<Option<Rc<str>>> {
        self.text(path)
    }

    /// The Rust files of the tree, sorted.
    pub fn files(&self) -> Vec<String> {
        let mut files: Vec<String> = self.paths.iter().cloned().collect();
        files.sort();
        files
    }

    /// The function `segments` names from a test of `module` whose body holds `local`
    /// `use` items.
    fn resolve_call(
        &mut self,
        module: &Module,
        local: &[UseItem],
        segments: &[String],
    ) -> Result<Option<CrateFn>> {
        let found = if let [name] = segments {
            // A `use` item of the body that names it wins; a glob import of the body and
            // the module's own names must agree.
            let named: Vec<&UseItem> = local
                .iter()
                .filter(|u| u.binds.as_deref() == Some(name))
                .collect();
            if named.is_empty() {
                let mut all = vec![self.fn_in(module, module, name, 0)?];
                for glob in local.iter().filter(|u| u.binds.is_none()) {
                    all.push(self.fn_through_glob(module, glob, name, 0)?);
                }
                merge(all)
            } else {
                let mut all = Vec::new();
                for item in named {
                    all.push(self.fn_at(module, &item.path, 0)?);
                }
                merge(all)
            }
        } else {
            // The first segment may be a module a `use` item of the body names.
            let first = &segments[0];
            let named = local.iter().find(|u| u.binds.as_ref() == Some(first));
            match named {
                Some(item) => {
                    let mut path = item.path.clone();
                    path.extend_from_slice(&segments[1..]);
                    self.fn_at(module, &path, 0)?
                }
                None => self.fn_at(module, segments, 0)?,
            }
        };
        let Lookup::Found(targets) = found else {
            return Ok(None);
        };
        let [(owner, name, line, cfg_test)] = &targets[..] else {
            return Ok(None);
        };
        let test_only = *cfg_test || self.test_only(owner, 0)?;
        Ok(Some(CrateFn {
            file: owner.file.clone(),
            name: name.clone(),
            line: *line,
            test_only,
        }))
    }

    /// The function the path `segments`, written in `from`, names.
    fn fn_at(
        &mut self,
        from: &Module,
        segments: &[String],
        depth: usize,
    ) -> Result<Lookup<FnTarget>> {
        let Some((name, modules)) = segments.split_last() else {
            return Ok(Lookup::Unknown);
        };
        if modules.is_empty() {
            return self.fn_in(from, from, name, depth);
        }
        match self.modules_at(from, modules, depth)? {
            Lookup::Found(owners) => {
                let mut all = Vec::new();
                for owner in owners {
                    all.push(self.fn_in(from, &owner, name, depth + 1)?);
                }
                Ok(merge(all))
            }
            Lookup::Nothing => Ok(Lookup::Nothing),
            Lookup::Unknown => Ok(Lookup::Unknown),
        }
    }

    /// The function named `name` among the names of `owner` that `from` can see: its own
    /// function, what a `use` item of it names, then what its glob imports hold.
    fn fn_in(
        &mut self,
        from: &Module,
        owner: &Module,
        name: &str,
        depth: usize,
    ) -> Result<Lookup<FnTarget>> {
        if depth > IMPORT_DEPTH {
            return Ok(Lookup::Unknown);
        }
        let Some(scopes) = self.scopes(&owner.file)? else {
            return Ok(Lookup::Unknown);
        };
        let Some(scope) = scopes.iter().find(|s| s.inline == owner.inline) else {
            return Ok(Lookup::Unknown);
        };
        let inside = self.is_inside(from, owner, 0)?;
        let own: Vec<&FnItem> = scope
            .fns
            .iter()
            .filter(|f| f.name == name && (f.public || inside))
            .collect();
        match own[..] {
            [] => {}
            [f] => {
                return Ok(Lookup::Found(vec![(
                    owner.clone(),
                    f.name.clone(),
                    f.line,
                    f.cfg_test || scope.cfg_test,
                )]))
            }
            _ => return Ok(Lookup::Unknown),
        }
        let visible: Vec<UseItem> = scope
            .uses
            .iter()
            .filter(|u| u.public || inside)
            .cloned()
            .collect();
        let named: Vec<&UseItem> = visible
            .iter()
            .filter(|u| u.binds.as_deref() == Some(name))
            .collect();
        if !named.is_empty() {
            let mut all = Vec::new();
            for item in named {
                all.push(match self.fn_at(owner, &item.path, depth + 1)? {
                    // The name is bound to something that is not a function of the crate.
                    Lookup::Nothing => Lookup::Unknown,
                    other => other,
                });
            }
            return Ok(merge(all));
        }
        let mut all = Vec::new();
        for glob in visible.iter().filter(|u| u.binds.is_none()) {
            all.push(self.fn_through_glob(owner, glob, name, depth)?);
        }
        Ok(merge(all))
    }

    /// The function named `name` that the glob import `glob`, written in `from`, brings.
    fn fn_through_glob(
        &mut self,
        from: &Module,
        glob: &UseItem,
        name: &str,
        depth: usize,
    ) -> Result<Lookup<FnTarget>> {
        match self.modules_at(from, &glob.path, depth)? {
            // A glob import of another crate, or of something that is not a module.
            Lookup::Nothing => Ok(Lookup::Nothing),
            Lookup::Unknown => Ok(Lookup::Unknown),
            Lookup::Found(owners) => {
                let mut all = Vec::new();
                for owner in owners.iter().filter(|o| *o != from) {
                    all.push(self.fn_in(from, owner, name, depth + 1)?);
                }
                Ok(merge(all))
            }
        }
    }

    /// The modules the path `segments`, written in `from`, names. `Nothing` when its
    /// first segment is no module of the crate that `from` names (another crate, a type).
    fn modules_at(
        &mut self,
        from: &Module,
        segments: &[String],
        depth: usize,
    ) -> Result<Lookup<Module>> {
        if depth > IMPORT_DEPTH {
            return Ok(Lookup::Unknown);
        }
        let Some((first, rest)) = segments.split_first() else {
            return Ok(Lookup::Unknown);
        };
        let mut at: Vec<Module> = match first.as_str() {
            "crate" => self.roots(from)?,
            "self" => vec![from.clone()],
            "super" => {
                let parents = self.parents_of(from)?;
                if parents.is_empty() {
                    return Ok(Lookup::Unknown);
                }
                parents
            }
            name => match self.module_in(from, from, name, depth)? {
                Lookup::Found(found) => found,
                other => return Ok(other),
            },
        };
        for segment in rest {
            let mut next = Vec::new();
            for module in &at {
                let step = match segment.as_str() {
                    "super" => self.parents_of(module)?,
                    "self" | "crate" => return Ok(Lookup::Unknown),
                    name => match self.module_in(from, module, name, depth + 1)? {
                        Lookup::Found(found) => found,
                        _ => return Ok(Lookup::Unknown),
                    },
                };
                if step.is_empty() {
                    return Ok(Lookup::Unknown);
                }
                for module in step {
                    if !next.contains(&module) {
                        next.push(module);
                    }
                }
            }
            at = next;
        }
        Ok(Lookup::Found(at))
    }

    /// The module named `name` among the names of `owner` that `from` can see: a module
    /// it declares, what a `use` item of it names, then what its glob imports hold.
    fn module_in(
        &mut self,
        from: &Module,
        owner: &Module,
        name: &str,
        depth: usize,
    ) -> Result<Lookup<Module>> {
        if depth > IMPORT_DEPTH {
            return Ok(Lookup::Unknown);
        }
        let Some(scopes) = self.scopes(&owner.file)? else {
            return Ok(Lookup::Unknown);
        };
        let Some(scope) = scopes.iter().find(|s| s.inline == owner.inline) else {
            return Ok(Lookup::Unknown);
        };
        let inside = self.is_inside(from, owner, 0)?;
        let declared: Vec<&ModDecl> = scope
            .mods
            .iter()
            .filter(|m| m.name == name && (m.public || inside))
            .collect();
        match declared[..] {
            [] => {}
            [decl] => {
                return Ok(match self.declared_module(owner, decl)? {
                    Some(module) => Lookup::Found(vec![module]),
                    None => Lookup::Unknown,
                })
            }
            _ => return Ok(Lookup::Unknown),
        }
        let visible: Vec<UseItem> = scope
            .uses
            .iter()
            .filter(|u| u.public || inside)
            .cloned()
            .collect();
        let named: Vec<&UseItem> = visible
            .iter()
            .filter(|u| u.binds.as_deref() == Some(name))
            .collect();
        if !named.is_empty() {
            let mut all = Vec::new();
            for item in named {
                all.push(self.modules_at(owner, &item.path, depth + 1)?);
            }
            return Ok(merge(all));
        }
        let mut all = Vec::new();
        for glob in visible.iter().filter(|u| u.binds.is_none()) {
            match self.modules_at(owner, &glob.path, depth + 1)? {
                Lookup::Nothing => {}
                Lookup::Unknown => all.push(Lookup::Unknown),
                Lookup::Found(owners) => {
                    for other in owners.iter().filter(|o| *o != owner) {
                        all.push(self.module_in(owner, other, name, depth + 1)?);
                    }
                }
            }
        }
        Ok(merge(all))
    }

    /// The module `decl`, declared in `owner`, is: the inline body, or the one file the
    /// declaration names. `None` for a `#[path]` module, and when no file or two files
    /// could be it.
    fn declared_module(&mut self, owner: &Module, decl: &ModDecl) -> Result<Option<Module>> {
        if decl.path_attribute {
            return Ok(None);
        }
        if decl.inline {
            let mut inline = owner.inline.clone();
            inline.push(decl.name.clone());
            return Ok(Some(Module {
                file: owner.file.clone(),
                inline,
            }));
        }
        let mut dir = self.children_dir(&owner.file)?;
        for inline in &owner.inline {
            dir = join(&dir, inline);
        }
        let candidates = [
            join(&dir, &format!("{}.rs", decl.name)),
            join(&dir, &format!("{}/mod.rs", decl.name)),
        ];
        let existing: Vec<&String> = candidates
            .iter()
            .filter(|p| self.paths.contains(*p))
            .collect();
        Ok(match existing[..] {
            [file] => Some(Module {
                file: file.clone(),
                inline: Vec::new(),
            }),
            _ => None,
        })
    }

    /// The directory the files of the modules `file` declares are in: its own for
    /// `mod.rs` and for a file no other declares (a crate root: `lib.rs`, `main.rs`, a
    /// file of `tests/`), and the directory named after it otherwise.
    fn children_dir(&mut self, file: &str) -> Result<String> {
        let (dir, name) = split_path(file);
        if name == "mod.rs" || self.file_parents(file)?.is_empty() {
            return Ok(dir.to_string());
        }
        Ok(join(dir, name.strip_suffix(".rs").unwrap_or(name)))
    }

    /// The modules that declare `module`: the inline module around it, or every module
    /// of another file holding the `mod` declaration that names its file.
    fn parents_of(&mut self, module: &Module) -> Result<Vec<Module>> {
        if let Some((_, outer)) = module.inline.split_last() {
            return Ok(vec![Module {
                file: module.file.clone(),
                inline: outer.to_vec(),
            }]);
        }
        Ok(self
            .file_parents(&module.file)?
            .iter()
            .map(|(parent, _)| parent.clone())
            .collect())
    }

    /// The modules that declare the file `file` with `mod name;`, each with whether that
    /// declaration carries `#[cfg(test)]`. Empty for a crate root.
    fn file_parents(&mut self, file: &str) -> Result<Rc<Vec<(Module, bool)>>> {
        if let Some(known) = self.parents.get(file) {
            return Ok(known.clone());
        }
        // A file met again while its own parents are looked for declares nothing here.
        if self.climbing.iter().any(|f| f == file) {
            return Ok(Rc::new(Vec::new()));
        }
        self.climbing.push(file.to_string());
        let found = self.find_file_parents(file);
        self.climbing.pop();
        let found = Rc::new(found?);
        self.parents.insert(file.to_string(), found.clone());
        Ok(found)
    }

    fn find_file_parents(&mut self, file: &str) -> Result<Vec<(Module, bool)>> {
        let (dir, name) = split_path(file);
        let (name, mut dir) = if name == "mod.rs" {
            let (above, module) = split_path(dir);
            (module.to_string(), above.to_string())
        } else {
            (
                name.strip_suffix(".rs").unwrap_or(name).to_string(),
                dir.to_string(),
            )
        };
        if name.is_empty() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        let mut inline: Vec<String> = Vec::new();
        for _ in 0..=INLINE_PARENT_DEPTH {
            let mut candidates: Vec<String> = self
                .paths
                .iter()
                .filter(|p| *p != file && split_path(p).0 == dir)
                .cloned()
                .collect();
            if !dir.is_empty() {
                candidates.push(format!("{dir}.rs"));
            }
            candidates.sort();
            for candidate in candidates {
                if !self.paths.contains(&candidate) {
                    continue;
                }
                let Some(text) = self.text(&candidate)? else {
                    continue;
                };
                if !may_declare(&text, &name) {
                    continue;
                }
                let Some(scopes) = self.scopes(&candidate)? else {
                    continue;
                };
                let Some(scope) = scopes.iter().find(|s| s.inline == inline) else {
                    continue;
                };
                let declares: Vec<&ModDecl> = scope
                    .mods
                    .iter()
                    .filter(|m| m.name == name && !m.inline && !m.path_attribute)
                    .collect();
                let [decl] = declares[..] else {
                    continue;
                };
                // The declaration names this file only when the candidate's modules live
                // in this directory.
                if self.children_dir(&candidate)? != dir {
                    continue;
                }
                found.push((
                    Module {
                        file: candidate,
                        inline: inline.clone(),
                    },
                    decl.cfg_test || scope.cfg_test,
                ));
            }
            // One directory up, the directory just left is an inline module's.
            let (above, module) = split_path(&dir);
            if module.is_empty() {
                break;
            }
            inline.insert(0, module.to_string());
            dir = above.to_string();
        }
        Ok(found)
    }

    /// The crate roots `module` is declared under: the files, reached by climbing the
    /// `mod` declarations, that no file declares.
    fn roots(&mut self, module: &Module) -> Result<Vec<Module>> {
        let mut roots = Vec::new();
        let mut seen = vec![module.file.clone()];
        let mut climb = vec![module.file.clone()];
        while let Some(file) = climb.pop() {
            let parents = self.file_parents(&file)?;
            if parents.is_empty() {
                let root = Module {
                    file,
                    inline: Vec::new(),
                };
                if !roots.contains(&root) {
                    roots.push(root);
                }
                continue;
            }
            for (parent, _) in parents.iter() {
                if !seen.contains(&parent.file) {
                    seen.push(parent.file.clone());
                    climb.push(parent.file.clone());
                }
            }
        }
        Ok(roots)
    }

    /// Whether `inner` is `outer` or a module inside it.
    fn is_inside(&mut self, inner: &Module, outer: &Module, depth: usize) -> Result<bool> {
        if inner.file == outer.file {
            return Ok(inner.inline.starts_with(&outer.inline));
        }
        if depth > IMPORT_DEPTH * 2 {
            return Ok(false);
        }
        for (parent, _) in self.file_parents(&inner.file)?.iter() {
            if self.is_inside(parent, outer, depth + 1)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether `module` is built for tests only: it or an inline module around it is
    /// `#[cfg(test)]`, or every declaration of its file is, or is in such a module.
    fn test_only(&mut self, module: &Module, depth: usize) -> Result<bool> {
        if depth > IMPORT_DEPTH * 2 {
            return Ok(false);
        }
        let Some(scopes) = self.scopes(&module.file)? else {
            return Ok(false);
        };
        if scopes
            .iter()
            .any(|s| s.cfg_test && module.inline.starts_with(&s.inline))
        {
            return Ok(true);
        }
        let parents = self.file_parents(&module.file)?;
        if parents.is_empty() {
            return Ok(false);
        }
        for (parent, cfg_test) in parents.iter() {
            if !*cfg_test && !self.test_only(parent, depth + 1)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn text(&mut self, path: &str) -> Result<Option<Rc<str>>> {
        if let Some(known) = self.texts.get(path) {
            return Ok(known.clone());
        }
        let text: Option<Rc<str>> = (self.read)(path)?.map(Rc::from);
        self.texts.insert(path.to_string(), text.clone());
        Ok(text)
    }

    /// The syntax tree of `path`; `None` when the file is not in the tree or gives none.
    fn tree(&mut self, path: &str) -> Result<Option<Rc<tree_sitter::Tree>>> {
        if let Some(known) = self.trees.get(path) {
            return Ok(known.clone());
        }
        let tree = match self.text(path)? {
            Some(text) => parse(path, &text).map(Rc::new),
            None => None,
        };
        self.trees.insert(path.to_string(), tree.clone());
        Ok(tree)
    }

    /// The module scopes of `path`, the file's own first. `None` when the file is not in
    /// the tree or has no tree without errors.
    fn scopes(&mut self, path: &str) -> Result<Option<Rc<Vec<Scope>>>> {
        if let Some(known) = self.files.get(path) {
            return Ok(known.clone());
        }
        let scopes = match (self.text(path)?, self.tree(path)?) {
            (Some(text), Some(tree)) => {
                let root = tree.root_node();
                (!root.has_error()).then(|| {
                    let mut scopes = Vec::new();
                    let gated = holds_inner_cfg_test(root, text.as_bytes());
                    read_scope(root, text.as_bytes(), Vec::new(), gated, &mut scopes);
                    Rc::new(scopes)
                })
            }
            _ => None,
        };
        self.files.insert(path.to_string(), scopes.clone());
        Ok(scopes)
    }
}

/// The module, name, line and own `#[cfg(test)]` of a function.
type FnTarget = (Module, String, usize, bool);

/// One answer from several: `Unknown` when any is, or when two that found something do
/// not agree; what was found otherwise.
fn merge<T: PartialEq>(all: Vec<Lookup<T>>) -> Lookup<T> {
    let mut found: Option<Vec<T>> = None;
    for one in all {
        match one {
            Lookup::Nothing => {}
            Lookup::Unknown => return Lookup::Unknown,
            Lookup::Found(these) => match &found {
                None => found = Some(these),
                Some(first) => {
                    let same =
                        first.len() == these.len() && these.iter().all(|t| first.contains(t));
                    if !same {
                        return Lookup::Unknown;
                    }
                }
            },
        }
    }
    found.map_or(Lookup::Nothing, Lookup::Found)
}

fn parse(path: &str, text: &str) -> Option<tree_sitter::Tree> {
    let tree = super::source_text::parse_file_as(
        &tree_sitter_rust::LANGUAGE.into(),
        "the Rust",
        path,
        text,
    )
    .ok();
    super::source_text::forget_unread_part();
    tree
}

/// Whether `text` holds `mod` followed by `name`: read before a file is parsed to look
/// for the declaration, which the syntax tree then decides.
fn may_declare(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].trim_end();
        before.len() < at && before.ends_with("mod")
    })
}

/// `(directory, file name)` of `path`; the directory of a top-level file is empty.
fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn text_of<'t>(node: Node, src: &'t [u8]) -> &'t str {
    node.utf8_text(src).unwrap_or("")
}

fn is_cfg_test(attribute: &str) -> bool {
    attribute == "#[cfg(test)]"
}

/// `#[path = ".."]`, and any `#[cfg_attr(..)]`, which may hold one.
fn is_path_attribute(attribute: &str) -> bool {
    attribute.starts_with("#[path=") || attribute.starts_with("#[cfg_attr(")
}

/// A file or a module body that opens with `#![cfg(test)]`.
fn holds_inner_cfg_test(body: Node, src: &[u8]) -> bool {
    let mut cursor = body.walk();
    let found = body.children(&mut cursor).any(|c| {
        c.kind() == "inner_attribute_item"
            && text_of(c, src).split_whitespace().collect::<String>() == "#![cfg(test)]"
    });
    found
}

/// Reads the items of `body` (a file or the body of an inline module) into a scope, and
/// the bodies of its inline modules into theirs.
fn read_scope(body: Node, src: &[u8], inline: Vec<String>, cfg_test: bool, out: &mut Vec<Scope>) {
    let at = out.len();
    out.push(Scope {
        inline: inline.clone(),
        cfg_test,
        ..Default::default()
    });
    // The attributes written above the item being read, without their white space.
    let mut attributes: Vec<String> = Vec::new();
    let mut cursor = body.walk();
    for item in body.children(&mut cursor) {
        match item.kind() {
            "attribute_item" => {
                attributes.push(text_of(item, src).split_whitespace().collect());
                continue;
            }
            "line_comment" | "block_comment" => continue,
            _ => {}
        }
        let above = std::mem::take(&mut attributes);
        let gated = above.iter().any(|a| is_cfg_test(a));
        let public = {
            let mut c = item.walk();
            let public = item
                .children(&mut c)
                .any(|n| n.kind() == "visibility_modifier");
            public
        };
        match item.kind() {
            "function_item" => {
                if let Some(name) = item.child_by_field_name("name") {
                    out[at].fns.push(FnItem {
                        name: text_of(name, src).to_string(),
                        line: item.start_position().row + 1,
                        public,
                        cfg_test: gated,
                    });
                }
            }
            "mod_item" => {
                let Some(name) = item.child_by_field_name("name") else {
                    continue;
                };
                let name = text_of(name, src).to_string();
                let inner = item.child_by_field_name("body");
                out[at].mods.push(ModDecl {
                    name: name.clone(),
                    inline: inner.is_some(),
                    public,
                    cfg_test: gated,
                    path_attribute: above.iter().any(|a| is_path_attribute(a)),
                });
                if let Some(inner) = inner {
                    let mut path = inline.clone();
                    path.push(name);
                    let gated = cfg_test || gated || holds_inner_cfg_test(inner, src);
                    read_scope(inner, src, path, gated, out);
                }
            }
            "use_declaration" => {
                if let Some(argument) = item.child_by_field_name("argument") {
                    let mut uses = Vec::new();
                    use_items(argument, src, &[], &mut uses);
                    for mut one in uses {
                        one.public = public;
                        out[at].uses.push(one);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The `use` items the clause `node` spells, each path prefixed with `prefix`. A clause
/// that cannot be read binds nothing here: a call through it stays unresolved.
fn use_items(node: Node, src: &[u8], prefix: &[String], out: &mut Vec<UseItem>) {
    let named = |path: Vec<String>, binds: Option<String>| UseItem {
        path,
        binds,
        public: false,
    };
    let with_prefix = |segments: Vec<String>| {
        let mut path = prefix.to_vec();
        path.extend(segments);
        path
    };
    match node.kind() {
        "identifier" | "scoped_identifier" | "crate" | "self" | "super" => {
            let Some(segments) = path_segments(node, src) else {
                return;
            };
            // `use a::{self}` binds `a`.
            let path = if node.kind() == "self" && !prefix.is_empty() {
                prefix.to_vec()
            } else {
                with_prefix(segments)
            };
            let binds = path.last().cloned();
            out.push(named(path, binds));
        }
        "use_as_clause" => {
            let path = node
                .child_by_field_name("path")
                .and_then(|p| path_segments(p, src));
            let alias = node.child_by_field_name("alias").map(|a| text_of(a, src));
            if let (Some(segments), Some(alias)) = (path, alias) {
                let path = if segments == ["self"] && !prefix.is_empty() {
                    prefix.to_vec()
                } else {
                    with_prefix(segments)
                };
                if alias != "_" {
                    out.push(named(path, Some(alias.to_string())));
                }
            }
        }
        "use_wildcard" => {
            let mut cursor = node.walk();
            let inner = node.named_children(&mut cursor).next();
            let segments = match inner {
                Some(path) => match path_segments(path, src) {
                    Some(segments) => segments,
                    None => return,
                },
                None => Vec::new(),
            };
            let path = with_prefix(segments);
            if !path.is_empty() {
                out.push(named(path, None));
            }
        }
        "scoped_use_list" => {
            let path = match node.child_by_field_name("path") {
                Some(p) => match path_segments(p, src) {
                    Some(segments) => with_prefix(segments),
                    None => return,
                },
                None => prefix.to_vec(),
            };
            if let Some(list) = node.child_by_field_name("list") {
                use_items(list, src, &path, out);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                use_items(child, src, prefix, out);
            }
        }
        _ => {}
    }
}

/// The segments of a path written with names only (`a::b`, `crate::a`, `f::<T>`).
/// `None` for any other callee: a method, a path through a type, a closure.
fn path_segments(node: Node, src: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "identifier" | "crate" | "self" | "super" => Some(vec![text_of(node, src).to_string()]),
        "scoped_identifier" => {
            let mut segments = path_segments(node.child_by_field_name("path")?, src)?;
            let name = node.child_by_field_name("name")?;
            if !matches!(name.kind(), "identifier" | "super") {
                return None;
            }
            segments.push(text_of(name, src).to_string());
            Some(segments)
        }
        "generic_function" => path_segments(node.child_by_field_name("function")?, src),
        _ => None,
    }
}

/// The `function_item` that starts on `line`, with the names of the inline modules
/// around it, the outermost first; `None` when two start there.
fn function_on_line<'t>(
    root: Node<'t>,
    src: &[u8],
    line: usize,
) -> Option<(Node<'t>, Vec<String>)> {
    let mut found = Vec::new();
    let mut stack = vec![(root, Vec::new())];
    while let Some((node, modules)) = stack.pop() {
        let (first, last) = (node.start_position().row + 1, node.end_position().row + 1);
        if line < first || line > last {
            continue;
        }
        if node.kind() == "function_item" && first == line {
            found.push((node, modules.clone()));
        }
        let mut inside = modules;
        if node.kind() == "mod_item" {
            if let Some(name) = node.child_by_field_name("name") {
                inside.push(text_of(name, src).to_string());
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, inside.clone()));
        }
    }
    if found.len() == 1 {
        found.pop()
    } else {
        None
    }
}

/// The identifiers a pattern or a parameter list binds.
fn bound_names(node: Node, src: &[u8], out: &mut Vec<String>) {
    if node.kind() == "identifier" {
        out.push(text_of(node, src).to_string());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        // The type of a parameter binds nothing.
        if node.field_name_for_child(child_index(node, child)) == Some("type") {
            continue;
        }
        bound_names(child, src, out);
    }
}

fn child_index(parent: Node, child: Node) -> u32 {
    let mut cursor = parent.walk();
    let at = parent
        .children(&mut cursor)
        .position(|c| c.id() == child.id())
        .unwrap_or(0);
    at as u32
}

/// What a test's body holds for the resolution: its calls, the names it binds, and its
/// own `use` items.
#[derive(Default)]
struct TestBody {
    /// The path of each call, `None` for a callee that is not a path.
    calls: Vec<Option<Vec<String>>>,
    bound: Vec<String>,
    uses: Vec<UseItem>,
}

impl TestBody {
    fn walk(&mut self, node: Node, src: &[u8]) {
        match node.kind() {
            "function_item" => {
                if let Some(name) = node.child_by_field_name("name") {
                    self.bound.push(text_of(name, src).to_string());
                }
                return;
            }
            "use_declaration" => {
                if let Some(argument) = node.child_by_field_name("argument") {
                    use_items(argument, src, &[], &mut self.uses);
                }
                return;
            }
            "call_expression" => {
                if let Some(callee) = node.child_by_field_name("function") {
                    self.calls.push(path_segments(callee, src));
                }
            }
            "macro_invocation" => {
                let evaluates = node.child_by_field_name("macro").is_some_and(|m| {
                    let name = text_of(m, src);
                    let name = name.rsplit("::").next().unwrap_or(name).trim();
                    !super::rust::NON_EVALUATING_MACROS.contains(&name)
                });
                if evaluates {
                    let mut cursor = node.walk();
                    for tree in node
                        .children(&mut cursor)
                        .filter(|c| c.kind() == "token_tree")
                    {
                        self.token_tree(tree, src);
                    }
                }
                return;
            }
            "let_declaration" | "for_expression" | "match_arm" | "let_condition" => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    bound_names(pattern, src, &mut self.bound);
                }
            }
            "closure_expression" => {
                if let Some(parameters) = node.child_by_field_name("parameters") {
                    bound_names(parameters, src, &mut self.bound);
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.walk(child, src);
        }
    }

    /// The calls inside macro arguments, which the grammar keeps as a flat token tree: a
    /// name followed by a parenthesised tree and not after a `.`, with the path segments
    /// written before it. A `let` in a macro argument is not read as a binding.
    fn token_tree(&mut self, tree: Node, src: &[u8]) {
        let mut cursor = tree.walk();
        let tokens: Vec<Node> = tree.children(&mut cursor).collect();
        let mut skip = None;
        for (i, token) in tokens.iter().enumerate() {
            let next = tokens.get(i + 1);
            if token.kind() == "identifier" {
                let name = text_of(*token, src);
                let after_dot = i > 0 && tokens[i - 1].kind() == ".";
                if next.is_some_and(|n| n.kind() == "!") {
                    if super::rust::NON_EVALUATING_MACROS.contains(&name) {
                        skip = Some(i + 2);
                    }
                } else if !after_dot
                    && next.is_some_and(|n| {
                        n.kind() == "token_tree" && n.child(0).is_some_and(|c| c.kind() == "(")
                    })
                {
                    let mut segments = vec![name.to_string()];
                    let mut at = i;
                    let mut whole = true;
                    while at >= 1 && tokens[at - 1].kind() == "::" {
                        let before = (at >= 2).then(|| tokens[at - 2]);
                        match before.map(|b| (b.kind(), b)) {
                            Some(("identifier" | "crate" | "self" | "super", b)) => {
                                segments.insert(0, text_of(b, src).to_string());
                                at -= 2;
                            }
                            _ => {
                                whole = false;
                                break;
                            }
                        }
                    }
                    // A path that starts with `::`, or goes through a type, is not one
                    // of this crate's modules; its last segment still names the call.
                    self.calls.push(if whole {
                        Some(segments)
                    } else {
                        Some(vec![String::new(), name.to_string()])
                    });
                }
            }
            if token.kind() == "token_tree" && skip != Some(i) {
                self.token_tree(*token, src);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// File, line and test-only of the function a call resolved to.
    type Target = Option<(String, usize, bool)>;

    /// The call sites of the test `t` of `path` in a tree of `files`, as `(leaf, file,
    /// line and test-only of the target)`.
    fn calls(files: &[(&str, &str)], path: &str) -> Vec<(String, Target)> {
        let text = files.iter().find(|(p, _)| *p == path).unwrap().1;
        let line = 1 + text.lines().position(|l| l.contains("fn t()")).unwrap();
        let paths: Vec<String> = files.iter().map(|(p, _)| p.to_string()).collect();
        let mut modules = CrateModules::new(paths, |wanted: &str| {
            Ok(files
                .iter()
                .find(|(p, _)| *p == wanted)
                .map(|(_, text)| text.to_string()))
        });
        modules
            .test_calls(path, line)
            .unwrap()
            .into_iter()
            .map(|site| {
                (
                    site.leaf,
                    site.target.map(|t| (t.file, t.line, t.test_only)),
                )
            })
            .collect()
    }

    fn found(file: &str, line: usize) -> Target {
        Some((file.to_string(), line, true))
    }

    const SUPPORT: &str = "pub(super) fn check(x: u32) {\n    assert_eq!(x, 1);\n}\n";

    /// A glob import, a named import, an alias and a path each reach the function of the
    /// module they name; a call the module does not import reaches nothing.
    #[test]
    fn a_call_resolves_through_the_import_or_the_path_it_is_written_with() {
        let lib = |imports: &str, call: &str| {
            format!("#[cfg(test)]\nmod support;\n\n#[cfg(test)]\nmod tests {{\n    use super::*;\n    {imports}\n    #[test]\n    fn t() {{\n        {call};\n    }}\n}}\n")
        };
        for (imports, call, leaf) in [
            ("use super::support::*;", "check(1)", "check"),
            ("use super::support::check;", "check(1)", "check"),
            ("use crate::support::*;", "check(1)", "check"),
            (
                "use super::support::check as verify;",
                "verify(1)",
                "verify",
            ),
            ("use super::support::{self, check};", "check(1)", "check"),
            ("use super::support::{self};", "support::check(1)", "check"),
            ("", "support::check(1)", "check"),
            ("", "super::support::check(1)", "check"),
            ("", "crate::support::check(1)", "check"),
            ("", "self::super::support::check(1)", "check"),
            ("", "assert!(support::check(1) == ())", "check"),
            ("", "support::check::<u32>(1)", "check"),
        ] {
            let lib = lib(imports, call);
            let files = [("src/lib.rs", lib.as_str()), ("src/support.rs", SUPPORT)];
            assert_eq!(
                calls(&files, "src/lib.rs"),
                vec![(leaf.to_string(), found("src/support.rs", 1))],
                "{imports} {call}"
            );
        }
        let lib = lib("", "check(1)");
        let files = [("src/lib.rs", lib.as_str()), ("src/support.rs", SUPPORT)];
        assert_eq!(
            calls(&files, "src/lib.rs"),
            vec![("check".to_string(), None)]
        );
    }

    /// Two functions of one name are told apart by the module the call goes through, and
    /// two glob imports that both hold the name resolve to neither.
    #[test]
    fn two_functions_of_one_name_are_two_functions() {
        let lib = |imports: &str| {
            format!("#[cfg(test)]\nmod a;\n#[cfg(test)]\nmod b;\n\n#[cfg(test)]\nmod tests {{\n    {imports}\n    #[test]\n    fn t() {{\n        check(1);\n    }}\n}}\n")
        };
        let other = format!("// another\n{SUPPORT}");
        let tree = |lib: &str| {
            calls(
                &[
                    ("src/lib.rs", lib),
                    ("src/a.rs", SUPPORT),
                    ("src/b.rs", &other),
                ],
                "src/lib.rs",
            )
        };
        assert_eq!(
            tree(&lib("use super::a::*;")),
            vec![("check".to_string(), found("src/a.rs", 1))]
        );
        assert_eq!(
            tree(&lib("use super::b::*;")),
            vec![("check".to_string(), found("src/b.rs", 2))]
        );
        assert_eq!(
            tree(&lib("use super::a::*; use super::b::*;")),
            vec![("check".to_string(), None)]
        );
        // A named import wins over a glob import.
        assert_eq!(
            tree(&lib("use super::a::*; use super::b::check;")),
            vec![("check".to_string(), found("src/b.rs", 2))]
        );
    }

    /// A module file is found from its declaration: beside a crate root or a `mod.rs`,
    /// under the directory named after any other file, and under an inline module's.
    #[test]
    fn a_module_file_is_where_its_declaration_puts_it() {
        let test = "#[cfg(test)]\nmod support;\n#[cfg(test)]\nmod tests {\n    use super::support::*;\n    #[test]\n    fn t() {\n        check(1);\n    }\n}\n";
        let one = vec![("check".to_string(), found("src/a/support.rs", 1))];
        // `src/a.rs` declares `src/a/support.rs`, not `src/support.rs`.
        let files = [
            ("src/lib.rs", "mod a;\n"),
            ("src/a.rs", test),
            ("src/a/support.rs", SUPPORT),
            ("src/support.rs", "pub fn check(x: u32) {}\n"),
        ];
        assert_eq!(calls(&files, "src/a.rs"), one);
        let files = [
            ("src/main.rs", "mod a;\n"),
            ("src/a/mod.rs", test),
            ("src/a/support.rs", SUPPORT),
        ];
        assert_eq!(calls(&files, "src/a/mod.rs"), one);
        // The crate root is found by climbing the declarations.
        let nested = test.replace("super::support", "crate::a::support");
        let files = [
            ("src/lib.rs", "mod a;\n"),
            ("src/a.rs", nested.as_str()),
            ("src/a/support.rs", SUPPORT),
        ];
        assert_eq!(calls(&files, "src/a.rs"), one);
        // An inline module's files are under a directory of its name.
        let inline = "#[cfg(test)]\nmod tests {\n    mod support;\n    use support::*;\n    #[test]\n    fn t() {\n        check(1);\n    }\n}\n";
        let files = [("src/lib.rs", inline), ("src/tests/support.rs", SUPPORT)];
        assert_eq!(
            calls(&files, "src/lib.rs"),
            vec![("check".to_string(), found("src/tests/support.rs", 1))]
        );
        // A file of a test directory is a crate root: its modules are beside it.
        let files = [
            (
                "tests/api.rs",
                "mod common;\n#[test]\nfn t() {\n    common::check(1);\n}\n",
            ),
            ("tests/common/mod.rs", "pub fn check(x: u32) {}\n"),
        ];
        assert_eq!(
            calls(&files, "tests/api.rs"),
            vec![(
                "check".to_string(),
                Some(("tests/common/mod.rs".to_string(), 1, false))
            )]
        );
    }

    /// What is not followed leaves the call unresolved.
    #[test]
    fn a_call_that_cannot_be_followed_is_unresolved() {
        let lib = |declared: &str, body: &str| {
            format!("{declared}\n#[cfg(test)]\nmod tests {{\n    use super::support::*;\n    #[test]\n    fn t() {{\n        {body}\n    }}\n}}\n")
        };
        let unresolved = vec![("check".to_string(), None)];
        let tree = |lib: &str, support: &str| {
            calls(
                &[("src/lib.rs", lib), ("src/support.rs", support)],
                "src/lib.rs",
            )
        };
        let declared = "#[cfg(test)] mod support;";
        assert_eq!(
            tree(&lib(declared, "check(1);"), SUPPORT),
            vec![("check".to_string(), found("src/support.rs", 1))]
        );
        // `#[path]`, a second declaration, no declaration, a private function, a file
        // with a syntax error, and a name the test binds.
        let path = "#[cfg(test)] #[path = \"elsewhere.rs\"] mod support;";
        assert_eq!(tree(&lib(path, "check(1);"), SUPPORT), unresolved);
        let twice = "#[cfg(test)] mod support;\n#[cfg(windows)] mod support;";
        assert_eq!(tree(&lib(twice, "check(1);"), SUPPORT), unresolved);
        assert_eq!(tree(&lib("", "check(1);"), SUPPORT), unresolved);
        let private = SUPPORT.replace("pub(super) ", "");
        assert_eq!(tree(&lib(declared, "check(1);"), &private), unresolved);
        let broken = format!("{SUPPORT}fn (\n");
        assert_eq!(tree(&lib(declared, "check(1);"), &broken), unresolved);
        for body in [
            "let check = |_: u32| (); check(1);",
            "fn check(_: u32) {} check(1);",
            "for check in [f] { check(1); }",
        ] {
            assert_eq!(
                tree(&lib(declared, body), SUPPORT)[0],
                unresolved[0],
                "{body}"
            );
        }
        // A method and an associated function are not calls by a path of modules.
        assert_eq!(
            tree(&lib(declared, "x.check(1); T::check(1);"), SUPPORT),
            unresolved
        );
    }

    /// A function is test-only through its own attribute, an inline module, its file's
    /// inner attribute, or the declaration of a module around it.
    #[test]
    fn test_only_is_read_from_the_cfg_test_attributes_down_to_the_function() {
        let lib = |declared: &str| {
            format!("{declared}\n#[cfg(test)]\nmod tests {{\n    use super::support::*;\n    #[test]\n    fn t() {{\n        check(1);\n    }}\n}}\n")
        };
        let test_only = |declared: &str, support: &str| {
            let lib = lib(declared);
            let files = [("src/lib.rs", lib.as_str()), ("src/support.rs", support)];
            calls(&files, "src/lib.rs")[0].1.clone().map(|t| t.2)
        };
        let public = "pub fn check(x: u32) {}\n";
        assert_eq!(test_only("mod support;", public), Some(false));
        assert_eq!(test_only("#[cfg(test)]\nmod support;", public), Some(true));
        assert_eq!(
            test_only(
                "#[cfg(test)]\n// shared\n#[allow(dead_code)]\nmod support;",
                public
            ),
            Some(true)
        );
        assert_eq!(
            test_only("mod support;", &format!("#![cfg(test)]\n{public}")),
            Some(true)
        );
        assert_eq!(
            test_only("mod support;", &format!("#[cfg(test)]\n{public}")),
            Some(true)
        );
        assert_eq!(
            test_only("#[cfg(feature = \"test\")]\nmod support;", public),
            Some(false)
        );
    }

    /// The functions of a file's modules are listed with the same reading: test-only by
    /// the function's attribute, an inline module, or the declaration of the file's
    /// module. A method is not listed, and a file no module declares is not test-only.
    #[test]
    fn the_functions_of_a_file_are_listed_with_whether_they_are_test_only() {
        let support = "pub fn plain() {}\n\n#[cfg(test)]\npub fn gated() {}\n\n#[cfg(test)]\nmod inner {\n    pub fn inside() {}\n}\n\nstruct S;\n\nimpl S {\n    fn method(&self) {}\n}\n";
        let listed = |declared: &str| {
            let files = [("src/lib.rs", declared), ("src/support.rs", support)];
            let paths: Vec<String> = files.iter().map(|(p, _)| p.to_string()).collect();
            let mut modules = CrateModules::new(paths, |wanted: &str| {
                Ok(files
                    .iter()
                    .find(|(p, _)| *p == wanted)
                    .map(|(_, text)| text.to_string()))
            });
            let fns = modules.module_fns("src/support.rs").unwrap();
            fns.into_iter()
                .map(|f| (f.name, f.line, f.test_only))
                .collect::<Vec<_>>()
        };
        let fns = |plain: bool| {
            vec![
                ("plain".to_string(), 1, plain),
                ("gated".to_string(), 4, true),
                ("inside".to_string(), 8, true),
            ]
        };
        assert_eq!(listed("mod support;\n"), fns(false));
        assert_eq!(listed("#[cfg(test)]\nmod support;\n"), fns(true));
        assert_eq!(listed("pub fn unrelated() {}\n"), fns(false));
    }

    /// A line is test-only by the module around it, a method by its module, and a
    /// function outside such a module by its own attribute.
    #[test]
    fn a_line_is_test_only_by_the_module_around_it() {
        let lib = "pub fn plain() {}\n\n#[cfg(test)]\nfn gated() {}\n\nstruct S;\n\nimpl S {\n    fn outside(&self) {}\n}\n\n#[cfg(test)]\nmod tests {\n    struct T;\n\n    impl T {\n        fn method(&self) {}\n    }\n\n    mod deeper {\n        fn inside() {}\n    }\n}\n";
        let files = [("src/lib.rs", lib)];
        let mut modules = CrateModules::new(vec!["src/lib.rs".to_string()], |wanted: &str| {
            Ok(files
                .iter()
                .find(|(p, _)| *p == wanted)
                .map(|(_, text)| text.to_string()))
        });
        let mut at = |needle: &str| {
            let line = 1 + lib.lines().position(|l| l.contains(needle)).unwrap();
            modules.test_only_line("src/lib.rs", line).unwrap()
        };
        assert!(!at("fn plain"));
        assert!(at("fn gated"));
        assert!(!at("fn outside"));
        assert!(at("fn method"));
        assert!(at("fn inside"));
    }
}
