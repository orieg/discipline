//! The `linguist-vendored` and `linguist-generated` attributes of a path, read from the
//! tracked `.gitattributes` files of one side of a change.
//!
//! The rules are git's own (`gitattributes(5)`): a pattern is a `.gitignore` pattern
//! without negation, relative to the directory of the file that holds it; a pattern
//! with no slash matches the base name at any depth below that directory; a pattern
//! matches the path itself, never the paths inside a directory it names (`vendor/`
//! matches no file; `vendor/**` does); within one file the later line wins, and a file
//! nearer the path wins over one above it. Only tracked `.gitattributes` files are read:
//! `$GIT_DIR/info/attributes`, the user's and the system's files are not part of the
//! change. A `[attr]` macro line is not expanded: an attribute set only through a macro
//! reads as not set, and the file counts.

use std::collections::HashMap;

/// The attributes that take a file out of the count, in the order they are reported.
pub const ATTRIBUTES: &[&str] = &["linguist-vendored", "linguist-generated"];

/// What one line says about one attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// `attr`, or `attr=true`.
    Set,
    /// `-attr`, `!attr`, or `attr=<anything else>`: the line decides, and the attribute
    /// is not set.
    NotSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    /// The pattern, without a leading `/`.
    pattern: String,
    /// No `/` in the pattern: it is matched against the base name.
    basename: bool,
    /// A trailing `/`: the pattern names directories only, so it matches no file.
    directory_only: bool,
    /// The state the line gives each of [`ATTRIBUTES`], when it names it.
    states: [Option<State>; 2],
}

/// Every tracked `.gitattributes` of one side, by the directory that holds it (empty
/// for the repository root).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitAttributes {
    files: HashMap<String, Vec<Rule>>,
}

/// A step budget for one match: a pattern is written by the change under review, and
/// a run of `*` must not be able to stall the check. Running out reads as no match,
/// which leaves the attribute unset and the file counted.
const MATCH_BUDGET: u32 = 200_000;

impl GitAttributes {
    /// Reads the `.gitattributes` at `path` (`.gitattributes`, `vendor/.gitattributes`).
    pub fn add_file(&mut self, path: &str, content: &str) {
        let dir = match path.rfind('/') {
            Some(i) => &path[..i],
            None => "",
        };
        let rules: Vec<Rule> = content.lines().filter_map(parse_line).collect();
        if !rules.is_empty() {
            self.files.insert(dir.to_string(), rules);
        }
    }

    /// The first of [`ATTRIBUTES`] that is set on `path`, with the `.gitattributes`
    /// file whose line sets it.
    pub fn set_on(&self, path: &str) -> Option<(&'static str, String)> {
        if self.files.is_empty() {
            return None;
        }
        let path = path.strip_prefix("./").unwrap_or(path);
        ATTRIBUTES
            .iter()
            .enumerate()
            .find_map(|(index, name)| self.deciding_file(path, index).map(|file| (*name, file)))
    }

    /// The attributes file whose line sets attribute `index` on `path`: the nearest
    /// directory's file is read first, each file from its last line to its first, and
    /// the first line that matches and names the attribute decides.
    fn deciding_file(&self, path: &str, index: usize) -> Option<String> {
        let mut dir = match path.rfind('/') {
            Some(i) => &path[..i],
            None => "",
        };
        loop {
            if let Some(rules) = self.files.get(dir) {
                let relative = if dir.is_empty() {
                    path
                } else {
                    &path[dir.len() + 1..]
                };
                for rule in rules.iter().rev() {
                    let Some(state) = rule.states[index] else {
                        continue;
                    };
                    if rule.matches(relative) {
                        return (state == State::Set).then(|| {
                            if dir.is_empty() {
                                ".gitattributes".to_string()
                            } else {
                                format!("{dir}/.gitattributes")
                            }
                        });
                    }
                }
            }
            if dir.is_empty() {
                return None;
            }
            dir = match dir.rfind('/') {
                Some(i) => &dir[..i],
                None => "",
            };
        }
    }
}

impl Rule {
    /// Whether the pattern matches the file at `relative`, a path below the directory
    /// of the attributes file.
    fn matches(&self, relative: &str) -> bool {
        if self.directory_only {
            return false;
        }
        let mut budget = MATCH_BUDGET;
        if self.basename {
            let name = relative.rsplit('/').next().unwrap_or(relative);
            wild(self.pattern.as_bytes(), name.as_bytes(), false, &mut budget)
        } else {
            wild(
                self.pattern.as_bytes(),
                relative.as_bytes(),
                true,
                &mut budget,
            )
        }
    }
}

/// One line of a `.gitattributes` file, when it names one of [`ATTRIBUTES`] for a
/// pattern git reads.
fn parse_line(line: &str) -> Option<Rule> {
    let line = line.trim_start_matches([' ', '\t']);
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (pattern, rest) = if let Some(quoted) = line.strip_prefix('"') {
        unquote(quoted)?
    } else {
        let end = line.find([' ', '\t', '\r']).unwrap_or(line.len());
        (line[..end].to_string(), &line[end..])
    };
    // A macro definition, and a negative pattern (which git ignores with a warning).
    if pattern.starts_with("[attr]") || pattern.starts_with('!') {
        return None;
    }
    let mut states = [None; 2];
    for token in rest.split([' ', '\t', '\r']).filter(|t| !t.is_empty()) {
        let (name, state) = if let Some(name) = token.strip_prefix(['-', '!']) {
            (name, State::NotSet)
        } else if let Some((name, value)) = token.split_once('=') {
            let state = if value == "true" {
                State::Set
            } else {
                State::NotSet
            };
            (name, state)
        } else {
            (token, State::Set)
        };
        if let Some(index) = ATTRIBUTES.iter().position(|a| *a == name) {
            // Within one line the later mention wins, as between lines.
            states[index] = Some(state);
        }
    }
    if states.iter().all(Option::is_none) {
        return None;
    }
    let (pattern, directory_only) = match pattern.strip_suffix('/') {
        Some(stripped) => (stripped.to_string(), true),
        None => (pattern, false),
    };
    let basename = !pattern.contains('/');
    let pattern = pattern.strip_prefix('/').unwrap_or(&pattern).to_string();
    if pattern.is_empty() {
        return None;
    }
    Some(Rule {
        pattern,
        basename,
        directory_only,
        states,
    })
}

/// A pattern written in double quotes, C style, with what follows the closing quote.
/// `None` when the quote is not closed.
fn unquote(quoted: &str) -> Option<(String, &str)> {
    let mut out = Vec::new();
    let bytes = quoted.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Some((String::from_utf8_lossy(&out).into_owned(), &quoted[i + 1..])),
            b'\\' => {
                i += 1;
                let escaped = *bytes.get(i)?;
                match escaped {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'0'..=b'3' => {
                        let digits = bytes.get(i..i + 3)?;
                        let mut value = 0u32;
                        for d in digits {
                            if !(b'0'..=b'7').contains(d) {
                                return None;
                            }
                            value = value * 8 + u32::from(d - b'0');
                        }
                        out.push(u8::try_from(value).ok()?);
                        i += 2;
                    }
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    None
}

/// git's `wildmatch`: `?` and `*` (which stop at a `/` when `pathname` is set), `**`
/// as a whole path segment crossing directories, `[...]` classes and `\` escapes.
/// Without `pathname` the text holds no `/` to stop at (a base name), or the caller
/// wants `fnmatch` semantics where `*` crosses everything.
pub(crate) fn wildmatch(pattern: &str, text: &str, pathname: bool) -> bool {
    let mut budget = MATCH_BUDGET;
    wild(pattern.as_bytes(), text.as_bytes(), pathname, &mut budget)
}

fn wild(pattern: &[u8], text: &[u8], pathname: bool, budget: &mut u32) -> bool {
    let (mut p, mut t) = (0, 0);
    while p < pattern.len() {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        match pattern[p] {
            b'\\' => {
                p += 1;
                if p >= pattern.len() || t >= text.len() || text[t] != pattern[p] {
                    return false;
                }
            }
            b'?' => {
                if t >= text.len() || (pathname && text[t] == b'/') {
                    return false;
                }
            }
            b'*' => {
                let mut next = p + 1;
                let crosses = if next < pattern.len() && pattern[next] == b'*' {
                    let starts_segment = p == 0 || pattern[p - 1] == b'/';
                    while next < pattern.len() && pattern[next] == b'*' {
                        next += 1;
                    }
                    if !pathname {
                        true
                    } else if starts_segment && (next == pattern.len() || pattern[next] == b'/') {
                        // `**/` also stands for no directory at all.
                        if next < pattern.len()
                            && wild(&pattern[next + 1..], &text[t..], pathname, budget)
                        {
                            return true;
                        }
                        true
                    } else {
                        // `a**b` is one `*`.
                        false
                    }
                } else {
                    !pathname
                };
                let rest = &pattern[next..];
                if rest.is_empty() {
                    return crosses || !text[t..].contains(&b'/');
                }
                let mut at = t;
                loop {
                    if wild(rest, &text[at..], pathname, budget) {
                        return true;
                    }
                    if *budget == 0 || at >= text.len() || (!crosses && text[at] == b'/') {
                        return false;
                    }
                    at += 1;
                }
            }
            b'[' => {
                if t >= text.len() || (pathname && text[t] == b'/') {
                    return false;
                }
                match class_matches(&pattern[p..], text[t]) {
                    Some((true, len)) => p += len - 1,
                    _ => return false,
                }
            }
            literal => {
                if t >= text.len() || text[t] != literal {
                    return false;
                }
            }
        }
        p += 1;
        t += 1;
    }
    t == text.len()
}

/// Whether the bracket expression at the start of `pattern` holds `c`, with the
/// expression's length. `None` when the bracket is not closed.
fn class_matches(pattern: &[u8], c: u8) -> Option<(bool, usize)> {
    let mut i = 1;
    let negated = matches!(pattern.get(i), Some(b'!' | b'^'));
    if negated {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let mut item = *pattern.get(i)?;
        if item == b']' && !first {
            return Some((matched != negated, i + 1));
        }
        first = false;
        if item == b'[' && pattern.get(i + 1) == Some(&b':') {
            let close = pattern[i + 2..].windows(2).position(|w| w == b":]")?;
            let name = &pattern[i + 2..i + 2 + close];
            matched |= match name {
                b"alnum" => c.is_ascii_alphanumeric(),
                b"alpha" => c.is_ascii_alphabetic(),
                b"digit" => c.is_ascii_digit(),
                b"upper" => c.is_ascii_uppercase(),
                b"lower" => c.is_ascii_lowercase(),
                b"space" => c.is_ascii_whitespace(),
                b"punct" => c.is_ascii_punctuation(),
                b"xdigit" => c.is_ascii_hexdigit(),
                b"blank" => c == b' ' || c == b'\t',
                b"cntrl" => c.is_ascii_control(),
                b"graph" => c.is_ascii_graphic(),
                b"print" => c.is_ascii_graphic() || c == b' ',
                _ => return None,
            };
            i += close + 4;
            continue;
        }
        if item == b'\\' {
            i += 1;
            item = *pattern.get(i)?;
        }
        if pattern.get(i + 1) == Some(&b'-') && pattern.get(i + 2).is_some_and(|e| *e != b']') {
            let mut end = pattern[i + 2];
            let mut used = 3;
            if end == b'\\' {
                end = *pattern.get(i + 3)?;
                used = 4;
            }
            matched |= item <= c && c <= end;
            i += used;
        } else {
            matched |= item == c;
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "\
# comment linguist-vendored
vendor/** linguist-vendored
third_party/ linguist-vendored
dironly.js/ linguist-vendored
named linguist-vendored
*.pb.go linguist-generated
/top.js linguist-generated
docs/api/*.js linguist-generated=true
keep/** linguist-vendored
keep/** -linguist-vendored
late/** -linguist-vendored
late/** linguist-vendored
value/** linguist-vendored=false
both/** linguist-generated linguist-vendored
\"quoted dir/**\" linguist-vendored
!negative/** linguist-vendored
[attr]vend linguist-vendored
macro/** vend
";

    fn attributes(files: &[(&str, &str)]) -> GitAttributes {
        let mut attrs = GitAttributes::default();
        for (path, content) in files {
            attrs.add_file(path, content);
        }
        attrs
    }

    fn set(attrs: &GitAttributes, path: &str) -> Option<&'static str> {
        attrs.set_on(path).map(|(name, _)| name)
    }

    #[test]
    fn patterns_follow_gits_attribute_rules() {
        let attrs = attributes(&[(".gitattributes", ROOT)]);
        for (path, expected) in [
            // `dir/**` holds everything below; `dir/` and a bare name hold no file inside.
            ("vendor/a.js", Some("linguist-vendored")),
            ("vendor/deep/er/a.js", Some("linguist-vendored")),
            ("x/vendor/a.js", None),
            ("third_party/a.js", None),
            // A pattern with a trailing slash names directories: a file of that name
            // is not one.
            ("x/dironly.js", None),
            ("named/a.js", None),
            ("x/named", Some("linguist-vendored")),
            // No slash: the base name at any depth. A leading slash anchors.
            ("a.pb.go", Some("linguist-generated")),
            ("x/y/a.pb.go", Some("linguist-generated")),
            ("top.js", Some("linguist-generated")),
            ("x/top.js", None),
            // `*` stays inside one directory.
            ("docs/api/a.js", Some("linguist-generated")),
            ("docs/api/v1/a.js", None),
            // The later line wins, either way.
            ("keep/a.js", None),
            ("late/a.js", Some("linguist-vendored")),
            ("value/a.js", None),
            // Both set: the first of `ATTRIBUTES` is the one reported.
            ("both/a.js", Some("linguist-vendored")),
            ("quoted dir/a.js", Some("linguist-vendored")),
            // A negative pattern and a macro are not read.
            ("negative/a.js", None),
            ("macro/a.js", None),
            ("comment", None),
        ] {
            assert_eq!(set(&attrs, path), expected, "{path}");
        }
    }

    #[test]
    fn a_nearer_file_wins_and_names_itself() {
        let attrs = attributes(&[
            (".gitattributes", "*.js linguist-vendored\n"),
            (
                "keep/.gitattributes",
                "*.js -linguist-vendored\nagain/*.js linguist-vendored\n",
            ),
            ("keep/reset/.gitattributes", "*.js !linguist-vendored\n"),
            ("gen/.gitattributes", "out/** linguist-generated\n"),
        ]);
        assert_eq!(
            attrs.set_on("a.js"),
            Some(("linguist-vendored", ".gitattributes".to_string()))
        );
        assert_eq!(attrs.set_on("keep/a.js"), None);
        assert_eq!(attrs.set_on("keep/deep/a.js"), None);
        assert_eq!(
            attrs.set_on("keep/again/a.js"),
            Some(("linguist-vendored", "keep/.gitattributes".to_string()))
        );
        assert_eq!(attrs.set_on("keep/reset/a.js"), None);
        // A nested file's patterns are relative to its own directory.
        assert_eq!(
            attrs.set_on("gen/out/a.go"),
            Some(("linguist-generated", "gen/.gitattributes".to_string()))
        );
        assert_eq!(attrs.set_on("out/a.go"), None);
        // An attribute the nearer file does not name is still read from above.
        assert_eq!(
            attrs.set_on("gen/out/a.js"),
            Some(("linguist-vendored", ".gitattributes".to_string()))
        );
    }

    #[test]
    fn wildmatch_reads_stars_classes_and_escapes() {
        for (pattern, text, expected) in [
            ("**/gen/**", "gen/a.js", true),
            ("**/gen/**", "x/y/gen/a/b.js", true),
            ("**/gen/**", "x/generated/a.js", false),
            ("a/**/z.js", "a/z.js", true),
            ("a/**/z.js", "a/b/c/z.js", true),
            ("a/**/z.js", "a/b/c/y.js", false),
            ("b/**c/d.js", "b/xc/d.js", true),
            ("b/**c/d.js", "b/x/c/d.js", false),
            ("a/*", "a/b", true),
            ("a/*", "a/b/c", false),
            ("a/**", "a/b/c", true),
            ("q/?.js", "q/a.js", true),
            ("q/?.js", "q/ab.js", false),
            ("q/?.js", "q//.js", false),
            ("c/[ab]*.js", "c/a1.js", true),
            ("c/[ab]*.js", "c/c1.js", false),
            ("n/[!a]*.js", "n/b1.js", true),
            ("n/[!a]*.js", "n/a1.js", false),
            ("r/[a-c][[:digit:]].js", "r/b7.js", true),
            ("r/[a-c][[:digit:]].js", "r/d7.js", false),
            ("e/\\*.js", "e/*.js", true),
            ("e/\\*.js", "e/a.js", false),
            ("u/[ab.js", "u/a.js", false),
        ] {
            assert_eq!(wildmatch(pattern, text, true), expected, "{pattern} {text}");
        }
        // Without path names `*` crosses everything, as `fnmatch` does.
        assert!(wildmatch("*.egg", "pkg.egg", false));
        assert!(wildmatch("{arch}", "{arch}", false));
        assert!(!wildmatch("{arch}", "arch", false));
        assert!(wildmatch("*/a/*", "x/a/b/c", false));
    }

    #[test]
    fn a_pattern_that_would_not_finish_reads_as_no_match() {
        let pattern = "*a".repeat(40) + "b";
        let text = "a".repeat(200);
        let attrs = attributes(&[(".gitattributes", &format!("{pattern} linguist-vendored\n"))]);
        assert_eq!(attrs.set_on(&text), None);
    }
}
