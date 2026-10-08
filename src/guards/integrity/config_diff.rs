//! The comparison of two configurations: every key whose head value is looser than its
//! base value, with the reason.

use super::{
    direction_of, Direction, ABSENT_IS_NONE, ABSENT_IS_UNLIMITED, ADDED_IS_ANOTHER_BASIS,
    ENTRY_SHAPES,
};
use crate::config::{DisciplineConfig, RunMode};
use anyhow::Result;
use toml::Value;

/// Whether `entry`, a base entry of `key` missing from head as written, is on head under the
/// same identity with only tightening edits. Anything ambiguous reads as lost.
fn entry_tightened(key: &str, entry: &Value, base: &[Value], head: &[Value]) -> bool {
    let Some(shape) = ENTRY_SHAPES.iter().find(|s| s.key == key) else {
        return false;
    };
    let Some(b) = entry.as_table() else {
        return false;
    };
    let id = |t: &toml::Table| -> Vec<Option<Value>> {
        shape.identity.iter().map(|f| t.get(*f).cloned()).collect()
    };
    let wanted = id(b);
    if wanted.iter().any(Option::is_none) {
        return false;
    }
    let named = |list: &[Value]| -> Vec<toml::Table> {
        list.iter()
            .filter_map(Value::as_table)
            .filter(|t| id(t) == wanted)
            .cloned()
            .collect()
    };
    let (on_base, on_head) = (named(base), named(head));
    let ([_], [h]) = (on_base.as_slice(), on_head.as_slice()) else {
        return false;
    };
    b.keys().chain(h.keys()).all(|field| {
        let (bv, hv) = (b.get(field), h.get(field));
        if shape.stricter_when_grown.contains(&field.as_str()) {
            within(bv, hv)
        } else if shape.stricter_when_shrunk.contains(&field.as_str()) {
            within(hv, bv)
        } else if shape.stricter_when_added.contains(&field.as_str()) && bv.is_none() {
            // Over a preset that supplies the field, the added value replaces the
            // preset's instead of adding a check: stricter only when it is that value.
            preset_default(b, field).is_none_or(|d| Some(&d) == hv)
        } else {
            // A field one side leaves to its preset reads as the preset's value, so the
            // default written down, or removed again, is no edit.
            let in_force = |t: &toml::Table, v: Option<&Value>| {
                v.cloned().or_else(|| preset_default(t, field))
            };
            in_force(b, bv) == in_force(h, hv)
        }
    })
}

/// What a `command` table (the gate's own, or a `commands` entry) reads for `key` when it
/// leaves it unset and names a preset that supplies it. `None` when unset means no such
/// check: no preset, an unknown one, or a key the preset does not supply.
fn preset_default(table: &toml::Table, key: &str) -> Option<Value> {
    let preset = table.get("preset")?.as_str()?;
    super::presets::resolve_preset(preset)?
        .supplied_default(key)
        .map(|v| Value::String(v.to_string()))
}

/// [`preset_default`] for the keys that decide what evidence the command's output is held
/// to and execute nothing: the guards of a preset.
fn preset_guard_default(table: &toml::Table, key: &str) -> Option<Value> {
    let preset = table.get("preset")?.as_str()?;
    super::presets::resolve_preset(preset)?
        .replaced_default(key)
        .map(|v| Value::String(v.to_string()))
}

const FROM_PRESET: &str = "the base value is its preset's default";
const FROM_BUILTIN: &str = "the base value is the built-in default";
const FROM_NEW_PRESET: &str = "compared with the default of the preset this change names";

/// Evidence keys of a `commands` entry a preset supplies a guard for.
const PRESET_GUARD_KEYS: &[&str] = &[
    "zero_items_pattern",
    "canary_expected_diagnostic",
    "snapshot",
];

/// What gate `gate` reads for the evidence key `key` when `table` leaves it unset, where
/// unset is a value rather than "no such check", with the words the finding says it in:
/// the built-in `issue-link` `pattern`, and a `command` key its table's preset supplies.
/// Every other optional evidence key unset means the check it names is not made, so
/// adding it adds a check (`optional_evidence_keys_are_inventoried` lists them).
fn effective_default(gate: &str, table: &toml::Table, key: &str) -> Option<(Value, &'static str)> {
    match (gate, key) {
        ("issue-link", "pattern") => Some((
            Value::String(super::issue_link::DEFAULT_ISSUE_PATTERN.to_string()),
            FROM_BUILTIN,
        )),
        ("command", _) => preset_default(table, key).map(|v| (v, FROM_PRESET)),
        _ => None,
    }
}

/// Every item of the list `small` is in the list `big`; an absent list reads as empty.
fn within(small: Option<&Value>, big: Option<&Value>) -> bool {
    fn items(v: Option<&Value>) -> Option<&[Value]> {
        match v {
            None => Some(&[]),
            Some(Value::Array(a)) => Some(a),
            Some(_) => None,
        }
    }
    match (items(small), items(big)) {
        (Some(s), Some(g)) => s.iter().all(|x| g.contains(x)),
        _ => false,
    }
}

/// The numeric components of a version written as dotted numbers (`1.90`, `1.90.1`), with
/// trailing zero components dropped so that `1.90` and `1.90.0` are the same version.
/// `None` for any other text: a pre-release suffix, an empty component, a sign.
fn version_components(text: &str) -> Option<Vec<u64>> {
    let mut parts = text
        .trim()
        .split('.')
        .map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u64>().ok())
                .flatten()
        })
        .collect::<Option<Vec<u64>>>()?;
    while parts.last() == Some(&0) && parts.len() > 1 {
        parts.pop();
    }
    Some(parts)
}

/// How version `head` stands to version `base`, component by component as numbers
/// (`1.9` is below `1.10`). `None` when either is not a dotted-number version, so the
/// two cannot be ordered.
pub fn version_order(base: &str, head: &str) -> Option<std::cmp::Ordering> {
    Some(version_components(head)?.cmp(&version_components(base)?))
}

/// Why a changed version that cannot be ordered is reported.
const UNORDERED_VERSION: &str =
    "one side is not a dotted-number version, so the change cannot be shown to be no lower";

/// What the sentence of an added [`ADDED_IS_ANOTHER_BASIS`] key says.
const ANOTHER_BASIS: &str =
    "the tests are then counted on a basis the base ref did not use, so its floor is compared with a different count";

/// What the sentence of a `base_report` added beside an existing `test_report` says.
const BASE_SIDE_MOVED: &str =
    "the head report was compared with `test_report` as the base ref committed it, and is now compared with a file the runner supplies";

/// How a configuration option moved in the looser direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Change {
    /// A switch or a named value changed to a looser one.
    Changed,
    /// An option that bounds or names the check was removed.
    Removed,
    Increased,
    Decreased,
    /// A severity lowered.
    Lowered,
    /// A list gained entries.
    Gained,
    /// A list lost entries.
    Lost,
    /// An allow-list was emptied, which switches it off.
    Emptied,
}

/// One loosening of a configuration option, as data. [`Weakening::what`] is the sentence
/// the `config-integrity` finding shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Weakening {
    /// The gate id, or the table (`directives`, `tests`, `languages`, `meta`).
    pub gate: String,
    /// The option, as written under that table (`c.macros` under `languages`).
    pub key: String,
    pub change: Change,
    /// The value before and after, as the message shows them; `None` for a list, whose
    /// entries are counted, never named (an entry can be a login).
    pub before: Option<String>,
    pub after: Option<String>,
    /// Entries gained or lost, for a list.
    pub count: Option<usize>,
    /// Why the change is looser, when the sentence says so.
    #[serde(skip)]
    pub note: Option<&'static str>,
}

impl Weakening {
    fn new(gate: &str, key: &str, change: Change) -> Self {
        Weakening {
            gate: gate.to_string(),
            key: key.to_string(),
            change,
            before: None,
            after: None,
            count: None,
            note: None,
        }
    }

    fn values(mut self, before: impl ToString, after: impl ToString) -> Self {
        self.before = Some(before.to_string());
        self.after = Some(after.to_string());
        self
    }

    fn was(mut self, before: impl ToString) -> Self {
        self.before = Some(before.to_string());
        self
    }

    fn count(mut self, n: usize) -> Self {
        self.count = Some(n);
        self
    }

    /// The key the weakening is about.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The sentence the finding shows, starting with the key in backticks.
    pub fn what(&self) -> String {
        let key = &self.key;
        let b = self.before.as_deref().unwrap_or("");
        let a = self.after.as_deref().unwrap_or("");
        let n = self.count.unwrap_or(0);
        let text = match self.change {
            Change::Changed => format!("`{key}` changed from {b} to {a}"),
            Change::Removed => format!("`{key}` removed (was {b})"),
            Change::Increased => format!("`{key}` increased from {b} to {a}"),
            Change::Decreased => format!("`{key}` decreased from {b} to {a}"),
            Change::Lowered => format!("`{key}` lowered from {b} to {a}"),
            Change::Gained if self.after.is_some() => format!("`{key}` gained {a}"),
            Change::Gained => format!("`{key}` gained {n} entr(y/ies)"),
            Change::Lost => format!("`{key}` lost {n} entr(y/ies)"),
            Change::Emptied => format!("`{key}` emptied, which switches the allow-list off"),
        };
        match self.note {
            Some(note) => format!("{text} ({note})"),
            None => text,
        }
    }
}

/// `error` > `warning` > `note`, for severities as configuration strings.
fn severity_rank(s: &str) -> u8 {
    match s {
        "error" => 2,
        "warning" => 1,
        _ => 0,
    }
}

pub fn diff_configs(base: &DisciplineConfig, head: &DisciplineConfig) -> Result<Vec<Weakening>> {
    diff_configs_under(base, head, false)
}

/// [`diff_configs`], told whether the runner authorises a change to the commands a gate
/// executes (`DISCIPLINE_ALLOW_COMMAND_CHANGE`). The authorisation covers what is
/// executed, not what evidence is accepted: a `command` preset the head is then allowed to
/// name becomes the reference its guards are compared with, so a guard key written beside
/// the new preset in place of the preset's own is still reported.
pub fn diff_configs_under(
    base: &DisciplineConfig,
    head: &DisciplineConfig,
    command_change_authorised: bool,
) -> Result<Vec<Weakening>> {
    let mut found = Vec::new();

    // Check [directives] table
    let mut dir_note = |w: Weakening| found.push(w);
    let dir = |key: &str, change: Change| Weakening::new("directives", key, change);
    if !base.directives.allow_hidden && head.directives.allow_hidden {
        dir_note(dir("allow_hidden", Change::Changed).values(false, true));
    }
    let gained_sources: Vec<_> = head
        .directives
        .sources
        .iter()
        .filter(|s| !base.directives.sources.contains(s))
        .collect();
    if gained_sources.iter().any(|s| s.as_str() == "commits") {
        let mut w = dir("sources", Change::Gained).count(1);
        w.after = Some("commits".to_string());
        dir_note(w);
    }
    if base.directives.fail_on_overrides && !head.directives.fail_on_overrides {
        dir_note(dir("fail_on_overrides", Change::Changed).values(true, false));
    }
    // A listed actor is exempt from `fail_on_overrides`.
    let gained_actors = head
        .directives
        .allowed_override_actors
        .iter()
        .filter(|a| !base.directives.allowed_override_actors.contains(a))
        .count();
    if gained_actors > 0 {
        dir_note(dir("allowed_override_actors", Change::Gained).count(gained_actors));
    }

    match (base.directives.max_overrides, head.directives.max_overrides) {
        (Some(b), None) => dir_note(dir("max_overrides", Change::Removed).was(b)),
        (Some(b), Some(h)) if h > b => {
            dir_note(dir("max_overrides", Change::Increased).values(b, h))
        }
        _ => {}
    }
    match (
        base.directives.max_inline_overrides,
        head.directives.max_inline_overrides,
    ) {
        (Some(b), None) => dir_note(dir("max_inline_overrides", Change::Removed).was(b)),
        (Some(b), Some(h)) if h > b => {
            dir_note(dir("max_inline_overrides", Change::Increased).values(b, h))
        }
        _ => {}
    }
    if base.directives.require_approval && !head.directives.require_approval {
        dir_note(dir("require_approval", Change::Changed).values(true, false));
    }
    if !base.directives.degrade_offline && head.directives.degrade_offline {
        let mut w = dir("degrade_offline", Change::Changed).values(false, true);
        w.note = Some("a failed merged-pr-body lookup no longer stops the run");
        dir_note(w);
    }
    // (true -> false is stricter: a failed lookup then stops the run.)

    // [tests]: widening what counts as test code narrows what the production-code gates see.
    for (key, b, h) in [
        ("functions", &base.tests.functions, &head.tests.functions),
        ("paths", &base.tests.paths, &head.tests.paths),
    ] {
        let gained = h.iter().filter(|x| !b.contains(x)).count();
        if gained > 0 {
            found.push(Weakening::new("tests", key, Change::Gained).count(gained));
        }
    }

    // [languages.c]: a macro blanked before parsing is code no gate reads.
    for (key, b, h) in [
        (
            "c.macros",
            &base.languages.c.macros,
            &head.languages.c.macros,
        ),
        (
            "c.function_macros",
            &base.languages.c.function_macros,
            &head.languages.c.function_macros,
        ),
    ] {
        let gained = h.iter().filter(|x| !b.contains(x)).count();
        if gained > 0 {
            found.push(Weakening::new("languages", key, Change::Gained).count(gained));
        }
    }

    // [meta]: advisory mode exits 0 whatever the gates found.
    if base.meta.mode == RunMode::Enforcing && head.meta.mode == RunMode::Advisory {
        found.push(Weakening::new("meta", "mode", Change::Changed).values("enforcing", "advisory"));
    }

    let base_v = Value::try_from(&base.gates)?;
    let head_v = Value::try_from(&head.gates)?;
    let (Some(base_t), Some(head_t)) = (base_v.as_table(), head_v.as_table()) else {
        return Ok(found);
    };

    for (gate, base_gate) in base_t {
        let (Some(b), Some(h)) = (
            base_gate.as_table(),
            head_t.get(gate).and_then(Value::as_table),
        ) else {
            continue;
        };
        let mut note = |w: Weakening| found.push(w);
        let w = |key: &str, change: Change| Weakening::new(gate, key, change);
        for (key, bv) in b {
            // An option this binary does not know is judged as a plain switch, so a
            // stale table degrades to the strict reading rather than to silence.
            let dir = direction_of(key).unwrap_or(Direction::LooserWhenFalse);
            // Only an optional key can be absent on the head side. Its removal is a
            // weakening when the absent reading is looser than the value removed.
            let Some(hv) = h.get(key) else {
                match dir {
                    // Unset reads as the default: removing that very value changes nothing.
                    Direction::Evidence
                        if effective_default(gate, h, key).is_some_and(|(d, _)| d == *bv) => {}
                    Direction::Evidence
                    | Direction::Floor
                    | Direction::Cap
                    | Direction::VersionFloor => note(w(key, Change::Removed).was(bv)),
                    // Absent means no limit at all (`max_noise_cv`: no noise check).
                    Direction::Tolerance if ABSENT_IS_UNLIMITED.contains(&key.as_str()) => {
                        note(w(key, Change::Removed).was(bv))
                    }
                    // A gate's `allow_hidden = false` removed: the gate then inherits
                    // `[directives] allow_hidden`, a loosening when that is `true`.
                    Direction::LooserWhenTrue
                        if *bv == Value::Boolean(false)
                            && key == "allow_hidden"
                            && head.directives.allow_hidden =>
                    {
                        note(w(key, Change::Removed).was(bv))
                    }
                    // An optional severity removed: the gate's `severity` applies again,
                    // a loosening when that is lower than the value removed.
                    Direction::Severity => {
                        if let (Value::String(was), Some(Value::String(now))) =
                            (bv, h.get("severity"))
                        {
                            if severity_rank(now) < severity_rank(was) {
                                note(w(key, Change::Lowered).values(was, now));
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            };
            let num = |v: &Value| match v {
                Value::Integer(i) => Some(*i as f64),
                Value::Float(f) => Some(*f),
                _ => None,
            };
            match dir {
                Direction::Evidence if bv != hv => note(w(key, Change::Changed).values(bv, hv)),
                Direction::StrictMode(strict) => {
                    if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                        if bs == strict && hs != strict {
                            note(w(key, Change::Changed).values(bs, hs));
                        }
                    }
                }
                Direction::Ordered(order) => {
                    if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                        let rank = |v: &str| order.iter().position(|o| *o == v);
                        if let (Some(b), Some(h)) = (rank(bs), rank(hs)) {
                            if h > b {
                                note(w(key, Change::Changed).values(bs, hs));
                            }
                        }
                    }
                }
                Direction::LooserWhenFalse
                    if matches!((bv, hv), (Value::Boolean(true), Value::Boolean(false))) =>
                {
                    note(w(key, Change::Changed).values(true, false))
                }
                Direction::LooserWhenTrue
                    if matches!((bv, hv), (Value::Boolean(false), Value::Boolean(true))) =>
                {
                    note(w(key, Change::Changed).values(false, true))
                }
                Direction::Floor => {
                    if let (Some(bn), Some(hn)) = (num(bv), num(hv)) {
                        if hn < bn {
                            note(w(key, Change::Decreased).values(bv, hv));
                        }
                    }
                }
                Direction::VersionFloor if bv != hv => {
                    let order = match (bv, hv) {
                        (Value::String(b), Value::String(h)) => version_order(b, h),
                        _ => None,
                    };
                    match order {
                        Some(std::cmp::Ordering::Less) => {
                            note(w(key, Change::Decreased).values(bv, hv))
                        }
                        Some(_) => {}
                        // A value that is not a version cannot be shown to be no lower.
                        None => {
                            let mut changed = w(key, Change::Changed).values(bv, hv);
                            changed.note = Some(UNORDERED_VERSION);
                            note(changed);
                        }
                    }
                }
                Direction::Cap | Direction::Tolerance => {
                    if let (Some(bn), Some(hn)) = (num(bv), num(hv)) {
                        if hn > bn {
                            note(w(key, Change::Increased).values(bv, hv));
                        }
                    }
                }
                Direction::Severity => {
                    if let (Value::String(bs), Value::String(hs)) = (bv, hv) {
                        if (bs == "error" && (hs == "warning" || hs == "note"))
                            || (bs == "warning" && hs == "note")
                        {
                            note(w(key, Change::Lowered).values(bs, hs));
                        }
                    }
                }
                Direction::Grown | Direction::Shrunk | Direction::Allowlist => {
                    let (Value::Array(ba), Value::Array(ha)) = (bv, hv) else {
                        continue;
                    };
                    let gained = ha.iter().filter(|x| !ba.contains(x)).count();
                    let lost = ba
                        .iter()
                        .filter(|x| !ha.contains(x) && !entry_tightened(key, x, ba, ha))
                        .count();
                    if dir == Direction::Allowlist && !ba.is_empty() && ha.is_empty() {
                        note(w(key, Change::Emptied));
                    } else if dir == Direction::Allowlist && ba.is_empty() {
                        // No allow-list on base: adopting one is a tightening.
                    } else if dir != Direction::Shrunk && gained > 0 {
                        note(w(key, Change::Gained).count(gained));
                    } else if dir == Direction::Shrunk && lost > 0 {
                        note(w(key, Change::Lost).count(lost));
                    }
                }
                _ => {}
            }
        }
        // A key only on head is an optional key the base left unset, judged by what unset
        // means: `ci_skip_severity` unset is the gate's `severity`; a tolerance in
        // `ABSENT_IS_NONE` unset adds none; a gate's `allow_hidden` unset inherits
        // `[directives] allow_hidden`; an evidence key with an effective default unset is
        // that default (`effective_default`); an evidence key in `ADDED_IS_ANOTHER_BASIS`
        // replaces the basis the gate counted on. Any other added floor, cap, version or
        // evidence key adds a check.
        let names_new_preset = gate == "command"
            && command_change_authorised
            && h.get("preset").is_some_and(|p| b.get("preset") != Some(p));
        for (key, hv) in h {
            if b.contains_key(key) {
                continue;
            }
            match direction_of(key) {
                // Judged as the same key changed from the default would be: any other
                // value is reported, the default written down is not.
                Some(Direction::Evidence) => {
                    let reference = effective_default(gate, b, key).or_else(|| {
                        names_new_preset
                            .then(|| preset_guard_default(h, key))
                            .flatten()
                            .map(|d| (d, FROM_NEW_PRESET))
                    });
                    match reference {
                        Some((default, why)) if default != *hv => {
                            let mut changed = w(key, Change::Changed).values(default, hv);
                            changed.note = Some(why);
                            note(changed);
                        }
                        Some(_) => {}
                        // No value stood in: the gate counted on another basis.
                        None if ADDED_IS_ANOTHER_BASIS.contains(&(gate.as_str(), key.as_str())) => {
                            let mut changed = w(key, Change::Changed).values("unset", hv);
                            changed.note = Some(ANOTHER_BASIS);
                            note(changed);
                        }
                        // `base_report` beside a `test_report` the base already had: the
                        // base side of the comparison was that file as the base ref
                        // committed it, and is now a file the runner supplies.
                        None if gate == "test-floor"
                            && key == "base_report"
                            && b.contains_key("test_report") =>
                        {
                            let mut changed = w(key, Change::Changed).values("unset", hv);
                            changed.note = Some(BASE_SIDE_MOVED);
                            note(changed);
                        }
                        None => {}
                    }
                }
                Some(Direction::Tolerance) if ABSENT_IS_NONE.contains(&key.as_str()) => {
                    let added = match hv {
                        Value::Integer(i) => *i as f64,
                        Value::Float(f) => *f,
                        _ => 0.0,
                    };
                    if added > 0.0 {
                        note(w(key, Change::Increased).values("unset", hv));
                    }
                }
                Some(Direction::LooserWhenTrue)
                    if key == "allow_hidden"
                        && *hv == Value::Boolean(true)
                        && !base.directives.allow_hidden =>
                {
                    note(w(key, Change::Changed).values("unset", hv));
                }
                _ => {}
            }
            if gate == "ignored-tests" && key == "ci_skip_severity" {
                if let (Some(Value::String(bs)), Value::String(hs)) = (b.get("severity"), hv) {
                    if (bs == "error" && (hs == "warning" || hs == "note"))
                        || (bs == "warning" && hs == "note")
                    {
                        note(w(key, Change::Lowered).values(bs, hs));
                    }
                }
            }
        }
        // An entry only the head has adds a command, which the runner authorised. Its
        // preset's guards are the reference for the guard keys written beside it.
        if gate == "command" && command_change_authorised {
            let entries = |t: &toml::Table| -> Vec<toml::Table> {
                t.get("commands")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_table).cloned().collect())
                    .unwrap_or_default()
            };
            let on_base = entries(b);
            for entry in entries(h) {
                let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
                if on_base.iter().any(|e| e.get("name") == entry.get("name")) {
                    continue;
                }
                for key in PRESET_GUARD_KEYS {
                    let (Some(hv), Some(default)) =
                        (entry.get(*key), preset_guard_default(&entry, key))
                    else {
                        continue;
                    };
                    if *hv != default {
                        let mut changed = w(&format!("commands[{name}].{key}"), Change::Changed)
                            .values(default, hv);
                        changed.note = Some(FROM_NEW_PRESET);
                        note(changed);
                    }
                }
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_an_optional_key_is_judged_by_what_absent_means() {
        let cfg = |body: &str| {
            DisciplineConfig::from_toml_str(&format!("[meta]\nversion = 1\nname = \"t\"\n{body}"))
                .unwrap()
        };
        let keys = |base: &str, head: &str| -> Vec<String> {
            diff_configs(&cfg(base), &cfg(head))
                .unwrap()
                .iter()
                .map(|w| format!("{}.{}", w.gate, w.key()))
                .collect()
        };
        // `max_noise_cv` absent is no noise check: a weakening.
        assert_eq!(
            keys(
                "[gates.bench-regression]\nmax_noise_cv = 0.05\n",
                "[gates.bench-regression]\n"
            ),
            ["bench-regression.max_noise_cv"]
        );
        // `noise_margin_pct` absent is 0: stricter, not reported.
        assert!(keys(
            "[gates.bench-regression]\nnoise_margin_pct = 2.0\n",
            "[gates.bench-regression]\n"
        )
        .is_empty());
        // A gate's `allow_hidden = false` removed loosens only when the global is `true`.
        let hidden = |global: bool, gate: &str| {
            format!("[directives]\nallow_hidden = {global}\n[gates.deletion-rationale]\n{gate}")
        };
        assert_eq!(
            keys(&hidden(true, "allow_hidden = false\n"), &hidden(true, "")),
            ["deletion-rationale.allow_hidden"]
        );
        assert!(keys(&hidden(false, "allow_hidden = false\n"), &hidden(false, "")).is_empty());
        assert!(keys(&hidden(true, "allow_hidden = true\n"), &hidden(true, "")).is_empty());
        // Every listed key is a real optional tolerance.
        for k in ABSENT_IS_UNLIMITED {
            assert_eq!(direction_of(k), Some(Direction::Tolerance), "{k}");
        }
    }

    fn cfg(body: &str) -> DisciplineConfig {
        DisciplineConfig::from_toml_str(&format!("[meta]\nversion = 1\nname = \"t\"\n{body}"))
            .unwrap()
    }

    #[test]
    fn a_command_key_added_over_its_preset_default_is_judged_as_changed_from_it() {
        let what = |base: &str, head: &str| -> Vec<String> {
            diff_configs(&cfg(base), &cfg(head))
                .unwrap()
                .iter()
                .map(|w| format!("{}: {}", w.gate, w.what()))
                .collect()
        };
        let table =
            |preset: &str, rest: &str| format!("[gates.command]\npreset = \"{preset}\"\n{rest}");
        let added = |preset: &str, rest: &str| what(&table(preset, ""), &table(preset, rest));
        let none: [&str; 0] = [];

        // A value other than the preset's replaces the check the preset made.
        assert_eq!(
            added("cargo-mutants", "zero_items_pattern = \"never\"\n"),
            [
                "command: `zero_items_pattern` changed from \"0 mutants tested\" to \"never\" \
              (the base value is its preset's default)"
            ]
        );
        assert_eq!(
            added("sanitizers", "canary_expected_diagnostic = \"ok\"\n"),
            ["command: `canary_expected_diagnostic` changed from \"ThreadSanitizer: data race\" \
              to \"ok\" (the base value is its preset's default)"]
        );
        assert_eq!(
            added("cargo-public-api", "snapshot = \"other.txt\"\n"),
            [
                "command: `snapshot` changed from \"public-api.txt\" to \"other.txt\" \
              (the base value is its preset's default)"
            ]
        );
        // The preset's own value written down is no change.
        assert_eq!(
            added(
                "cargo-mutants",
                "zero_items_pattern = \"0 mutants tested\"\n"
            ),
            none
        );
        assert_eq!(
            added("cargo-public-api", "snapshot = \"public-api.txt\"\n"),
            none
        );
        // A key the preset does not supply adds a check, as it does without a preset.
        assert_eq!(
            added("cargo-deny", "zero_items_pattern = \"never\"\n"),
            none
        );
        assert_eq!(added("cargo-mutants", "snapshot = \"out.txt\"\n"), none);
        assert_eq!(
            added(
                "cargo-mutants",
                "count_pattern = '(\\d+) mutants'\nmin_count = 3\n"
            ),
            none
        );
        assert_eq!(
            what(
                "[gates.command]\ncommand = \"true\"\n",
                "[gates.command]\ncommand = \"true\"\nzero_items_pattern = \"never\"\n"
            ),
            none
        );
        // The default is the base side's: a preset only the head names supplied nothing
        // to the base, and the `command` gate refuses the new preset itself.
        assert_eq!(
            what(
                "[gates.command]\n",
                &table("cargo-mutants", "zero_items_pattern = \"never\"\n")
            ),
            none
        );
        // Lists are added to the preset's, so growth is judged as on any table.
        assert_eq!(
            added("cargo-mutants", "forbid_output = [\"timeout\"]\n"),
            none
        );
        assert_eq!(
            added("cargo-public-api", "snapshot_ignore = [\"^//\"]\n"),
            ["command: `snapshot_ignore` gained 1 entr(y/ies)"]
        );

        // A `commands` entry: `snapshot` added is stricter unless the entry's preset
        // already supplied one.
        let entry = |preset: &str, rest: &str| {
            format!(
                "[gates.command]\n[[gates.command.commands]]\nname = \"api\"\n\
                 command = \"true\"\n{preset}{rest}"
            )
        };
        let api = "preset = \"cargo-public-api\"\n";
        assert_eq!(
            what(&entry(api, ""), &entry(api, "snapshot = \"other.txt\"\n")),
            ["command: `commands` lost 1 entr(y/ies)"]
        );
        assert_eq!(
            what(
                &entry(api, ""),
                &entry(api, "snapshot = \"public-api.txt\"\n")
            ),
            none
        );
        assert_eq!(
            what(&entry("", ""), &entry("", "snapshot = \"other.txt\"\n")),
            none
        );
        assert_eq!(
            what(
                &entry("preset = \"cargo-deny\"\n", ""),
                &entry("preset = \"cargo-deny\"\n", "snapshot = \"other.txt\"\n")
            ),
            none
        );
    }

    #[test]
    fn ratification_policy_loosening_is_a_weakening_and_tightening_is_not() {
        use crate::config::{AcceptEdited, RatificationWindow};
        let mut base = DisciplineConfig::default_for_repo("t");
        base.gates.ratified_paths.enabled = true;
        base.gates.ratified_paths.protected_paths = vec!["scripts/**".into()];
        base.gates.ratified_paths.ratifiers = vec!["owner".into()];
        base.gates.ratified_paths.ratification_max_age_days = Some(30);
        let weaker = |edit: &dyn Fn(&mut crate::config::RatifiedPathsGate)| {
            let mut head = base.clone();
            edit(&mut head.gates.ratified_paths);
            diff_configs(&base, &head).unwrap()
        };
        // The window is ordered: pull-created, path-last-changed, any.
        assert_eq!(
            weaker(&|g| g.ratification_valid_from = RatificationWindow::Any).len(),
            1
        );
        assert!(
            weaker(&|g| g.ratification_valid_from = RatificationWindow::PullCreated).is_empty()
        );
        assert_eq!(weaker(&|g| g.ratification_max_age_days = None).len(), 1);
        assert_eq!(weaker(&|g| g.ratification_max_age_days = Some(90)).len(), 1);
        assert!(weaker(&|g| g.ratification_max_age_days = Some(7)).is_empty());
        assert_eq!(weaker(&|g| g.ratifiers.push("helper".into())).len(), 1);
        assert_eq!(weaker(&|g| g.protected_paths.clear()).len(), 1);
        assert_eq!(
            weaker(&|g| g.never_ratifiable.pop().map(|_| ()).unwrap_or(())).len(),
            1
        );
        assert_eq!(
            weaker(&|g| g.accept_edited = AcceptEdited::ByAuthor).len(),
            1
        );
        assert_eq!(weaker(&|g| g.require_open_issue = false).len(), 1);
        assert!(weaker(&|g| g.agent_logins.push("bot".into())).is_empty());
        // Turning on `refuse_author_ratification` tightens; turning it off loosens.
        assert!(weaker(&|g| g.refuse_author_ratification = true).is_empty());
        let mut on = base.clone();
        on.gates.ratified_paths.refuse_author_ratification = true;
        assert_eq!(diff_configs(&on, &base).unwrap().len(), 1);
    }

    #[test]
    fn each_weakening_carries_its_key_change_and_values_as_data() {
        let base = cfg(
            "[directives]\nmax_overrides = 1\nallowed_override_actors = [\"lead\"]\n\
             [gates.pii]\nexempt_paths = [\"a/**\"]\n\
             [gates.test-floor]\nenabled = true\nmin_tests = 40\n",
        );
        let head = cfg(
            "[directives]\nallowed_override_actors = [\"lead\", \"someone\"]\n\
             [gates.pii]\nexempt_paths = [\"a/**\", \"b/**\", \"c/**\"]\nseverity = \"warning\"\n\
             [gates.test-floor]\nenabled = true\nmin_tests = 3\n",
        );
        let found = diff_configs(&base, &head).unwrap();
        let get = |gate: &str, key: &str| {
            found
                .iter()
                .find(|w| w.gate == gate && w.key == key)
                .unwrap_or_else(|| panic!("{gate}.{key} not in {found:?}"))
        };
        let removed = get("directives", "max_overrides");
        assert_eq!(
            (
                removed.change,
                removed.before.as_deref(),
                removed.after.as_deref()
            ),
            (Change::Removed, Some("1"), None)
        );
        assert_eq!(removed.what(), "`max_overrides` removed (was 1)");
        let actors = get("directives", "allowed_override_actors");
        assert_eq!((actors.change, actors.count), (Change::Gained, Some(1)));
        // A list entry is counted, never named: an actor is a login.
        let json = serde_json::to_string(&found).unwrap();
        assert!(!json.contains("someone"), "{json}");
        let paths = get("pii", "exempt_paths");
        assert_eq!((paths.change, paths.count), (Change::Gained, Some(2)));
        assert_eq!(paths.what(), "`exempt_paths` gained 2 entr(y/ies)");
        let sev = get("pii", "severity");
        assert_eq!(
            (sev.change, sev.before.as_deref(), sev.after.as_deref()),
            (Change::Lowered, Some("error"), Some("warning"))
        );
        let floor = get("test-floor", "min_tests");
        assert_eq!(
            (
                floor.change,
                floor.before.as_deref(),
                floor.after.as_deref()
            ),
            (Change::Decreased, Some("40"), Some("3"))
        );
        assert_eq!(floor.what(), "`min_tests` decreased from 40 to 3");
        assert!(json.contains(r#""change":"decreased""#), "{json}");
    }

    #[test]
    fn identical_and_stricter_configs_are_not_weakenings() {
        let base = cfg("[gates.pii]\nexempt_paths = [\"a/**\"]\n");
        assert!(diff_configs(&base, &base).unwrap().is_empty());
        let stricter = cfg("[gates.pii]\nhostname_denylist = [\"h\"]\n");
        assert!(diff_configs(&base, &stricter).unwrap().is_empty());
    }

    #[test]
    fn each_loosening_class_is_detected_and_attributed() {
        let base = cfg("[gates.pii]\nhostname_denylist = [\"h\"]\n");
        let head = cfg(
            "[gates.pii]\nlan_ips = false\nexempt_paths = [\"docs/**\"]\n\
             [gates.vacuous-tests]\nenabled = false\n\
             [gates.agent-scratch]\nseverity = \"warning\"\n",
        );
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        assert!(has("pii", "`lan_ips` changed from true to false"));
        assert!(has("pii", "`exempt_paths` gained 1"));
        assert!(has("pii", "`hostname_denylist` lost 1"));
        assert!(has("vacuous-tests", "`enabled` changed from true to false"));
        assert!(has("agent-scratch", "`severity` lowered"));
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn a_dropped_or_edited_banned_action_is_a_loosening_and_an_added_one_is_not() {
        let base = cfg(
            "[gates.ci-integrity]\nbanned_actions = [\"a/b\", { uses = \"c/d@v1\", reason = \"x\" }]\n",
        );
        let dropped = cfg("[gates.ci-integrity]\nbanned_actions = [\"a/b\"]\n");
        let narrowed = cfg(
            "[gates.ci-integrity]\nbanned_actions = [\"a/b@v1\", { uses = \"c/d@v1\", reason = \"x\" }]\n",
        );
        let added = cfg(
            "[gates.ci-integrity]\nbanned_actions = [\"a/b\", { uses = \"c/d@v1\", reason = \"x\" }, \"e/f\"]\n",
        );
        let lost = |head: &DisciplineConfig| {
            diff_configs(&base, head)
                .unwrap()
                .iter()
                .any(|w| w.gate == "ci-integrity" && w.what().contains("`banned_actions` lost"))
        };
        assert!(lost(&dropped));
        assert!(lost(&narrowed));
        assert!(diff_configs(&base, &added).unwrap().is_empty());
        assert!(diff_configs(&base, &base).unwrap().is_empty());
    }

    #[test]
    fn integer_and_security_boolean_loosening_detected() {
        let base = cfg("[gates.command]\nmin_count = 10\n[gates.unsafe-budget]\nmax_unsafe = 5\n[gates.pii]\ndiff_only = false\n");
        let head = cfg("[gates.command]\nmin_count = 5\n[gates.unsafe-budget]\nmax_unsafe = 10\n[gates.pii]\ndiff_only = true\n");
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        assert!(has("command", "`min_count` decreased from 10 to 5"));
        assert!(has("unsafe-budget", "`max_unsafe` increased from 5 to 10"));
        assert!(has("pii", "`diff_only` changed from false to true"));
    }

    #[test]
    fn the_measured_citation_keys_loosen_when_switched_off_narrowed_or_widened() {
        let table = |keys: &str| cfg(&format!("[gates.provenance-tags]\n{keys}"));
        let strict = "verify_measured_commit = true\nverify_cited_figures = true\n\
                      figure_tolerance_pct = 1.0\nrecord_paths = [\"results/**\", \"bench/**\"]\n";
        let said = |head: &str| -> Vec<String> {
            diff_configs(&table(strict), &table(head))
                .unwrap()
                .iter()
                .map(|w| w.what())
                .collect()
        };
        let has = |found: &[String], needle: &str| found.iter().any(|w| w.contains(needle));
        // Each key loosened on its own is the one thing reported.
        for (from, to, needle) in [
            (
                "verify_measured_commit = true",
                "verify_measured_commit = false",
                "`verify_measured_commit` changed from true to false",
            ),
            (
                "verify_cited_figures = true",
                "verify_cited_figures = false",
                "`verify_cited_figures` changed from true to false",
            ),
            (
                "figure_tolerance_pct = 1.0",
                "figure_tolerance_pct = 2.5",
                "`figure_tolerance_pct` increased from 1.0 to 2.5",
            ),
            (", \"bench/**\"", "", "`record_paths` lost 1"),
        ] {
            let found = said(&strict.replace(from, to));
            assert!(has(&found, needle), "{needle}: {found:?}");
            assert_eq!(found.len(), 1, "{found:?}");
        }
        // Removing a key returns it to its default: the three checks are off, and the
        // tolerance is back at zero, which is stricter.
        let found = said("");
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(!has(&found, "figure_tolerance_pct"), "{found:?}");
        // Another commit key reads another field of every record.
        let found = said(&format!("{strict}record_commit_key = \"rev\"\n"));
        assert!(has(&found, "`record_commit_key` changed"), "{found:?}");
        // Controls: unchanged, and tightened, are not reported.
        assert!(said(strict).is_empty());
        let tighter = strict
            .replace("1.0", "0.5")
            .replace(", \"bench/**\"", ", \"bench/**\", \"runs/**\"");
        assert!(said(&tighter).is_empty());
        // Switching the keys on from their defaults adds checks. A tolerance written in
        // the same change is above the default of zero, and is reported as raised.
        let on = strict.replace("figure_tolerance_pct = 1.0\n", "");
        assert!(diff_configs(&table(""), &table(&on)).unwrap().is_empty());
        let raised: Vec<String> = diff_configs(&table(""), &table(strict))
            .unwrap()
            .iter()
            .map(|w| w.what())
            .collect();
        assert_eq!(raised, ["`figure_tolerance_pct` increased from 0.0 to 1.0"]);
    }

    #[test]
    fn evidence_references_and_strict_modes_cannot_be_dropped_silently() {
        let base = cfg(
            "[gates.provenance-tags]\nsuperseded_registry = \"reg.json\"\n\
             superseded_json_paths = [\"docs/**/*.json\"]\nrequire_open_pending_issues = true\n\
             [gates.bench-regression]\nmode = \"paired-ratio\"\nratio_baseline = \"b.json\"\n\
             citation_source_paths = [\"src\"]\n",
        );
        let removed = cfg("[gates.bench-regression]\nratio_baseline = \"other.json\"\n");
        let found = diff_configs(&base, &removed).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        assert!(
            has("provenance-tags", "`superseded_registry` removed"),
            "{found:?}"
        );
        assert!(
            has("provenance-tags", "`superseded_json_paths` lost 1"),
            "{found:?}"
        );
        assert!(
            has(
                "provenance-tags",
                "`require_open_pending_issues` changed from true to false"
            ),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`mode` changed from paired-ratio"),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`ratio_baseline` changed from"),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`citation_source_paths` lost 1"),
            "{found:?}"
        );

        // Keys added with ci-skip-set and bench rigor.
        let skip_base = cfg("[gates.ci-skip-set]\nunconditional_jobs = [\"lint\"]\n\
             [gates.bench-regression]\nratio_tolerance_pct = 2.0\nexempt_arms = [\"a\"]\n");
        let skip_head = cfg(
            "[gates.ci-skip-set]\nunconditional_jobs = []\nchange_job = \"\"\nworkflow = \"x.yml\"\n\
             [gates.bench-regression]\nratio_tolerance_pct = 9.5\nexempt_arms = [\"a\", \"*\"]\n",
        );
        let skip_found = diff_configs(&skip_base, &skip_head).unwrap();
        let has = |gate: &str, needle: &str| {
            skip_found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        let found = &skip_found;
        assert!(
            has("ci-skip-set", "`unconditional_jobs` lost 1"),
            "{found:?}"
        );
        assert!(has("ci-skip-set", "`change_job` changed"), "{found:?}");
        assert!(has("ci-skip-set", "`workflow` changed"), "{found:?}");
        assert!(
            has("bench-regression", "`ratio_tolerance_pct` increased"),
            "{found:?}"
        );
        assert!(
            has("bench-regression", "`exempt_arms` gained 1"),
            "{found:?}"
        );

        // Adopting the registry or the stricter mode is not a weakening.
        let plain = cfg("");
        assert!(diff_configs(&plain, &base).unwrap().is_empty());
    }

    fn lockstep(group: &str, sources: &[(&str, &str)]) -> DisciplineConfig {
        let mut body = format!("[[gates.version-lockstep.groups]]\nname = \"{group}\"\n");
        for (path, regex) in sources {
            body.push_str(&format!(
                "[[gates.version-lockstep.groups.sources]]\npath = \"{path}\"\nregex = '{regex}'\n"
            ));
        }
        cfg(&body)
    }

    #[test]
    fn a_list_entry_is_matched_by_identity_and_only_a_tightening_edit_passes() {
        let base = lockstep(
            "release",
            &[("Cargo.toml", "v(.+)"), ("README.md", "@v(.+)")],
        );
        let whats = |head: &DisciplineConfig| -> Vec<String> {
            diff_configs(&base, head)
                .unwrap()
                .into_iter()
                .map(|w| format!("{}: {}", w.gate, w.what()))
                .collect()
        };
        let lost = vec!["version-lockstep: `groups` lost 1 entr(y/ies)".to_string()];
        // A source added to the same group, every base source kept: a tightening.
        let added = lockstep(
            "release",
            &[
                ("Cargo.toml", "v(.+)"),
                ("CITATION.cff", "version: (.+)"),
                ("README.md", "@v(.+)"),
            ],
        );
        assert!(whats(&added).is_empty(), "{:?}", whats(&added));
        // A source removed.
        assert_eq!(
            whats(&lockstep("release", &[("Cargo.toml", "v(.+)")])),
            lost
        );
        // A source's regex edited, even alongside an added source.
        let edited = lockstep(
            "release",
            &[
                ("Cargo.toml", "v(.+)"),
                ("README.md", "(.+)"),
                ("CITATION.cff", "version: (.+)"),
            ],
        );
        assert_eq!(whats(&edited), lost);
        // The group renamed: the base group is gone.
        let renamed = lockstep(
            "rel",
            &[
                ("Cargo.toml", "v(.+)"),
                ("README.md", "@v(.+)"),
                ("CITATION.cff", "version: (.+)"),
            ],
        );
        assert_eq!(whats(&renamed), lost);
        // Two head groups under the base name: which one kept the sources is ambiguous.
        // Two groups, one grows: each base group is found by its name, not its position.
        let mut two = base.clone();
        let mut docs = lockstep("docs", &[("a.md", "(.+)"), ("b.md", "(.+)")])
            .gates
            .version_lockstep
            .groups;
        two.gates.version_lockstep.groups.splice(0..0, docs.clone());
        docs.extend(added.gates.version_lockstep.groups.clone());
        let mut two_added = base.clone();
        two_added.gates.version_lockstep.groups = docs;
        assert!(diff_configs(&two, &two_added).unwrap().is_empty());
        let mut dup = added.clone();
        dup.gates
            .version_lockstep
            .groups
            .push(added.gates.version_lockstep.groups[0].clone());
        assert_eq!(whats(&dup), lost);
    }

    #[test]
    fn manifest_sync_rules_and_command_entries_follow_the_same_identity_rule() {
        let rules = |watched: &str, exclude: &str, regex: &str| {
            cfg(&format!(
                "[[gates.manifest-sync.rules]]\nmanifest = \"m.json\"\nextract_regex = '{regex}'\n\
                 watched_paths = [{watched}]\nexclude_paths = [{exclude}]\n"
            ))
        };
        let base = rules("\"src/**\"", "\"src/gen/**\", \"src/x/**\"", "n(.+)");
        let count = |head: &DisciplineConfig| diff_configs(&base, head).unwrap().len();
        // A watched path added and an exclusion dropped: both tighten.
        assert_eq!(
            count(&rules("\"src/**\", \"lib/**\"", "\"src/gen/**\"", "n(.+)")),
            0
        );
        // A watched path dropped, an exclusion added, the regex edited: each loosens.
        assert_eq!(
            count(&rules("", "\"src/gen/**\", \"src/x/**\"", "n(.+)")),
            1
        );
        assert_eq!(
            count(&rules(
                "\"src/**\"",
                "\"src/gen/**\", \"src/x/**\", \"a/**\"",
                "n(.+)"
            )),
            1
        );
        assert_eq!(
            count(&rules("\"src/**\"", "\"src/gen/**\", \"src/x/**\"", "(.+)")),
            1
        );

        let cmd = |forbid: &str, min: u64| {
            cfg(&format!(
                "[[gates.command.commands]]\nname = \"t\"\ncommand = \"cargo test\"\n\
                 min_count = {min}\nforbid_output = [{forbid}]\n"
            ))
        };
        let base = cmd("\"panicked\"", 5);
        let count = |head: &DisciplineConfig| diff_configs(&base, head).unwrap().len();
        assert_eq!(count(&cmd("\"panicked\", \"ignored\"", 5)), 0);
        assert_eq!(count(&cmd("", 5)), 1);
        // A field outside the list is judged whole: even a raised floor reads as lost.
        assert_eq!(count(&cmd("\"panicked\"", 9)), 1);

        // A snapshot added to an entry tightens; repointed or removed, it loosens. Fewer
        // ignored lines tighten; one more, or an edited pattern, loosens, even when it
        // arrives with the snapshot (a preset can supply the snapshot itself).
        let snap = |extra: &str| {
            cfg(&format!(
                "[[gates.command.commands]]\nname = \"t\"\ncommand = \"cargo test\"\n{extra}"
            ))
        };
        let plain = snap("");
        let with = snap("snapshot = \"api.txt\"\nsnapshot_ignore = ['^#', '^//']\n");
        let count = |base: &DisciplineConfig, head: &DisciplineConfig| {
            diff_configs(base, head).unwrap().len()
        };
        assert_eq!(count(&plain, &snap("snapshot = \"api.txt\"\n")), 0);
        assert_eq!(count(&plain, &with), 1);
        assert_eq!(count(&with, &plain), 1);
        assert_eq!(
            count(
                &with,
                &snap("snapshot = \"other.txt\"\nsnapshot_ignore = ['^#', '^//']\n")
            ),
            1
        );
        assert_eq!(
            count(
                &with,
                &snap("snapshot = \"api.txt\"\nsnapshot_ignore = ['^#']\n")
            ),
            0
        );
        assert_eq!(
            count(
                &with,
                &snap("snapshot = \"api.txt\"\nsnapshot_ignore = ['^#', '^//', '.*']\n")
            ),
            1
        );
        assert_eq!(
            count(
                &with,
                &snap("snapshot = \"api.txt\"\nsnapshot_ignore = ['^#', '.*']\n")
            ),
            1
        );
    }

    #[test]
    fn command_table_snapshot_keys_are_directional() {
        let top = |extra: &str| cfg(&format!("[gates.command]\ncommand = \"x\"\n{extra}"));
        let keys = |base: &str, head: &str| -> Vec<String> {
            diff_configs(&top(base), &top(head))
                .unwrap()
                .iter()
                .map(|w| w.key.clone())
                .collect()
        };
        let with = "snapshot = \"api.txt\"\nsnapshot_ignore = ['^#']\n";
        assert!(keys("", "snapshot = \"api.txt\"\n").is_empty());
        assert_eq!(keys("", with), ["snapshot_ignore"]);
        assert_eq!(keys(with, "snapshot_ignore = ['^#']\n"), ["snapshot"]);
        assert_eq!(
            keys(with, "snapshot = \"b.txt\"\nsnapshot_ignore = ['^#']\n"),
            ["snapshot"]
        );
        assert_eq!(
            keys(with, "snapshot = \"api.txt\"\nsnapshot_ignore = ['.*']\n"),
            ["snapshot_ignore"]
        );
        assert!(keys(with, "snapshot = \"api.txt\"\n").is_empty());
    }

    #[test]
    fn every_directives_option_is_judged() {
        // `diff_configs` reads [directives] field by field; a new option must be added
        // there and here.
        const JUDGED: &[&str] = &[
            "sources",
            "allow_hidden",
            "fail_on_overrides",
            "allowed_override_actors",
            "max_overrides",
            "max_inline_overrides",
            "require_approval",
            "degrade_offline",
        ];
        let schema = crate::schema::generate_schema();
        let props = schema["properties"]["directives"]["properties"]
            .as_object()
            .unwrap();
        let unjudged: Vec<_> = props
            .keys()
            .filter(|k| !JUDGED.contains(&k.as_str()))
            .collect();
        assert!(
            unjudged.is_empty(),
            "judge these in diff_configs: {unjudged:?}"
        );
        assert_eq!(props.len(), JUDGED.len());
    }

    #[test]
    fn a_raised_or_removed_override_budget_and_dropped_approval_are_weakenings() {
        let cfg = |d: &str| {
            DisciplineConfig::from_toml_str(&format!(
                "[meta]\nversion = 1\nname = \"t\"\n[directives]\n{d}"
            ))
            .unwrap()
        };
        let base = cfg("max_overrides = 1\nmax_inline_overrides = 2\nrequire_approval = true\n");
        let whats = |head: &DisciplineConfig| -> Vec<String> {
            diff_configs(&base, head)
                .unwrap()
                .into_iter()
                .map(|w| w.what())
                .collect()
        };
        assert_eq!(
            whats(&cfg("max_overrides = 4\nmax_inline_overrides = 5\n")),
            vec![
                "`max_overrides` increased from 1 to 4",
                "`max_inline_overrides` increased from 2 to 5",
                "`require_approval` changed from true to false"
            ]
        );
        assert_eq!(
            whats(&cfg("require_approval = true\n")),
            vec![
                "`max_overrides` removed (was 1)",
                "`max_inline_overrides` removed (was 2)"
            ]
        );
        assert!(whats(&cfg(
            "max_overrides = 0\nmax_inline_overrides = 1\nrequire_approval = true\n"
        ))
        .is_empty());
        // Switching the failed-lookup degrade on is a loosening; off is a tightening.
        let off = cfg("degrade_offline = false\n");
        let on = cfg("degrade_offline = true\n");
        let notes: Vec<String> = diff_configs(&off, &on)
            .unwrap()
            .into_iter()
            .map(|w| w.what())
            .collect();
        assert!(
            notes
                .iter()
                .any(|w| w.contains("`degrade_offline` changed from false to true")),
            "{notes:?}"
        );
        assert!(diff_configs(&on, &off).unwrap().is_empty());
        // Adopting either is a tightening.
        assert!(diff_configs(&cfg(""), &base).unwrap().is_empty());
    }

    #[test]
    fn advisory_mode_and_override_actors_are_weakenings() {
        let base = DisciplineConfig::from_toml_str(
            "[meta]\nversion = 1\nname = \"t\"\n[directives]\nallowed_override_actors = [\"lead\"]\n",
        )
        .unwrap();
        let head = DisciplineConfig::from_toml_str(
            "[meta]\nversion = 1\nname = \"t\"\nmode = \"advisory\"\n\
             [directives]\nallowed_override_actors = [\"lead\", \"bot\"]\n",
        )
        .unwrap();
        let found = diff_configs(&base, &head).unwrap();
        assert_eq!(
            found
                .iter()
                .map(|w| (w.gate.as_str(), w.what()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "directives",
                    "`allowed_override_actors` gained 1 entr(y/ies)".to_string()
                ),
                (
                    "meta",
                    "`mode` changed from enforcing to advisory".to_string()
                ),
            ]
        );
        // Leaving advisory mode, or dropping an actor, tightens.
        assert!(diff_configs(&head, &base).unwrap().is_empty());
    }

    #[test]
    fn floors_caps_allowlists_and_commands_are_directional() {
        let base = cfg(
            "[gates.test-floor]\nmin_tests = 40\ntolerance = 0\ntest_command = \"cargo test\"\n\
             [gates.suppression-delta]\nmax_increase = 0\nallowed_suppressions = []\n\
             [gates.dependency-delta]\nallow_dependencies = [\"serde\"]\ndeny_dependencies = [\"openssl\"]\n\
             [gates.scope-confinement]\nforbidden_paths = [\"ci/**\"]\n\
             [gates.ci-integrity]\nworkflows = [\".github/workflows/*.yml\"]\n",
        );
        let head = cfg(
            "[gates.test-floor]\ntolerance = 5\ntest_command = \"true\"\n\
             [gates.suppression-delta]\nmax_increase = 9\nallowed_suppressions = [\"noqa\"]\n\
             [gates.dependency-delta]\nallow_dependencies = []\ndeny_dependencies = []\n\
             [gates.scope-confinement]\nforbidden_paths = []\n\
             [gates.ci-integrity]\nworkflows = []\n",
        );
        let found = diff_configs(&base, &head).unwrap();
        let has = |gate: &str, needle: &str| {
            found
                .iter()
                .any(|w| w.gate == gate && w.what().contains(needle))
        };
        assert!(has("test-floor", "`min_tests` removed"), "{found:?}");
        assert!(
            has("test-floor", "`tolerance` increased from 0 to 5"),
            "{found:?}"
        );
        assert!(has("test-floor", "`test_command` changed"), "{found:?}");
        assert!(
            has("suppression-delta", "`max_increase` increased"),
            "{found:?}"
        );
        assert!(
            has("suppression-delta", "`allowed_suppressions` gained 1"),
            "{found:?}"
        );
        assert!(
            has("dependency-delta", "`allow_dependencies` emptied"),
            "{found:?}"
        );
        assert!(
            has("dependency-delta", "`deny_dependencies` lost 1"),
            "{found:?}"
        );
        assert!(
            has("scope-confinement", "`forbidden_paths` lost 1"),
            "{found:?}"
        );
        assert!(has("ci-integrity", "`workflows` lost 1"), "{found:?}");
        assert_eq!(found.len(), 9, "{found:?}");

        // The reverse direction tightens everything except the repointed command: which
        // command is the stronger check is not something a diff can decide.
        let back = diff_configs(&head, &base).unwrap();
        assert_eq!(back.len(), 1, "{back:?}");
        assert!(
            back[0].what().contains("`test_command` changed"),
            "{back:?}"
        );
        // Adopting an allow-list where none existed is a tightening; growing one is not.
        let none = cfg("");
        let adopted = cfg("[gates.dependency-delta]\nallow_dependencies = [\"serde\"]\n");
        let grown =
            cfg("[gates.dependency-delta]\nallow_dependencies = [\"serde\", \"left-pad\"]\n");
        assert!(diff_configs(&none, &adopted).unwrap().is_empty());
        assert_eq!(diff_configs(&adopted, &grown).unwrap().len(), 1);
    }

    /// An optional key is absent from the compared table when unset, so a key only on one
    /// side is judged by what its absence means (#520).
    #[test]
    fn an_optional_key_removed_or_added_is_judged_by_what_its_absence_means() {
        let one = |base: &str, head: &str| {
            let found = diff_configs(&cfg(base), &cfg(head)).unwrap();
            assert_eq!(found.len(), 1, "{base:?} -> {head:?}: {found:?}");
            found.into_iter().next().unwrap()
        };
        let none = |base: &str, head: &str| {
            let found = diff_configs(&cfg(base), &cfg(head)).unwrap();
            assert!(found.is_empty(), "{base:?} -> {head:?}: {found:?}");
        };

        // `ci_skip_severity` removed: the gate's `severity` applies again.
        let w = one(
            "[gates.ignored-tests]\nseverity = \"warning\"\nci_skip_severity = \"error\"\n",
            "[gates.ignored-tests]\nseverity = \"warning\"\n",
        );
        assert_eq!(
            (w.gate.as_str(), w.key(), w.change),
            ("ignored-tests", "ci_skip_severity", Change::Lowered)
        );
        assert_eq!(
            (w.before.as_deref(), w.after.as_deref()),
            (Some("error"), Some("warning"))
        );
        // Removed where it equalled or sat below the gate severity: nothing loosens.
        none(
            "[gates.ignored-tests]\nseverity = \"error\"\nci_skip_severity = \"error\"\n",
            "[gates.ignored-tests]\nseverity = \"error\"\n",
        );
        none(
            "[gates.ignored-tests]\nci_skip_severity = \"warning\"\n",
            "",
        );

        // A tolerance whose absence means none: adding one above zero widens it.
        for key in ["noise_margin_pct", "ratio_tolerance_pct"] {
            let w = one("", &format!("[gates.bench-regression]\n{key} = 50.0\n"));
            assert_eq!(
                (w.gate.as_str(), w.key(), w.change),
                ("bench-regression", key, Change::Increased)
            );
            none("", &format!("[gates.bench-regression]\n{key} = 0.0\n"));
        }
        // `max_noise_cv` absent means no noise check: adding it only tightens.
        none("", "[gates.bench-regression]\nmax_noise_cv = 0.05\n");

        // A gate's own `allow_hidden = true` over a global `false`.
        let w = one(
            "[directives]\nallow_hidden = false\n",
            "[directives]\nallow_hidden = false\n[gates.deletion-rationale]\nallow_hidden = true\n",
        );
        assert_eq!(
            (w.gate.as_str(), w.key(), w.change),
            ("deletion-rationale", "allow_hidden", Change::Changed)
        );
        // Over a global `true` the gate already accepted hidden directives.
        none(
            "[directives]\nallow_hidden = true\n",
            "[directives]\nallow_hidden = true\n[gates.deletion-rationale]\nallow_hidden = true\n",
        );
        none(
            "[directives]\nallow_hidden = true\n",
            "[directives]\nallow_hidden = true\n[gates.deletion-rationale]\nallow_hidden = false\n",
        );

        // The test-identity report: removing or repointing it replaces the check.
        for key in ["test_report", "base_report", "head_report"] {
            let w = one(
                &format!("[gates.test-floor]\n{key} = \"reports/junit.xml\"\n"),
                "",
            );
            assert_eq!(
                (w.gate.as_str(), w.key(), w.change),
                ("test-floor", key, Change::Removed)
            );
            let w = one(
                &format!("[gates.test-floor]\n{key} = \"reports/junit.xml\"\n"),
                &format!("[gates.test-floor]\n{key} = \"other.xml\"\n"),
            );
            assert_eq!((w.key(), w.change), (key, Change::Changed));
            let adopted = format!("[gates.test-floor]\n{key} = \"reports/junit.xml\"\n");
            if key == "base_report" {
                // What the head report is compared with: adopting it adds a check.
                none("", &adopted);
            } else {
                // What the count is read from: adopting it replaces the counting basis.
                let w = one("", &adopted);
                assert_eq!(
                    (w.gate.as_str(), w.key(), w.change, w.before.as_deref()),
                    ("test-floor", key, Change::Changed, Some("unset"))
                );
            }
        }
    }

    #[test]
    fn ci_skip_severity_added_on_head_is_detected_as_weakening() {
        // a. base "" , head "[gates.ignored-tests]\nci_skip_severity = \"note\"" -> 1 weakening naming ci_skip_severity
        let found_a = diff_configs(
            &cfg(""),
            &cfg("[gates.ignored-tests]\nci_skip_severity = \"note\"\n"),
        )
        .unwrap();
        assert_eq!(found_a.len(), 1);
        assert_eq!(found_a[0].gate, "ignored-tests");
        assert_eq!(found_a[0].key(), "ci_skip_severity");
        assert_eq!(found_a[0].change, Change::Lowered);
        assert_eq!(found_a[0].before.as_deref(), Some("error"));
        assert_eq!(found_a[0].after.as_deref(), Some("note"));

        // b. base ci_skip_severity="error", head "note" -> 1
        let found_b = diff_configs(
            &cfg("[gates.ignored-tests]\nci_skip_severity = \"error\"\n"),
            &cfg("[gates.ignored-tests]\nci_skip_severity = \"note\"\n"),
        )
        .unwrap();
        assert_eq!(found_b.len(), 1);
        assert_eq!(found_b[0].gate, "ignored-tests");
        assert_eq!(found_b[0].key(), "ci_skip_severity");

        // c. control: base "", head severity="note" -> 1
        let found_c = diff_configs(
            &cfg(""),
            &cfg("[gates.ignored-tests]\nseverity = \"note\"\n"),
        )
        .unwrap();
        assert_eq!(found_c.len(), 1);
        assert_eq!(found_c[0].gate, "ignored-tests");
        assert_eq!(found_c[0].key(), "severity");

        // d. base severity="warning", head severity="warning" + ci_skip_severity="error" -> 0
        let found_d = diff_configs(
            &cfg("[gates.ignored-tests]\nseverity = \"warning\"\n"),
            &cfg("[gates.ignored-tests]\nseverity = \"warning\"\nci_skip_severity = \"error\"\n"),
        )
        .unwrap();
        assert!(found_d.is_empty(), "expected 0 weakenings, got {found_d:?}");
    }

    // ---- #592: evidence keys judged by their effective value ----

    const ISSUE_LINK_ON: &str = "[gates.issue-link]\nenabled = true\n";
    const MUTANTS: &str = "[gates.command]\npreset = \"cargo-mutants\"\n";
    const SANITIZERS: &str = "[gates.command]\npreset = \"sanitizers\"\n";
    const NEVER: &str = "zero_items_pattern = \"never\"\n";
    const MUTANTS_GUARD: &str = "zero_items_pattern = \"0 mutants tested\"\n";
    const MUTANTS_ENTRY: &str =
        "[gates.command]\n[[gates.command.commands]]\nname = \"mut\"\npreset = \"cargo-mutants\"\n";

    fn said(base: &str, head: &str, authorised: bool) -> Vec<String> {
        diff_configs_under(&cfg(base), &cfg(head), authorised)
            .unwrap()
            .iter()
            .map(|w| format!("{}: {}", w.gate, w.what()))
            .collect()
    }

    fn builtin_pattern_line() -> String {
        format!(
            "pattern = '{}'\n",
            crate::guards::issue_link::DEFAULT_ISSUE_PATTERN
        )
    }

    #[test]
    fn an_issue_pattern_added_over_the_builtin_default_is_judged_as_changed_from_it() {
        let found = said(
            ISSUE_LINK_ON,
            &format!("{ISSUE_LINK_ON}pattern = \".\"\n"),
            false,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("issue-link: `pattern` changed from ")
                && found[0].ends_with("to \".\" (the base value is the built-in default)"),
            "{found:?}"
        );
        // The built-in pattern written down, or removed again, changes nothing in force.
        let written = format!("{ISSUE_LINK_ON}{}", builtin_pattern_line());
        assert!(said(ISSUE_LINK_ON, &written, false).is_empty());
        assert!(said(&written, ISSUE_LINK_ON, false).is_empty());
        // Removing any other pattern is still a removal.
        assert_eq!(
            said(
                &format!("{ISSUE_LINK_ON}pattern = \".\"\n"),
                ISSUE_LINK_ON,
                false
            ),
            ["issue-link: `pattern` removed (was \".\")"]
        );
        // `pattern` has that default in `issue-link` only: another gate's `pattern` is
        // not compared with it.
        assert!(effective_default("ci-integrity", &toml::Table::new(), "pattern").is_none());
    }

    #[test]
    fn a_command_or_canary_added_over_its_preset_default_is_judged_as_changed_from_it() {
        assert_eq!(
            said(MUTANTS, &format!("{MUTANTS}command = \"true\"\n"), false),
            [
                "command: `command` changed from \"cargo mutants --in-diff\" to \"true\" \
              (the base value is its preset's default)"
            ]
        );
        assert_eq!(
            said(
                SANITIZERS,
                &format!("{SANITIZERS}canary_command = \"false\"\n"),
                false
            ),
            [
                "command: `canary_command` changed from \"cargo test --test race_canary\" to \
              \"false\" (the base value is its preset's default)"
            ]
        );
        // Written down, and removed again.
        let written = format!("{MUTANTS}command = \"cargo mutants --in-diff\"\n");
        assert!(said(MUTANTS, &written, false).is_empty());
        assert!(said(&written, MUTANTS, false).is_empty());
        // A preset with no canary: unset means no canary, and adding one adds a check.
        assert!(said(
            MUTANTS,
            &format!("{MUTANTS}canary_command = \"false\"\n"),
            false
        )
        .is_empty());
        // No preset: unset means no command.
        assert!(said(
            "[gates.command]\n",
            "[gates.command]\ncommand = \"true\"\n",
            false
        )
        .is_empty());
    }

    #[test]
    fn removing_a_key_equal_to_its_effective_default_is_not_a_weakening() {
        assert!(said(&format!("{MUTANTS}{MUTANTS_GUARD}"), MUTANTS, false).is_empty());
        assert_eq!(
            said(&format!("{MUTANTS}{NEVER}"), MUTANTS, false),
            ["command: `zero_items_pattern` removed (was \"never\")"]
        );
        // With no preset on the head side the removed value has no default to equal.
        assert_eq!(
            said(
                &format!("[gates.command]\ncommand = \"true\"\n{MUTANTS_GUARD}"),
                "[gates.command]\ncommand = \"true\"\n",
                false
            ),
            ["command: `zero_items_pattern` removed (was \"0 mutants tested\")"]
        );
        // An entry is judged as a whole: the default removed or written down is no edit,
        // any other value is.
        let with = |rest: &str| format!("{MUTANTS_ENTRY}{rest}");
        assert!(said(&with(MUTANTS_GUARD), &with(""), false).is_empty());
        assert!(said(&with(""), &with(MUTANTS_GUARD), false).is_empty());
        assert_eq!(
            said(&with(NEVER), &with(""), false),
            ["command: `commands` lost 1 entr(y/ies)"]
        );
        assert_eq!(
            said(&with(""), &with(NEVER), false),
            ["command: `commands` lost 1 entr(y/ies)"]
        );
    }

    #[test]
    fn guards_beside_a_preset_the_head_first_names_are_judged_under_authorisation() {
        let base = "[gates.command]\ncommand = \"true\"\n";
        let head = |rest: &str| format!("{base}preset = \"cargo-mutants\"\n{rest}");
        let expected =
            "command: `zero_items_pattern` changed from \"0 mutants tested\" to \"never\" \
             (compared with the default of the preset this change names)";
        assert_eq!(said(base, &head(NEVER), true), [expected]);
        // Unauthorised, the `command` gate refuses the new preset and nothing runs.
        assert!(said(base, &head(NEVER), false).is_empty());
        // The preset alone, or its guard written down.
        assert!(said(base, &head(""), true).is_empty());
        assert!(said(base, &head(MUTANTS_GUARD), true).is_empty());
        // A key the new preset has no guard for adds a check.
        assert!(said(base, &head("snapshot = \"out.txt\"\n"), true).is_empty());
        // The command written beside the new preset is what the runner authorised.
        assert!(said(
            "[gates.command]\n",
            "[gates.command]\npreset = \"cargo-mutants\"\ncommand = \"true\"\n",
            true
        )
        .is_empty());
        // A preset both sides name is the base's: judged the same either way.
        for authorised in [false, true] {
            assert_eq!(
                said(MUTANTS, &format!("{MUTANTS}{NEVER}"), authorised).len(),
                1
            );
        }

        // An entry only the head has.
        let entry = |rest: &str| format!("{MUTANTS_ENTRY}{rest}");
        assert_eq!(
            said("[gates.command]\n", &entry(NEVER), true),
            [
                "command: `commands[mut].zero_items_pattern` changed from \"0 mutants tested\" \
              to \"never\" (compared with the default of the preset this change names)"
            ]
        );
        assert!(said("[gates.command]\n", &entry(NEVER), false).is_empty());
        assert!(said("[gates.command]\n", &entry(""), true).is_empty());
        assert!(said("[gates.command]\n", &entry(MUTANTS_GUARD), true).is_empty());
        // An entry both sides have is matched by name and judged as an edited entry.
        assert_eq!(
            said(&entry(""), &entry(NEVER), true),
            ["command: `commands` lost 1 entr(y/ies)"]
        );
    }

    /// What leaving an optional evidence key unset means to the gate that reads it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Unset {
        /// The check the key names is not made, so adding the key adds a check.
        NoCheck,
        /// A value stands in (`effective_default`): adding the key replaces it.
        Default,
        /// The gate cannot run without it (exit 2), so there is no unset behaviour.
        Required,
        /// Another way of counting is used; there is no value the head could write down.
        OtherBasis,
    }

    /// Every evidence key a gate table can leave out, and what leaving it out means.
    /// A key filled by the gate's `Default` (`deny_file`, `rollup_job`, `marker`,
    /// `review_trailer`, `workflow`, `change_job`, `sanitizer`, `closing_keywords`) is on
    /// both sides of every comparison and is not optional in this sense.
    const OPTIONAL_EVIDENCE_KEYS: &[(&str, &str, Unset)] = &[
        ("archive-contents", "archive_path", Unset::Required),
        ("archive-contents", "preset", Unset::NoCheck),
        ("bench-regression", "ratio_baseline", Unset::Required),
        ("ci-integrity", "documented_job_count_path", Unset::NoCheck),
        (
            "ci-integrity",
            "documented_job_count_pattern",
            Unset::NoCheck,
        ),
        ("command", "canary_command", Unset::Default),
        ("command", "canary_expected_diagnostic", Unset::Default),
        ("command", "command", Unset::Default),
        ("command", "count_pattern", Unset::NoCheck),
        ("command", "preset", Unset::NoCheck),
        ("command", "snapshot", Unset::Default),
        ("command", "zero_items_pattern", Unset::Default),
        ("issue-link", "pattern", Unset::Default),
        ("msrv", "command", Unset::NoCheck),
        ("provenance-tags", "superseded_registry", Unset::NoCheck),
        ("test-floor", "base_report", Unset::OtherBasis),
        ("test-floor", "constant_file", Unset::NoCheck),
        ("test-floor", "constant_name", Unset::NoCheck),
        ("test-floor", "head_report", Unset::OtherBasis),
        ("test-floor", "test_command", Unset::OtherBasis),
        ("test-floor", "test_report", Unset::OtherBasis),
    ];

    #[test]
    fn optional_evidence_keys_are_inventoried() {
        let schema = crate::schema::generate_schema();
        let gates_schema = schema["properties"]["gates"]["properties"]
            .as_object()
            .unwrap();
        let defaults = Value::try_from(DisciplineConfig::default_for_repo("t").gates).unwrap();
        let mut optional = Vec::new();
        for (gate, table) in defaults.as_table().unwrap() {
            let def_ref = gates_schema[gate]["allOf"][0]["$ref"].as_str().unwrap();
            let def_name = def_ref.rsplit('/').next().unwrap();
            let props = schema["$defs"][def_name]["properties"].as_object().unwrap();
            for key in props.keys() {
                if direction_of(key) == Some(Direction::Evidence)
                    && !table.as_table().unwrap().contains_key(key)
                {
                    optional.push((gate.clone(), key.clone()));
                }
            }
        }
        optional.sort();
        let listed: Vec<(String, String)> = OPTIONAL_EVIDENCE_KEYS
            .iter()
            .map(|(g, k, _)| (g.to_string(), k.to_string()))
            .collect();
        assert_eq!(
            optional, listed,
            "an optional evidence key is new or gone: say in OPTIONAL_EVIDENCE_KEYS what unset means, and give it an `effective_default` if a value stands in"
        );
        // A key has an effective default exactly when the inventory says a value stands in.
        // `command` keys take theirs from a preset; `sanitizers` and `cargo-public-api`
        // between them supply all five.
        let tables: Vec<toml::Table> = ["sanitizers", "cargo-public-api", "cargo-mutants"]
            .iter()
            .map(|p| {
                let mut t = toml::Table::new();
                t.insert("preset".to_string(), Value::String(p.to_string()));
                t
            })
            .collect();
        for (gate, key, unset) in OPTIONAL_EVIDENCE_KEYS {
            let has = tables
                .iter()
                .any(|t| effective_default(gate, t, key).is_some());
            assert_eq!(has, *unset == Unset::Default, "{gate}.{key}");
        }
    }

    #[test]
    fn versions_are_ordered_by_their_numbers_not_their_text() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        for (base, head, want) in [
            ("1.90", "1.80", Some(Less)),
            ("1.80", "1.90", Some(Greater)),
            // Lower as a version, higher as text; and the reverse.
            ("1.10", "1.9", Some(Less)),
            ("1.9", "1.10", Some(Greater)),
            ("1.90", "1.90.0", Some(Equal)),
            ("1.90.0", "1.90", Some(Equal)),
            ("1.90.1", "1.90", Some(Less)),
            ("1.90", "1.90.1", Some(Greater)),
            ("2", "1.99", Some(Less)),
            // Not dotted numbers: no order.
            ("1.90", "stable", None),
            ("nightly", "1.90", None),
            ("1.90", "1.90-beta", None),
            ("1.90", "1..90", None),
            ("1.90", "", None),
            ("1.90", "-1.90", None),
            ("1.90", "+1", None),
        ] {
            assert_eq!(version_order(base, head), want, "{base} -> {head}");
        }
    }

    #[test]
    fn a_pinned_version_is_a_floor_compared_as_a_version() {
        let pinned = |v: &str| format!("[gates.msrv]\npinned_version = \"{v}\"\n");
        let said = |base: &str, head: &str| -> Vec<String> {
            diff_configs(&cfg(base), &cfg(head))
                .unwrap()
                .iter()
                .map(|w| format!("{}: {}", w.gate, w.what()))
                .collect()
        };
        assert_eq!(
            said(&pinned("1.90"), &pinned("1.80")),
            ["msrv: `pinned_version` decreased from \"1.90\" to \"1.80\""]
        );
        assert_eq!(
            said(&pinned("1.10"), &pinned("1.9")),
            ["msrv: `pinned_version` decreased from \"1.10\" to \"1.9\""]
        );
        assert_eq!(
            said(&pinned("1.90"), ""),
            ["msrv: `pinned_version` removed (was \"1.90\")"]
        );
        // A side that is not a version cannot be shown to be no lower.
        let unordered = said(&pinned("1.90"), &pinned("stable"));
        assert_eq!(unordered.len(), 1, "{unordered:?}");
        assert!(
            unordered[0]
                .starts_with("msrv: `pinned_version` changed from \"1.90\" to \"stable\" (")
                && unordered[0].contains("not a dotted-number version"),
            "{unordered:?}"
        );
        // Raised, equal under another spelling, unchanged, or added: nothing loosens.
        for (base, head) in [
            (pinned("1.80"), pinned("1.90")),
            (pinned("1.9"), pinned("1.10")),
            (pinned("1.90"), pinned("1.90.0")),
            (pinned("stable"), pinned("stable")),
            (String::new(), pinned("1.50")),
        ] {
            assert!(said(&base, &head).is_empty(), "{base} -> {head}");
        }
        assert_eq!(
            direction_of("pinned_version"),
            Some(Direction::VersionFloor)
        );
    }

    #[test]
    fn a_counting_basis_added_where_the_base_had_none_is_a_change_of_evidence() {
        let said = |base: &str, head: &str| -> Vec<String> {
            diff_configs(&cfg(base), &cfg(head))
                .unwrap()
                .iter()
                .map(|w| format!("{}: {}", w.gate, w.what()))
                .collect()
        };
        for (key, value) in [
            ("test_report", "reports/junit.xml"),
            ("head_report", "reports/head.xml"),
            ("test_command", "cargo test -- --list"),
        ] {
            let with = format!("[gates.test-floor]\nmin_tests = 4\n{key} = \"{value}\"\n");
            let found = said("[gates.test-floor]\nmin_tests = 4\n", &with);
            assert_eq!(found.len(), 1, "{found:?}");
            assert!(
                found[0].starts_with(&format!(
                    "test-floor: `{key}` changed from unset to \"{value}\" ("
                )) && found[0].contains("a basis the base ref did not use"),
                "{found:?}"
            );
            // Unchanged on both sides, and removed (already judged).
            assert!(said(&with, &with).is_empty());
            assert_eq!(
                said(&with, "[gates.test-floor]\nmin_tests = 4\n"),
                [format!("test-floor: `{key}` removed (was \"{value}\")")]
            );
        }
        // Controls: an optional evidence key whose absence means no check adds one, and
        // `base_report` names what the head report is compared with, not what is counted.
        assert!(said(
            "",
            "[gates.test-floor]\nconstant_file = \"a.rs\"\nconstant_name = \"N\"\n"
        )
        .is_empty());
        let reports = "[gates.test-floor]\nhead_report = \"h.xml\"\n";
        assert!(said(reports, &format!("{reports}base_report = \"b.xml\"\n")).is_empty());
        // Every key judged this way is inventoried as counting on another basis.
        for pair in ADDED_IS_ANOTHER_BASIS {
            assert!(
                OPTIONAL_EVIDENCE_KEYS
                    .iter()
                    .any(|(g, k, u)| (*g, *k) == *pair && *u == Unset::OtherBasis),
                "{pair:?}"
            );
        }
    }
}
