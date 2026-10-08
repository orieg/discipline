//! Which files a rule table reads, and how each format is loaded into one generic tree.

use super::engine::{segment_matches, Rule};
use serde_json::Value;

/// [`classify`] over another gate's rule table and executable list. A file pattern is a
/// basename (`*` matches within it) or `dir/basename`.
pub fn classify_with(
    path: &str,
    rules: &'static [Rule],
    executable: &[&str],
) -> Option<Classified> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let tail2 = {
        let mut it = path.rsplitn(3, '/');
        let a = it.next().unwrap_or("");
        match it.next() {
            Some(b) => format!("{b}/{a}"),
            None => a.to_string(),
        }
    };
    let matches = |pat: &str| {
        if let Some(prefix) = pat.strip_suffix(".*.json") {
            name.starts_with(&format!("{prefix}."))
                && name.ends_with(".json")
                && name.len() > prefix.len() + 6
        } else if pat.contains('/') {
            if pat.contains('*') {
                let (pd, pb) = pat.split_once('/').unwrap_or(("", pat));
                let (td, tb) = tail2.split_once('/').unwrap_or(("", &tail2));
                segment_matches(pd, td) && segment_matches(pb, tb)
            } else {
                tail2 == pat
            }
        } else {
            segment_matches(pat, name)
        }
    };
    if executable.iter().any(|e| matches(e)) {
        return Some(Classified::Executable);
    }
    let rules: Vec<&'static Rule> = rules
        .iter()
        .filter(|r| r.files.iter().any(|f| matches(f)))
        .collect();
    if rules.is_empty() {
        return None;
    }
    Some(Classified::Data {
        name: name.to_string(),
        rules,
    })
}

#[derive(Debug)]
pub enum Classified {
    Executable,
    Data {
        name: String,
        rules: Vec<&'static Rule>,
    },
}

/// Strip `//` and `/* */` comments and trailing commas outside strings (JSON with comments,
/// as `tsconfig.json` and `.eslintrc` allow).
fn strip_jsonc(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            '/' if b.get(i + 1) == Some(&'/') => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if b.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                // Drop the comma when the next non-space char closes a container.
                let mut j = i + 1;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if !(j < b.len() && (b[j] == '}' || b[j] == ']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// A minimal INI reader: `[section]`, `key = value` / `key: value`, `;`/`#` comments,
/// indented continuation lines joined into a list. Values stay strings; lists are split
/// on newlines and commas.
fn parse_ini(src: &str) -> Value {
    let mut root = serde_json::Map::new();
    let mut section = String::new();
    let mut last_key: Option<String> = None;
    for raw in src.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with([';', '#']) {
            continue;
        }
        if let Some(name) = line
            .trim()
            .strip_prefix('[')
            .and_then(|l| l.strip_suffix(']'))
        {
            section = name.trim().to_string();
            root.entry(section.clone())
                .or_insert(Value::Object(Default::default()));
            last_key = None;
            continue;
        }
        let is_continuation = raw.starts_with([' ', '\t']);
        let table = root
            .entry(section.clone())
            .or_insert(Value::Object(Default::default()))
            .as_object_mut()
            .expect("sections are objects");
        if is_continuation {
            if let Some(k) = &last_key {
                let existing = table.get(k).cloned().unwrap_or(Value::Null);
                let mut items: Vec<Value> = match existing {
                    Value::Array(a) => a,
                    Value::String(s) if s.is_empty() => Vec::new(),
                    Value::String(s) => vec![Value::String(s)],
                    _ => Vec::new(),
                };
                items.push(Value::String(line.trim().to_string()));
                table.insert(k.clone(), Value::Array(items));
            }
            continue;
        }
        let Some((k, v)) = line.split_once(['=', ':']) else {
            continue;
        };
        let key = k.trim().to_string();
        let value = v.trim();
        let parsed = if value.contains(',') {
            Value::Array(
                value
                    .split(',')
                    .map(|x| Value::String(x.trim().to_string()))
                    .filter(|x| x.as_str() != Some(""))
                    .collect(),
            )
        } else {
            Value::String(value.to_string())
        };
        table.insert(key.clone(), parsed);
        last_key = Some(key);
    }
    Value::Object(root)
}

/// A minimal XML reader for PHPUnit: root element attributes only.
fn parse_phpunit_xml(src: &str) -> Option<Value> {
    let start = src.find("<phpunit")?;
    let end = src[start..].find('>')? + start;
    let tag = &src[start + "<phpunit".len()..end];
    let mut attrs = serde_json::Map::new();
    let mut rest = tag;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq]
            .trim()
            .rsplit(char::is_whitespace)
            .next()?
            .to_string();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        let close = after[1..].find(quote)? + 1;
        attrs.insert(key, Value::String(after[1..close].to_string()));
        rest = &after[close + 1..];
    }
    let mut root = serde_json::Map::new();
    root.insert("phpunit".to_string(), Value::Object(attrs));
    Some(Value::Object(root))
}

/// Load a configuration file into one generic tree. `None` = not parseable as its format.
pub fn load(name: &str, content: &str) -> Option<Value> {
    if name.ends_with(".toml") || name == ".cargo/config" || name == "config" {
        let v: toml::Value = toml::from_str(content).ok()?;
        serde_json::to_value(v).ok()
    } else if name.ends_with(".json") || name.ends_with(".jsonc") || name == ".eslintrc" {
        serde_json::from_str(&strip_jsonc(content)).ok()
    } else if name.ends_with(".yml") || name.ends_with(".yaml") {
        let v: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
        serde_json::to_value(v).ok()
    } else if name.ends_with(".neon") {
        // NEON is YAML-shaped for the keys this gate reads.
        let v: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
        serde_json::to_value(v).ok()
    } else if name.ends_with(".xml") || name.ends_with(".xml.dist") {
        parse_phpunit_xml(content)
    } else {
        Some(parse_ini(content))
    }
}

/// [`load`] for a side that may be absent: an absent file is an empty tree.
pub fn load_or_empty(name: &str, content: Option<&str>) -> Option<Value> {
    match content {
        None => Some(Value::Object(Default::default())),
        Some(src) => load(name, src),
    }
}
