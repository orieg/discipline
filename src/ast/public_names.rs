//! Public names (`Fact::PublicNames`): the names a source file exposes to code outside
//! its package or crate, and which of them a change adds, removes or modifies.
//!
//! Plain data for the gates that need it (#1086): a documentation check that the doc a
//! change edits names what changed, and stale-symbol references. Nothing here is a
//! finding.
//!
//! "Public" is a per-language contract, written as a table in `docs/GATES.md` ("Public
//! names"). Narrowing a row turns a passing change into a blocked one for the gates that
//! read this, so a change to a row carries a `discipline replay` comparison, like a
//! default change. A file whose language has no pack supplying the fact, or that does
//! not parse, is "could not check" for the consuming gate, never an empty set.
//!
//! A name is qualified by its enclosing type or module (`Parser::parse`, `Config.load`),
//! and compared base against head by kind and qualified name. It is **modified** when
//! the text of its declaration changed (whitespace runs folded); for a type, that is
//! any change inside the type, its members' bodies included. Overloads share one entry.

use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

/// What kind of declaration a public name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NameKind {
    Function,
    Method,
    Type,
    Const,
    Module,
    Macro,
}

impl NameKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NameKind::Function => "fn",
            NameKind::Method => "method",
            NameKind::Type => "type",
            NameKind::Const => "const",
            NameKind::Module => "module",
            NameKind::Macro => "macro",
        }
    }
}

/// One public declaration of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicName {
    /// Qualified by the enclosing type or module.
    pub name: String,
    pub kind: NameKind,
    /// 1-based line of the declaration.
    pub line: usize,
    /// A digest of the declaration's text, whitespace runs folded.
    pub digest: u64,
}

/// A public name a change touched, located on the side that still has it (head for an
/// added or modified name, base for a removed one).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangedName {
    pub name: String,
    pub kind: NameKind,
    pub path: String,
    pub line: usize,
}

/// What a change does to the public names of the files it touches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedNames {
    pub added: Vec<ChangedName>,
    pub removed: Vec<ChangedName>,
    pub modified: Vec<ChangedName>,
    /// Changed files whose public names could not be read, with why: no language pack
    /// supplies the fact, or a side does not parse. A consuming gate reports these as
    /// "could not check", never as "no public name changed".
    pub not_checked: Vec<(String, String)>,
}

impl ChangedNames {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }
}

fn fold(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn digest(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    fold(text).hash(&mut h);
    h.finish()
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

fn name_of(node: Node, src: &str, field: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|n| text(n, src).to_string())
        .filter(|s| !s.is_empty())
}

fn push(out: &mut Vec<PublicName>, name: String, kind: NameKind, node: Node, src: &str) {
    out.push(PublicName {
        name,
        kind,
        line: node.start_position().row + 1,
        digest: digest(text(node, src)),
    });
}

fn qualify(prefix: &[String], name: &str, sep: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}{sep}{name}", prefix.join(sep))
    }
}

fn named_children<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

// ---------------------------------------------------------------------------------------
// Rust: `pub` exactly (not `pub(crate)`, `pub(super)`, `pub(in ..)`), inside modules that
// are themselves `pub`. Methods of an inherent `impl` that are `pub`; every item of a
// `pub trait`. `macro_rules!` with `#[macro_export]`.

#[cfg(feature = "lang-rust")]
fn rust_is_pub(node: Node, src: &str) -> bool {
    named_children(node)
        .into_iter()
        .any(|c| c.kind() == "visibility_modifier" && text(c, src).trim() == "pub")
}

#[cfg(feature = "lang-rust")]
fn rust_items(node: Node, src: &str, prefix: &mut Vec<String>, out: &mut Vec<PublicName>) {
    let children = named_children(node);
    for (i, item) in children.iter().enumerate() {
        let item = *item;
        let kind = match item.kind() {
            "function_item" => Some(NameKind::Function),
            "struct_item" | "enum_item" | "union_item" | "type_item" | "trait_item" => {
                Some(NameKind::Type)
            }
            "const_item" | "static_item" => Some(NameKind::Const),
            "mod_item" => Some(NameKind::Module),
            _ => None,
        };
        if let Some(kind) = kind {
            if !rust_is_pub(item, src) {
                continue;
            }
            let Some(name) = name_of(item, src, "name") else {
                continue;
            };
            push(out, qualify(prefix, &name, "::"), kind, item, src);
            match item.kind() {
                "mod_item" => {
                    if let Some(body) = item.child_by_field_name("body") {
                        prefix.push(name);
                        rust_items(body, src, prefix, out);
                        prefix.pop();
                    }
                }
                "trait_item" => {
                    if let Some(body) = item.child_by_field_name("body") {
                        for member in named_children(body) {
                            if matches!(member.kind(), "function_item" | "function_signature_item")
                            {
                                if let Some(m) = name_of(member, src, "name") {
                                    let q = qualify(prefix, &format!("{name}::{m}"), "::");
                                    push(out, q, NameKind::Method, member, src);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        match item.kind() {
            // Inherent impl: its `pub fn`s. A trait impl's methods are the trait's.
            "impl_item" if item.child_by_field_name("trait").is_none() => {
                let Some(ty) = item.child_by_field_name("type") else {
                    continue;
                };
                let ty = text(ty, src);
                let ty = ty.split('<').next().unwrap_or(ty).trim().to_string();
                if let Some(body) = item.child_by_field_name("body") {
                    for member in named_children(body) {
                        if member.kind() == "function_item" && rust_is_pub(member, src) {
                            if let Some(m) = name_of(member, src, "name") {
                                let q = qualify(prefix, &format!("{ty}::{m}"), "::");
                                push(out, q, NameKind::Method, member, src);
                            }
                        }
                    }
                }
            }
            "macro_definition" => {
                let exported = children[..i]
                    .iter()
                    .rev()
                    .take_while(|n| {
                        matches!(
                            n.kind(),
                            "attribute_item" | "line_comment" | "block_comment"
                        )
                    })
                    .any(|n| {
                        n.kind() == "attribute_item" && fold(text(*n, src)).contains("macro_export")
                    });
                if exported {
                    if let Some(name) = name_of(item, src, "name") {
                        // An exported macro lives at the crate root whatever module holds it.
                        push(out, name, NameKind::Macro, item, src);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(feature = "lang-rust")]
pub fn rust(root: Node, src: &str) -> Vec<PublicName> {
    let mut out = Vec::new();
    rust_items(root, src, &mut Vec::new(), &mut out);
    out
}

// ---------------------------------------------------------------------------------------
// Python: module-level functions, classes and assigned names whose name does not start
// with `_`, or, when the module assigns `__all__` a list or tuple of strings, exactly the
// names it lists. Methods of a public class whose name does not start with `_`. A module
// whose file name starts with `_` (other than `__init__.py`) exposes nothing.

#[cfg(feature = "lang-python")]
fn python_definition(node: Node) -> Node {
    if node.kind() == "decorated_definition" {
        node.child_by_field_name("definition").unwrap_or(node)
    } else {
        node
    }
}

#[cfg(feature = "lang-python")]
fn python_all(root: Node, src: &str) -> Option<BTreeSet<String>> {
    for stmt in named_children(root) {
        let Some(assign) = (stmt.kind() == "expression_statement")
            .then(|| stmt.named_child(0))
            .flatten()
            .filter(|n| n.kind() == "assignment")
        else {
            continue;
        };
        let (Some(left), Some(right)) = (
            assign.child_by_field_name("left"),
            assign.child_by_field_name("right"),
        ) else {
            continue;
        };
        if text(left, src) != "__all__" || !matches!(right.kind(), "list" | "tuple") {
            continue;
        }
        let mut names = BTreeSet::new();
        for item in named_children(right) {
            if item.kind() != "string" {
                continue;
            }
            let t = text(item, src);
            names.insert(t.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
        return Some(names);
    }
    None
}

#[cfg(feature = "lang-python")]
pub fn python(path: &str, root: Node, src: &str) -> Vec<PublicName> {
    let mut out = Vec::new();
    let file = path.rsplit('/').next().unwrap_or(path);
    if file.starts_with('_') && file != "__init__.py" {
        return out;
    }
    let all = python_all(root, src);
    let public = |name: &str| match &all {
        Some(listed) => listed.contains(name),
        None => !name.starts_with('_'),
    };
    for stmt in named_children(root) {
        let def = python_definition(stmt);
        match def.kind() {
            "function_definition" => {
                if let Some(name) = name_of(def, src, "name").filter(|n| public(n)) {
                    push(&mut out, name, NameKind::Function, stmt, src);
                }
            }
            "class_definition" => {
                let Some(name) = name_of(def, src, "name").filter(|n| public(n)) else {
                    continue;
                };
                push(&mut out, name.clone(), NameKind::Type, stmt, src);
                if let Some(body) = def.child_by_field_name("body") {
                    for member in named_children(body) {
                        let m_def = python_definition(member);
                        if m_def.kind() != "function_definition" {
                            continue;
                        }
                        if let Some(m) = name_of(m_def, src, "name").filter(|m| !m.starts_with('_'))
                        {
                            push(
                                &mut out,
                                format!("{name}.{m}"),
                                NameKind::Method,
                                member,
                                src,
                            );
                        }
                    }
                }
            }
            "expression_statement" => {
                let Some(assign) = def.named_child(0).filter(|n| n.kind() == "assignment") else {
                    continue;
                };
                let Some(left) = assign.child_by_field_name("left") else {
                    continue;
                };
                if left.kind() == "identifier" {
                    let name = text(left, src);
                    if name != "__all__" && public(name) {
                        push(&mut out, name.to_string(), NameKind::Const, stmt, src);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Go: top-level functions, types, constants and variables whose name starts with an
// upper-case letter; methods with such a name on such a receiver type. A package under
// an `internal/` directory exposes nothing outside its parent.

#[cfg(feature = "lang-go")]
fn go_exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

/// The first node of `kind` at or under `node`, in document order.
#[cfg(feature = "lang-go")]
fn first_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    if node.kind() == kind {
        return Some(node);
    }
    named_children(node)
        .into_iter()
        .find_map(|c| first_of_kind(c, kind))
}

#[cfg(feature = "lang-go")]
pub fn go(path: &str, root: Node, src: &str) -> Vec<PublicName> {
    let mut out = Vec::new();
    if path.split('/').any(|seg| seg == "internal") {
        return out;
    }
    for decl in named_children(root) {
        match decl.kind() {
            "function_declaration" => {
                if let Some(name) = name_of(decl, src, "name").filter(|n| go_exported(n)) {
                    push(&mut out, name, NameKind::Function, decl, src);
                }
            }
            "method_declaration" => {
                let Some(name) = name_of(decl, src, "name").filter(|n| go_exported(n)) else {
                    continue;
                };
                // `(p *Parser)`, `(Parser)`, `(c *Cache[K, V])`: the first type name in
                // the receiver's type, past the pointer and the type arguments.
                let ty = decl
                    .child_by_field_name("receiver")
                    .and_then(|r| {
                        named_children(r)
                            .into_iter()
                            .find_map(|p| p.child_by_field_name("type"))
                    })
                    .and_then(|t| first_of_kind(t, "type_identifier"))
                    .map(|t| text(t, src))
                    .unwrap_or("");
                if go_exported(ty) {
                    push(
                        &mut out,
                        format!("{ty}.{name}"),
                        NameKind::Method,
                        decl,
                        src,
                    );
                }
            }
            "type_declaration" | "const_declaration" | "var_declaration" => {
                let kind = if decl.kind() == "type_declaration" {
                    NameKind::Type
                } else {
                    NameKind::Const
                };
                let mut specs = Vec::new();
                for child in named_children(decl) {
                    match child.kind() {
                        "type_spec" | "type_alias" | "const_spec" | "var_spec" => specs.push(child),
                        // `var ( ... )` groups its specs one level down.
                        "var_spec_list" | "const_spec_list" => specs.extend(named_children(child)),
                        _ => {}
                    }
                }
                for spec in specs {
                    let mut cursor = spec.walk();
                    for name in spec.children_by_field_name("name", &mut cursor) {
                        let name = text(name, src);
                        if go_exported(name) {
                            push(&mut out, name.to_string(), kind, spec, src);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// JavaScript / TypeScript: what a module exports. `export` before a function, class,
// variable, interface, type alias, enum or namespace; the names of `export { a, b as c }`
// (the exported name) and of `export { x } from "./m"`; `export default` as `default`.
// Methods and fields of an exported class unless `private`, `protected` or `#name`.
// `export * from "./m"` names nothing here.

#[cfg(feature = "lang-javascript")]
fn js_class_members(class: Node, name: &str, src: &str, out: &mut Vec<PublicName>) {
    let Some(body) = class.child_by_field_name("body") else {
        return;
    };
    for member in named_children(body) {
        if !matches!(
            member.kind(),
            "method_definition" | "method_signature" | "abstract_method_signature"
        ) {
            continue;
        }
        let hidden = named_children(member).into_iter().any(|c| {
            c.kind() == "private_property_identifier"
                || (c.kind() == "accessibility_modifier"
                    && matches!(text(c, src), "private" | "protected"))
        });
        if hidden {
            continue;
        }
        if let Some(m) = name_of(member, src, "name").filter(|m| !m.starts_with('#')) {
            push(out, format!("{name}.{m}"), NameKind::Method, member, src);
        }
    }
}

#[cfg(feature = "lang-javascript")]
fn js_declaration(decl: Node, src: &str, out: &mut Vec<PublicName>) {
    match decl.kind() {
        "function_declaration" | "generator_function_declaration" | "function_signature" => {
            if let Some(name) = name_of(decl, src, "name") {
                push(out, name, NameKind::Function, decl, src);
            }
        }
        "class_declaration" | "abstract_class_declaration" | "class" => {
            if let Some(name) = name_of(decl, src, "name") {
                push(out, name.clone(), NameKind::Type, decl, src);
                js_class_members(decl, &name, src, out);
            }
        }
        "interface_declaration" | "type_alias_declaration" | "enum_declaration" => {
            if let Some(name) = name_of(decl, src, "name") {
                push(out, name, NameKind::Type, decl, src);
            }
        }
        "internal_module" | "module" => {
            if let Some(name) = name_of(decl, src, "name") {
                push(out, name, NameKind::Module, decl, src);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            for declarator in named_children(decl) {
                if declarator.kind() != "variable_declarator" {
                    continue;
                }
                if let Some(name) = declarator
                    .child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")
                {
                    push(
                        out,
                        text(name, src).to_string(),
                        NameKind::Const,
                        declarator,
                        src,
                    );
                }
            }
        }
        _ => {}
    }
}

#[cfg(feature = "lang-javascript")]
pub fn javascript(root: Node, src: &str) -> Vec<PublicName> {
    let mut out = Vec::new();
    for stmt in named_children(root) {
        if stmt.kind() != "export_statement" {
            continue;
        }
        let is_default = (0..stmt.child_count())
            .filter_map(|i| stmt.child(i))
            .any(|c| c.kind() == "default");
        // `export default function open() {}` exports `default`; `open` stays local.
        if is_default {
            push(&mut out, "default".into(), NameKind::Const, stmt, src);
            continue;
        }
        if let Some(decl) = stmt.child_by_field_name("declaration") {
            js_declaration(decl, src, &mut out);
            continue;
        }
        for clause in named_children(stmt) {
            if clause.kind() != "export_clause" {
                continue;
            }
            for spec in named_children(clause) {
                if spec.kind() != "export_specifier" {
                    continue;
                }
                let exported = spec
                    .child_by_field_name("alias")
                    .or_else(|| spec.child_by_field_name("name"));
                if let Some(n) = exported {
                    push(
                        &mut out,
                        text(n, src).to_string(),
                        NameKind::Const,
                        spec,
                        src,
                    );
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Java, Kotlin, Scala, C#: types holding members, visibility written as modifiers.

/// What a modifier list says about a declaration's visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vis {
    Public,
    Hidden,
    /// No visibility modifier: the language's default for where it sits.
    Default,
}

/// What a declaration is to the walker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decl {
    /// A type with a member body; `true` when its members are public by default (an
    /// interface).
    Type(bool),
    /// A member or top-level name of this kind.
    Member(NameKind),
    /// Always public where its type is (an enum constant).
    AlwaysPublic(NameKind),
    /// A namespace, package block or companion object: walked as if its members were
    /// written where it is.
    Transparent,
}

/// One language's rules for [`oo_walk`].
struct OoLang {
    decl: fn(Node, &str) -> Option<Decl>,
    vis: fn(Node, &str) -> Vis,
    /// Whether a declaration with no visibility modifier is public at the top level, and
    /// as a member of a type that is not an interface.
    top_default: bool,
    member_default: bool,
    /// The declared names (a field may declare several).
    names: fn(Node, &str) -> Vec<String>,
}

/// The node holding a declaration's members: its `body` field, or a child that is a body.
fn body_of(node: Node) -> Option<Node> {
    node.child_by_field_name("body").or_else(|| {
        named_children(node)
            .into_iter()
            .find(|c| c.kind().ends_with("_body") || c.kind() == "declaration_list")
    })
}

fn oo_walk(
    node: Node,
    src: &str,
    lang: &OoLang,
    prefix: &mut Vec<String>,
    in_interface: bool,
    out: &mut Vec<PublicName>,
) {
    for child in named_children(node) {
        let Some(decl) = (lang.decl)(child, src) else {
            continue;
        };
        let default = if prefix.is_empty() {
            lang.top_default
        } else {
            in_interface || lang.member_default
        };
        let public = match decl {
            Decl::AlwaysPublic(_) => true,
            Decl::Transparent => (lang.vis)(child, src) != Vis::Hidden,
            _ => match (lang.vis)(child, src) {
                Vis::Public => true,
                Vis::Hidden => false,
                Vis::Default => default,
            },
        };
        if !public {
            continue;
        }
        match decl {
            // Its members are in its body, or are its own children (Java's
            // `enum_body_declarations`, Scala's `enum_case_definitions`).
            Decl::Transparent => {
                let body = body_of(child).unwrap_or(child);
                oo_walk(body, src, lang, prefix, in_interface, out);
            }
            Decl::Type(interface) => {
                let Some(name) = (lang.names)(child, src).into_iter().next() else {
                    continue;
                };
                push(out, qualify(prefix, &name, "."), NameKind::Type, child, src);
                if let Some(body) = body_of(child) {
                    prefix.push(name);
                    oo_walk(body, src, lang, prefix, interface, out);
                    prefix.pop();
                }
            }
            Decl::Member(kind) | Decl::AlwaysPublic(kind) => {
                let kind = if kind == NameKind::Function && !prefix.is_empty() {
                    NameKind::Method
                } else {
                    kind
                };
                for name in (lang.names)(child, src) {
                    push(out, qualify(prefix, &name, "."), kind, child, src);
                }
            }
        }
    }
}

/// The words of a node's modifier children (`modifiers`, `modifier`), lower-cased.
fn modifier_words(node: Node, src: &str) -> Vec<String> {
    named_children(node)
        .into_iter()
        .filter(|c| matches!(c.kind(), "modifiers" | "modifier"))
        .flat_map(|c| {
            text(c, src)
                .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
                .filter(|w| !w.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Java and C#: `private` (C# `private protected` too) hides; `public` or `protected`
/// shows; nothing is the default (package-private, `internal`).
fn java_like_vis(node: Node, src: &str) -> Vis {
    let words = modifier_words(node, src);
    let has = |w: &str| words.iter().any(|x| x == w);
    if has("private") {
        Vis::Hidden
    } else if has("public") || has("protected") {
        Vis::Public
    } else {
        Vis::Default
    }
}

/// Kotlin and Scala: `private` and `internal` (Scala `private[x]`) hide; anything else,
/// nothing included, shows.
fn public_by_default_vis(node: Node, src: &str) -> Vis {
    let words = modifier_words(node, src);
    if words.iter().any(|w| w == "private" || w == "internal") {
        Vis::Hidden
    } else {
        Vis::Default
    }
}

fn name_field(node: Node, src: &str) -> Vec<String> {
    name_of(node, src, "name").into_iter().collect()
}

#[cfg(feature = "lang-java")]
fn java_decl(node: Node, _src: &str) -> Option<Decl> {
    Some(match node.kind() {
        "class_declaration" | "enum_declaration" | "record_declaration" => Decl::Type(false),
        "interface_declaration" | "annotation_type_declaration" => Decl::Type(true),
        "method_declaration" | "constructor_declaration" => Decl::Member(NameKind::Method),
        "field_declaration" | "constant_declaration" => Decl::Member(NameKind::Const),
        "enum_constant" => Decl::AlwaysPublic(NameKind::Const),
        "enum_body_declarations" => Decl::Transparent,
        _ => return None,
    })
}

#[cfg(feature = "lang-java")]
fn java_names(node: Node, src: &str) -> Vec<String> {
    if matches!(node.kind(), "field_declaration" | "constant_declaration") {
        let mut cursor = node.walk();
        return node
            .children_by_field_name("declarator", &mut cursor)
            .filter_map(|d| name_of(d, src, "name"))
            .collect();
    }
    name_field(node, src)
}

/// Java: `public` or `protected`; members of an interface unless `private`; enum
/// constants of a public enum. A top-level type with no modifier is package-private.
#[cfg(feature = "lang-java")]
pub fn java(root: Node, src: &str) -> Vec<PublicName> {
    let mut out = Vec::new();
    let lang = OoLang {
        decl: java_decl,
        vis: |n, s| {
            // `enum_body_declarations` has no modifiers: walk it.
            if n.kind() == "enum_body_declarations" {
                Vis::Public
            } else {
                java_like_vis(n, s)
            }
        },
        top_default: false,
        member_default: false,
        names: java_names,
    };
    oo_walk(root, src, &lang, &mut Vec::new(), false, &mut out);
    out
}

#[cfg(feature = "lang-csharp")]
fn csharp_decl(node: Node, _src: &str) -> Option<Decl> {
    Some(match node.kind() {
        "class_declaration" | "struct_declaration" | "record_declaration" | "enum_declaration" => {
            Decl::Type(false)
        }
        "interface_declaration" => Decl::Type(true),
        "delegate_declaration" => Decl::Member(NameKind::Type),
        "method_declaration" | "constructor_declaration" | "operator_declaration" => {
            Decl::Member(NameKind::Method)
        }
        "field_declaration"
        | "property_declaration"
        | "event_field_declaration"
        | "event_declaration" => Decl::Member(NameKind::Const),
        "enum_member_declaration" => Decl::AlwaysPublic(NameKind::Const),
        "namespace_declaration" => Decl::Transparent,
        _ => return None,
    })
}

#[cfg(feature = "lang-csharp")]
fn csharp_names(node: Node, src: &str) -> Vec<String> {
    if matches!(node.kind(), "field_declaration" | "event_field_declaration") {
        return named_children(node)
            .into_iter()
            .filter(|c| c.kind() == "variable_declaration")
            .flat_map(named_children)
            .filter(|d| d.kind() == "variable_declarator")
            .filter_map(|d| name_of(d, src, "name"))
            .collect();
    }
    if node.kind() == "operator_declaration" {
        return node
            .child_by_field_name("operator")
            .map(|o| format!("operator {}", text(o, src)))
            .into_iter()
            .collect();
    }
    name_field(node, src)
}

/// C#: `public`, `protected`, `protected internal`; members of an interface unless
/// `private`; members of a public enum. No modifier is `internal` (a type) or `private`
/// (a member).
#[cfg(feature = "lang-csharp")]
pub fn csharp(root: Node, src: &str) -> Vec<PublicName> {
    let mut out = Vec::new();
    let lang = OoLang {
        decl: csharp_decl,
        vis: java_like_vis,
        top_default: false,
        member_default: false,
        names: csharp_names,
    };
    oo_walk(root, src, &lang, &mut Vec::new(), false, &mut out);
    out
}

#[cfg(feature = "lang-kotlin")]
fn kotlin_decl(node: Node, _src: &str) -> Option<Decl> {
    Some(match node.kind() {
        "class_declaration" | "object_declaration" => Decl::Type(false),
        "function_declaration" => Decl::Member(NameKind::Function),
        "property_declaration" => Decl::Member(NameKind::Const),
        "type_alias" => Decl::Member(NameKind::Type),
        "enum_entry" => Decl::AlwaysPublic(NameKind::Const),
        "companion_object" => Decl::Transparent,
        _ => return None,
    })
}

#[cfg(feature = "lang-kotlin")]
fn kotlin_names(node: Node, src: &str) -> Vec<String> {
    match node.kind() {
        "property_declaration" => named_children(node)
            .into_iter()
            .filter(|c| c.kind() == "variable_declaration")
            .filter_map(|v| {
                named_children(v)
                    .into_iter()
                    .find(|c| c.kind() == "identifier")
                    .map(|i| text(i, src).to_string())
            })
            .collect(),
        "enum_entry" | "type_alias" => name_of(node, src, "type")
            .or_else(|| {
                named_children(node)
                    .into_iter()
                    .find(|c| c.kind() == "identifier")
                    .map(|i| text(i, src).to_string())
            })
            .into_iter()
            .collect(),
        _ => name_field(node, src),
    }
}

/// Kotlin: everything not `private` or `internal` (public is the default). The members
/// of a `companion object` are named on its class.
#[cfg(feature = "lang-kotlin")]
pub fn kotlin(root: Node, src: &str) -> Vec<PublicName> {
    let mut out = Vec::new();
    let lang = OoLang {
        decl: kotlin_decl,
        vis: public_by_default_vis,
        top_default: true,
        member_default: true,
        names: kotlin_names,
    };
    oo_walk(root, src, &lang, &mut Vec::new(), false, &mut out);
    out
}

#[cfg(feature = "lang-scala")]
fn scala_decl(node: Node, _src: &str) -> Option<Decl> {
    Some(match node.kind() {
        "class_definition" | "object_definition" | "trait_definition" | "enum_definition" => {
            Decl::Type(false)
        }
        "function_definition" | "function_declaration" => Decl::Member(NameKind::Function),
        "val_definition" | "var_definition" | "val_declaration" | "var_declaration" => {
            Decl::Member(NameKind::Const)
        }
        "type_definition" => Decl::Member(NameKind::Type),
        "simple_enum_case" | "full_enum_case" => Decl::AlwaysPublic(NameKind::Const),
        "enum_case_definitions" | "package_clause" => Decl::Transparent,
        _ => return None,
    })
}

#[cfg(feature = "lang-scala")]
fn scala_names(node: Node, src: &str) -> Vec<String> {
    match node.kind() {
        "val_definition" | "var_definition" => node
            .child_by_field_name("pattern")
            .filter(|p| p.kind() == "identifier")
            .map(|p| text(p, src).to_string())
            .into_iter()
            .collect(),
        _ => name_field(node, src),
    }
}

/// Scala: everything not `private` (`private[x]` included); public is the default.
#[cfg(feature = "lang-scala")]
pub fn scala(root: Node, src: &str) -> Vec<PublicName> {
    let mut out = Vec::new();
    let lang = OoLang {
        decl: scala_decl,
        vis: public_by_default_vis,
        top_default: true,
        member_default: true,
        names: scala_names,
    };
    oo_walk(root, src, &lang, &mut Vec::new(), false, &mut out);
    out
}

/// Parses `src` with `language` and reads its public names with `read`. A file that does
/// not parse, or parses with errors, is an error: a name inside the broken part could be
/// missed, so the caller names the file as not checked.
pub fn names_of(
    language: &tree_sitter::Language,
    grammar: &str,
    path: &str,
    src: &str,
    read: impl FnOnce(Node, &str) -> Vec<PublicName>,
) -> anyhow::Result<Vec<PublicName>> {
    let tree = super::source_text::parse_file_as(language, grammar, path, src)?;
    let root = tree.root_node();
    if root.has_error() {
        anyhow::bail!("`{path}` parses with errors as {grammar}");
    }
    Ok(read(root, src))
}

// ---------------------------------------------------------------------------------------
// Comparing two sides.

/// The names of one side keyed by kind and qualified name; overloads (the same key)
/// share one entry whose digest covers them all and whose line is the first.
fn by_key(names: &[PublicName]) -> BTreeMap<(NameKind, String), (usize, u64)> {
    let mut grouped: BTreeMap<(NameKind, String), (usize, Vec<u64>)> = BTreeMap::new();
    for n in names {
        let e = grouped
            .entry((n.kind, n.name.clone()))
            .or_insert((n.line, Vec::new()));
        e.0 = e.0.min(n.line);
        e.1.push(n.digest);
    }
    grouped
        .into_iter()
        .map(|(k, (line, mut digests))| {
            digests.sort_unstable();
            let mut h = 0u64;
            for d in digests {
                h = h.rotate_left(7) ^ d;
            }
            (k, (line, h))
        })
        .collect()
}

/// The public names one file's change adds, removes and modifies. `base` and `head` are
/// the file's names on each side (empty for a side the file is absent from).
pub fn compare(path: &str, base: &[PublicName], head: &[PublicName], out: &mut ChangedNames) {
    let (b, h) = (by_key(base), by_key(head));
    let changed = |(kind, name): &(NameKind, String), line: usize| ChangedName {
        name: name.clone(),
        kind: *kind,
        path: path.to_string(),
        line,
    };
    for (key, (line, digest)) in &h {
        match b.get(key) {
            None => out.added.push(changed(key, *line)),
            Some((_, base_digest)) if base_digest != digest => {
                out.modified.push(changed(key, *line))
            }
            Some(_) => {}
        }
    }
    for (key, (line, _)) in &b {
        if !h.contains_key(key) {
            out.removed.push(changed(key, *line));
        }
    }
}

/// One changed file as [`changed_public_names`] reads it: its path on each side (they
/// differ for a rename) and its text there (`None` where it is absent).
pub struct FileChange<'a> {
    pub base_path: &'a str,
    pub head_path: &'a str,
    pub base: Option<&'a str>,
    pub head: Option<&'a str>,
}

/// The public names a change adds, removes and modifies, over every changed file a pack
/// supplying `Fact::PublicNames` reads. Test files are skipped: they are not public API.
/// A file no such pack reads, or a side that does not parse, is in `not_checked`. A
/// renamed file is compared under its head path, so a moved name is neither added nor
/// removed.
pub fn changed_public_names(
    registry: &super::LanguageRegistry,
    files: &[FileChange],
) -> ChangedNames {
    let mut out = ChangedNames::default();
    for f in files {
        let path = if f.head.is_some() {
            f.head_path
        } else {
            f.base_path
        };
        let Some(pack) = registry.find_pack(path) else {
            out.not_checked
                .push((path.to_string(), "no language pack reads this file".into()));
            continue;
        };
        if !pack.supplies(super::Fact::PublicNames) {
            out.not_checked.push((
                path.to_string(),
                format!("the {} pack does not read public names", pack.name()),
            ));
            continue;
        }
        if pack.is_test_path(path) {
            continue;
        }
        let side = |p: &str, text: Option<&str>| -> Result<Vec<PublicName>, String> {
            match text {
                None => Ok(Vec::new()),
                Some(t) => pack.public_names(p, t).map_err(|e| format!("{e:#}")),
            }
        };
        match (side(f.base_path, f.base), side(f.head_path, f.head)) {
            (Ok(b), Ok(h)) => compare(path, &b, &h, &mut out),
            (Err(e), _) | (_, Err(e)) => out.not_checked.push((path.to_string(), e)),
        }
    }
    out.added.sort();
    out.removed.sort();
    out.modified.sort();
    out
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-go"
))]
mod tests {
    use super::*;
    use crate::ast::default_registry;

    fn names(path: &str, src: &str) -> Vec<(String, &'static str)> {
        let reg = default_registry();
        let pack = reg.find_pack(path).expect("a pack");
        assert!(pack.supplies(crate::ast::Fact::PublicNames), "{path}");
        let mut got: Vec<(String, &'static str)> = pack
            .public_names(path, src)
            .expect("parses")
            .into_iter()
            .map(|n| (n.name, n.kind.as_str()))
            .collect();
        got.sort();
        got
    }

    fn want(items: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
        let mut v: Vec<(String, &'static str)> =
            items.iter().map(|(n, k)| (n.to_string(), *k)).collect();
        v.sort();
        v
    }

    #[test]
    fn rust_public_is_pub_exactly_inside_pub_modules() {
        let src = r#"
pub fn open() {}
pub(crate) fn internal() {}
fn private() {}
pub struct Parser<T> { field: T }
impl<T> Parser<T> {
    pub fn parse(&self) {}
    fn helper(&self) {}
}
impl Default for Parser<u8> { fn default() -> Self { todo!() } }
pub trait Visit { fn visit(&self); }
pub const LIMIT: usize = 3;
pub(super) static HIDDEN: u8 = 0;
pub mod api { pub fn call() {} fn quiet() {} }
mod inner { pub fn unreachable() {} }
#[macro_export]
macro_rules! shout { () => {} }
macro_rules! whisper { () => {} }
pub enum Mode { A }
"#;
        assert_eq!(
            names("src/lib.rs", src),
            want(&[
                ("open", "fn"),
                ("Parser", "type"),
                ("Parser::parse", "method"),
                ("Visit", "type"),
                ("Visit::visit", "method"),
                ("LIMIT", "const"),
                ("api", "module"),
                ("api::call", "fn"),
                ("shout", "macro"),
                ("Mode", "type"),
            ])
        );
    }

    #[test]
    fn python_public_is_no_leading_underscore_or_all() {
        let src = "\
import os
VERSION = '1'
_cache = {}
def load(path): pass
def _helper(): pass
@decorator
def wrapped(): pass
class Config:
    def get(self): pass
    def _set(self): pass
    def __init__(self): pass
class _Hidden:
    def shown(self): pass
";
        assert_eq!(
            names("pkg/config.py", src),
            want(&[
                ("VERSION", "const"),
                ("load", "fn"),
                ("wrapped", "fn"),
                ("Config", "type"),
                ("Config.get", "method"),
            ])
        );
        let with_all = "__all__ = ['load', \"Config\"]\ndef load(): pass\ndef other(): pass\nclass Config: pass\n";
        assert_eq!(
            names("pkg/m.py", with_all),
            want(&[("load", "fn"), ("Config", "type")])
        );
        assert!(names("pkg/_private.py", "def open(): pass\n").is_empty());
        assert_eq!(
            names("pkg/__init__.py", "def open(): pass\n"),
            want(&[("open", "fn")])
        );
    }

    #[test]
    fn go_public_is_an_upper_case_name_outside_internal() {
        let src = "\
package p

func Open() {}
func close() {}
type Parser struct{}
type state int
func (p *Parser) Parse() {}
func (p *Parser) reset() {}
func (s state) Show() {}
const Limit = 3
const (
	A = 1
	b = 2
)
var Default = Parser{}
";
        assert_eq!(
            names("pkg/p/p.go", src),
            want(&[
                ("Open", "fn"),
                ("Parser", "type"),
                ("Parser.Parse", "method"),
                ("Limit", "const"),
                ("A", "const"),
                ("Default", "const"),
            ])
        );
        assert!(names("pkg/internal/x/x.go", "package x\nfunc Open() {}\n").is_empty());
    }

    #[test]
    fn javascript_public_is_what_the_module_exports() {
        let js = "\
export function open() {}
function hidden() {}
export const LIMIT = 3, other = 4;
const local = 1;
export class Parser {
  parse() {}
  #secret() {}
}
export { local, hidden as shown };
export { x } from './x';
export * from './all';
export default function () {}
";
        assert_eq!(
            names("src/index.js", js),
            want(&[
                ("open", "fn"),
                ("LIMIT", "const"),
                ("other", "const"),
                ("Parser", "type"),
                ("Parser.parse", "method"),
                ("local", "const"),
                ("shown", "const"),
                ("x", "const"),
                ("default", "const"),
            ])
        );
        let ts = "\
export interface Options { a: string }
export type Id = string;
export enum Mode { A }
export class Client {
  public send(): void {}
  private retry(): void {}
  protected log(): void {}
}
interface Local {}
";
        assert_eq!(
            names("src/api.ts", ts),
            want(&[
                ("Options", "type"),
                ("Id", "type"),
                ("Mode", "type"),
                ("Client", "type"),
                ("Client.send", "method"),
            ])
        );
    }

    #[test]
    fn a_change_is_read_as_added_removed_and_modified_names() {
        let reg = default_registry();
        let base =
            "pub fn keep() {}\npub fn edit() -> u8 { 1 }\npub fn gone() {}\nfn private() {}\n";
        let head = "pub fn keep() {}\npub fn edit() -> u8 { 2 }\npub fn fresh() {}\nfn private() { let _ = 1; }\n";
        let got = changed_public_names(
            &reg,
            &[
                FileChange {
                    base_path: "src/lib.rs",
                    head_path: "src/lib.rs",
                    base: Some(base),
                    head: Some(head),
                },
                FileChange {
                    base_path: "README.txt",
                    head_path: "README.txt",
                    base: None,
                    head: Some("x"),
                },
                FileChange {
                    base_path: "tests/t.rs",
                    head_path: "tests/t.rs",
                    base: None,
                    head: Some("pub fn helper() {}\n"),
                },
                FileChange {
                    base_path: "src/broken.rs",
                    head_path: "src/broken.rs",
                    base: None,
                    head: Some("pub fn ( {\n"),
                },
            ],
        );
        let names = |v: &[ChangedName]| v.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&got.added), ["fresh"]);
        assert_eq!(names(&got.removed), ["gone"]);
        assert_eq!(names(&got.modified), ["edit"]);
        assert_eq!(got.added[0].line, 3);
        // A file no pack reads, and one that does not parse, could not be checked; a
        // test file is not public API.
        let unchecked: Vec<&str> = got.not_checked.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(unchecked, ["README.txt", "src/broken.rs"]);
        // A rename moves the names with the file: nothing added or removed.
        let moved = changed_public_names(
            &reg,
            &[FileChange {
                base_path: "src/a.rs",
                head_path: "src/b.rs",
                base: Some(base),
                head: Some(base),
            }],
        );
        assert!(moved.is_empty(), "{moved:?}");
    }
}
