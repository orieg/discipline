//! The gate loop `toolchain-config` and `sandbox-config` run, and what each hands it
//! ([`TreeGate`]).

use super::{diff_trees, load, load_or_empty, Classified};
use crate::config::{BasicGate, GateSettings, Severity};
use crate::findings::FindingKind;
use crate::gitctx::{ChangeKind, ChangedFile};
use crate::guards::{Context, GateOutcome, PathFilter};
use anyhow::Result;
use serde_json::Value;

/// What a data file missing on one side means.
pub enum AbsentSide {
    /// It is compared as an empty file, so a file that appears or disappears is judged
    /// key by key.
    Empty,
    /// It is not compared: a file that appears is passed, and one that disappears is
    /// reported as `deleted`.
    NotCompared { deleted: &'static FindingKind },
}

/// Which side of a data file is read first. It decides which error is returned when
/// neither side can be read.
pub enum ReadOrder {
    HeadThenBase,
    BaseThenHead,
}

/// `(key, what was gained)` for the keys through which a file (by name) inherits another
/// one, between its base and head trees.
pub type Inherited = fn(&str, &Value, &Value) -> Vec<(String, String)>;

/// What to do with a changed file the gate's `classify` does not know, at the gate's
/// severity.
pub type OtherFile = fn(&Context, Severity, &mut GateOutcome, &ChangedFile) -> Result<()>;

/// One gate over a rule table: everything [`run`] does not decide itself.
pub struct TreeGate {
    /// The gate id.
    pub id: &'static str,
    /// The names of the directive that lifts this gate's findings. The first is the one a
    /// remedy prints.
    pub directive: &'static [&'static str],
    /// Whether a path is a file of this gate, and which kind.
    pub classify: fn(&str) -> Option<Classified>,
    /// A key that loosened.
    pub changed: &'static FindingKind,
    /// A change whose effect cannot be read from the diff.
    pub not_analysed: &'static FindingKind,
    /// A file that does not parse on one side.
    pub unreadable: &'static FindingKind,
    /// The message for a changed executable configuration at a path.
    pub executable_message: fn(&str) -> String,
    /// The message for a file at a path that does not parse; the second argument is the
    /// side (`base` or `head`), the base side when neither parses.
    pub unreadable_message: fn(&str, &str) -> String,
    pub absent_side: AbsentSide,
    pub read_order: ReadOrder,
    pub inherited: Option<Inherited>,
    pub other_file: Option<OtherFile>,
    /// The note when the change touches no file of this gate.
    pub nothing_examined: &'static str,
}

/// Run `gate` over the changed files under `settings`.
pub fn run(ctx: &Context, settings: &BasicGate, gate: &TreeGate) -> Result<GateOutcome> {
    let mut out = GateOutcome::new(gate.id);
    let exempt = PathFilter::new(&settings.exempt_paths)?;
    let directive = gate.directive[0];

    for file in ctx.git.changed_files()? {
        if exempt.matches(&file.path) {
            continue;
        }
        let Some(class) = (gate.classify)(&file.path) else {
            if let Some(other_file) = gate.other_file {
                other_file(ctx, settings.severity(), &mut out, &file)?;
            }
            continue;
        };
        out.examined += 1;
        // `lifts`: the finding the caller would report.
        let lift = |lifts: &FindingKind, subject: &str| {
            ctx.find_override(gate.id, lifts, gate.directive, subject)
        };

        match class {
            Classified::Executable => {
                if file.kind == ChangeKind::Added {
                    continue;
                }
                let ov = lift(gate.not_analysed, &file.path);
                let sev = settings.severity().capped_at_warning();
                out.lift_or_push(
                    ov,
                    ctx.overridable(sev),
                    gate.not_analysed,
                    (Some(&file.path), None),
                    (gate.executable_message)(&file.path),
                    &format!(
                        "Review the change; record it with `{directive}: <path> <reason>` if it is intended."
                    ),
                );
            }
            Classified::Data { name, rules } => {
                let (base, head) = match gate.read_order {
                    ReadOrder::HeadThenBase => {
                        let head = ctx.git.head_content(&file.path)?;
                        (ctx.git.base_content(&file.old_path)?, head)
                    }
                    ReadOrder::BaseThenHead => {
                        let base = ctx.git.base_content(&file.old_path)?;
                        (base, ctx.git.head_content(&file.path)?)
                    }
                };
                let (base_tree, head_tree) = match gate.absent_side {
                    AbsentSide::Empty => (
                        load_or_empty(&name, base.as_deref()),
                        load_or_empty(&name, head.as_deref()),
                    ),
                    AbsentSide::NotCompared { deleted } => {
                        let (Some(base_src), Some(head_src)) = (base, head) else {
                            if file.kind == ChangeKind::Deleted {
                                out.lift_or_push(
                                    lift(deleted, &file.path),
                                    ctx.overridable(settings.severity()),
                                    deleted,
                                    (Some(&file.path), None),
                                    format!(
                                        "`{}` was deleted; the settings it carried no longer apply.",
                                        file.path
                                    ),
                                    &format!(
                                        "Restore it, or record the deletion with `{directive}: <path> <reason>`."
                                    ),
                                );
                            }
                            continue;
                        };
                        (load(&name, &base_src), load(&name, &head_src))
                    }
                };
                let (Some(base_tree), Some(head_tree)) = (&base_tree, &head_tree) else {
                    let side = if base_tree.is_none() { "base" } else { "head" };
                    out.push(
                        settings.severity(),
                        gate.unreadable,
                        Some(&file.path),
                        None,
                        (gate.unreadable_message)(&file.path, side),
                        "Fix the file so it parses.",
                    );
                    continue;
                };
                let inherited = match gate.inherited {
                    Some(inherited) => inherited(&name, base_tree, head_tree),
                    None => Vec::new(),
                };
                for (key, gained) in inherited {
                    let ov = lift(gate.not_analysed, &key)
                        .or_else(|| lift(gate.not_analysed, &file.path));
                    let anchored = ov.is_none();
                    let sev = settings.severity().capped_at_warning();
                    out.lift_or_push(
                        ov,
                        ctx.overridable(sev),
                        gate.not_analysed,
                        (Some(&file.path), None),
                        format!(
                            "`{key}` in `{}` now inherits {gained}; what an inherited configuration loosens cannot be read from this diff.",
                            file.path
                        ),
                        &format!(
                            "Review the inherited configuration; record it with `{directive}: {key} <reason>` if it is intended."
                        ),
                    );
                    // One file can inherit through several keys: the key tells them apart.
                    if anchored {
                        out.anchor_last(key.clone());
                    }
                }
                for w in diff_trees(base_tree, head_tree, &rules) {
                    let ov = lift(gate.changed, &w.key)
                        .or_else(|| w.key.rsplit('.').next().and_then(|k| lift(gate.changed, k)))
                        .or_else(|| lift(gate.changed, &file.path));
                    let anchored = ov.is_none();
                    out.lift_or_push(
                        ov,
                        ctx.overridable(settings.severity()),
                        gate.changed,
                        (Some(&file.path), None),
                        format!("`{}` {} in `{}`.", w.key, w.what, file.path),
                        &format!(
                            "Revert it, or justify it on its own line in the PR body or a commit message: `{directive}: {} <reason>`.",
                            w.key
                        ),
                    );
                    if anchored {
                        out.anchor_last(w.key.clone());
                    }
                }
            }
        }
    }
    if out.examined == 0 {
        out.notes.push(gate.nothing_examined.to_string());
    }
    Ok(out)
}
