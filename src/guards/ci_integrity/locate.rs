//! Where a job, a step and their keys stand in the text of a workflow or pipeline file.
//!
//! The parsed document (`serde_yaml::Value`) carries no positions: the library reports a
//! line only for a parse error. A finding's line is therefore read from the text, by the
//! indentation YAML block style is written in: a key is found as a whole key (bare or
//! quoted, followed by `:`) among the direct children of its parent mapping, and a step
//! as the n-th item of its job's `steps` sequence. Text that merely contains a name (a
//! longer job id, a `needs:` entry, a comment, a line of a script) is never a match.
//! A document written in flow style (`jobs: {a: {...}}`) has no line per key; a caller
//! then falls back to the nearest enclosing line it did find.

/// The lines of `content` that hold YAML content: `(1-based line, indent, text after the
/// indent)`. Blank lines and comment lines are left out.
fn content_lines(content: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    content.lines().enumerate().filter_map(|(idx, raw)| {
        let text = raw.trim_start_matches(' ');
        let t = text.trim_end();
        (!t.is_empty() && !t.starts_with('#')).then_some((idx + 1, raw.len() - text.len(), t))
    })
}

/// Whether `text` (a line without its indent) opens with the mapping key `key`: the whole
/// key, bare or in single or double quotes, then `:` and the end of the line or a blank.
fn is_key(text: &str, key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    [key.to_string(), format!("'{key}'"), format!("\"{key}\"")]
        .iter()
        .any(|spelling| {
            text.strip_prefix(spelling.as_str())
                .map(|rest| rest.trim_start_matches(' '))
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
        })
}

/// The indent of the content line `line`.
fn indent_at(content: &str, line: usize) -> Option<usize> {
    content_lines(content)
        .find(|(no, _, _)| *no == line)
        .map(|(_, indent, _)| indent)
}

/// The line of the top-level key `key`.
pub(crate) fn top_key_line(content: &str, key: &str) -> Option<usize> {
    content_lines(content)
        .find(|(_, indent, text)| *indent == 0 && is_key(text, key))
        .map(|(no, _, _)| no)
}

/// The line of `key` among the direct children of the mapping whose key stands on line
/// `parent`: the lines at the indent of the first child, up to the end of the parent's
/// block.
pub(crate) fn child_key_line(content: &str, parent: usize, key: &str) -> Option<usize> {
    let parent_indent = indent_at(content, parent)?;
    let mut child_indent = None;
    for (no, indent, text) in content_lines(content).filter(|(no, _, _)| *no > parent) {
        if indent <= parent_indent {
            return None;
        }
        if indent == *child_indent.get_or_insert(indent) && is_key(text, key) {
            return Some(no);
        }
    }
    None
}

/// The line of the job `job_id`: its key among the children of the top-level `jobs`.
pub(crate) fn job_key_line(content: &str, job_id: &str) -> Option<usize> {
    child_key_line(content, top_key_line(content, "jobs")?, job_id)
}

/// The line a job is reported at: its own key, or, in a document that gives no line per
/// job (flow style), the `jobs` key.
pub(crate) fn job_line(content: &str, job_id: &str) -> Option<usize> {
    job_key_line(content, job_id).or_else(|| top_key_line(content, "jobs"))
}

/// The lines of one block: `start` is its first line, `end` the line after its last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
}

/// The block that opens on line `line`: up to the next content line indented no deeper.
pub(crate) fn block_span(content: &str, line: usize) -> Span {
    let end = indent_at(content, line)
        .and_then(|own| {
            content_lines(content)
                .find(|(no, indent, _)| *no > line && *indent <= own)
                .map(|(no, _, _)| no)
        })
        .unwrap_or_else(|| content.lines().count() + 1);
    Span { start: line, end }
}

/// The items of the `steps` sequence under the mapping on line `parent` (a job, or a
/// composite action's `runs`), one span per item in order. `None` when the sequence is
/// not written as `count` block items (flow style, an alias): no item can then be told
/// from its neighbours by line.
pub(crate) fn step_spans(content: &str, parent: usize, count: usize) -> Option<Vec<Span>> {
    let steps_line = child_key_line(content, parent, "steps")?;
    let steps_indent = indent_at(content, steps_line)?;
    let is_item = |text: &str| text == "-" || text.starts_with("- ");
    let mut starts = Vec::new();
    let mut item_indent = None;
    let mut end = content.lines().count() + 1;
    for (no, indent, text) in content_lines(content).filter(|(no, _, _)| *no > steps_line) {
        let item = match item_indent {
            Some(i) => i,
            None if indent >= steps_indent && is_item(text) => *item_indent.insert(indent),
            None => return None,
        };
        if indent < item || (indent == item && !is_item(text)) {
            end = no;
            break;
        }
        if indent == item {
            starts.push(no);
        }
    }
    if starts.len() != count {
        return None;
    }
    Some(
        starts
            .iter()
            .enumerate()
            .map(|(i, start)| Span {
                start: *start,
                end: starts.get(i + 1).copied().unwrap_or(end),
            })
            .collect(),
    )
}

impl Span {
    /// The line of `key` among the keys of the sequence item this span holds: the key
    /// after the item's `-`, and the keys aligned with it below.
    pub(crate) fn item_key_line(&self, content: &str, key: &str) -> Option<usize> {
        let mut key_column = None;
        for (no, indent, text) in content_lines(content) {
            if no < self.start || no >= self.end {
                continue;
            }
            let (column, text) = match key_column {
                None => {
                    let after = text.strip_prefix('-')?;
                    let key_text = after.trim_start_matches(' ');
                    let column = indent + 1 + (after.len() - key_text.len());
                    key_column = Some(column);
                    (column, key_text)
                }
                Some(_) => (indent, text),
            };
            if Some(column) == key_column && is_key(text, key) {
                return Some(no);
            }
        }
        None
    }

    /// The first line of the span, at or after `from`, that contains `needle`.
    pub(crate) fn line_with(&self, content: &str, from: usize, needle: &str) -> Option<usize> {
        content
            .lines()
            .enumerate()
            .map(|(idx, line)| (idx + 1, line))
            .find(|(no, line)| {
                *no >= from.max(self.start) && *no < self.end && line.contains(needle)
            })
            .map(|(no, _)| no)
    }

    /// The first content line of the span that holds `word` as a whole word: not as part
    /// of a longer identifier (letters, digits, `_`, `-`).
    pub(crate) fn line_with_word(&self, content: &str, word: &str) -> Option<usize> {
        let part_of_word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        content_lines(content)
            .filter(|(no, _, _)| *no >= self.start && *no < self.end)
            .find(|(_, _, text)| {
                text.match_indices(word).any(|(at, _)| {
                    !text[..at].chars().next_back().is_some_and(part_of_word)
                        && !text[at + word.len()..]
                            .chars()
                            .next()
                            .is_some_and(part_of_word)
                })
            })
            .map(|(no, _, _)| no)
    }
}

/// The line a step is reported at, inside its own span: the line of its `name`, else of
/// its `id`, else of its `uses`, else the line of its `run` that holds the script's first
/// line, else the item's first line.
pub(crate) fn step_line(
    content: &str,
    span: Span,
    name: &str,
    id: &str,
    uses: Option<&str>,
    run: Option<&str>,
) -> usize {
    let key =
        |present: bool, key: &str| present.then(|| span.item_key_line(content, key)).flatten();
    key(!name.is_empty(), "name")
        .or_else(|| key(!id.is_empty(), "id"))
        .or_else(|| key(uses.is_some(), "uses"))
        .or_else(|| {
            let run_line = key(run.is_some(), "run")?;
            let first = run?.lines().next().unwrap_or("").trim();
            (!first.is_empty())
                .then(|| span.line_with(content, run_line, first))
                .flatten()
                .or(Some(run_line))
        })
        .unwrap_or(span.start)
}

/// The scalar a line holds: the value after `key: ` (or the whole item of a `- value`
/// line), without a trailing comment and without its quotes.
pub(crate) fn line_scalar(line: &str) -> &str {
    let t = line.trim();
    let t = t.strip_prefix("- ").map_or(t, str::trim_start);
    let value = match t.split_once(": ") {
        Some((_, v)) => v,
        None if t.ends_with(':') => "",
        None => t,
    };
    let value = value.split(" #").next().unwrap_or(value).trim();
    let unquoted = |q: char| {
        value
            .strip_prefix(q)
            .and_then(|v| v.strip_suffix(q))
            .filter(|_| value.len() >= 2)
    };
    unquoted('"').or_else(|| unquoted('\'')).unwrap_or(value)
}

/// The line that names the trigger `event`: its key under `on`, else the line of the `on`
/// block that lists it (`on: [push, event]`, `- event`), else the `on` line.
pub(crate) fn trigger_line(content: &str, event: &str) -> Option<usize> {
    let on_line = top_key_line(content, "on").or_else(|| top_key_line(content, "true"))?;
    child_key_line(content, on_line, event)
        .or_else(|| block_span(content, on_line).line_with_word(content, event))
        .or(Some(on_line))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WF: &str = "\
name: test
on:
  push:
# test: a comment
jobs:
  test-lint:
    needs: [test]
    runs-on: ubuntu-latest
    steps:
      - run: echo test
  'test':
    runs-on: ubuntu-latest
    continue-on-error: true
    steps:
      - name: Build
        run: |
          cargo build
          cargo test
      - run: cargo test
        if: failure()
  \"deploy\" :
    steps:
    - uses: a/b@v1
    - id: x
      name: Named
";

    #[test]
    fn a_job_is_found_by_its_whole_key_among_the_children_of_jobs() {
        assert_eq!(job_key_line(WF, "test"), Some(11));
        assert_eq!(job_key_line(WF, "test-lint"), Some(6));
        assert_eq!(job_key_line(WF, "deploy"), Some(21));
        // A key of a job is not a job, and a name that is only part of a key is none.
        assert_eq!(job_key_line(WF, "needs"), None);
        assert_eq!(job_key_line(WF, "lint"), None);
        assert_eq!(job_key_line(WF, ""), None);
        assert_eq!(top_key_line(WF, "jobs"), Some(5));
        assert_eq!(top_key_line(WF, "push"), None);
    }

    #[test]
    fn a_job_key_is_a_direct_child_only() {
        assert_eq!(child_key_line(WF, 11, "continue-on-error"), Some(13));
        assert_eq!(child_key_line(WF, 6, "continue-on-error"), None);
        // `if` is a key of a step of the job, not of the job.
        assert_eq!(child_key_line(WF, 11, "if"), None);
        assert_eq!(block_span(WF, 11), Span { start: 11, end: 21 });
        assert_eq!(block_span(WF, 21), Span { start: 21, end: 26 });
    }

    #[test]
    fn steps_are_the_items_of_their_own_job() {
        let spans = step_spans(WF, 11, 2).expect("two block items");
        assert_eq!(
            spans,
            [Span { start: 15, end: 19 }, Span { start: 19, end: 21 }]
        );
        // The second step's script is also a line of the first step's script.
        assert_eq!(
            step_line(WF, spans[1], "", "", None, Some("cargo test")),
            19
        );
        assert_eq!(
            step_line(WF, spans[0], "Build", "", None, Some("cargo build\n")),
            15
        );
        assert_eq!(
            step_line(WF, spans[0], "", "", None, Some("cargo build\n")),
            17
        );
        assert_eq!(spans[1].item_key_line(WF, "if"), Some(20));
        assert_eq!(spans[0].item_key_line(WF, "if"), None);
        // Items written at the indent of `steps` itself.
        let spans = step_spans(WF, 21, 2).expect("two block items");
        assert_eq!(spans[0].item_key_line(WF, "uses"), Some(23));
        assert_eq!(step_line(WF, spans[1], "Named", "x", None, None), 25);
        assert_eq!(step_line(WF, spans[1], "", "x", None, None), 24);
        // A count that is not the number of block items gives no spans.
        assert_eq!(step_spans(WF, 21, 3), None);
        assert_eq!(
            step_spans("jobs:\n  a:\n    steps: [{run: x}]\n", 2, 1),
            None
        );
    }

    #[test]
    fn a_word_is_not_part_of_a_longer_one() {
        let on = "on:\n  # pull_request_target is not used\n  workflow_run:\n    types: [my-pull_request_target]\n  push:\non2: [push, pull_request_target]\n";
        assert_eq!(
            block_span(on, 1).line_with_word(on, "pull_request_target"),
            None
        );
        assert_eq!(
            block_span(on, 6).line_with_word(on, "pull_request_target"),
            Some(6)
        );
    }

    #[test]
    fn the_scalar_of_a_line_is_its_whole_value() {
        assert_eq!(line_scalar("  - uses: a/b@v1.2 # pinned"), "a/b@v1.2");
        assert_eq!(line_scalar("    version: \"0.17.2\""), "0.17.2");
        assert_eq!(line_scalar("  image: ghcr.io/a/b:1"), "ghcr.io/a/b:1");
        assert_eq!(
            line_scalar("  - 'https://example.org/t.yml'"),
            "https://example.org/t.yml"
        );
        assert_eq!(line_scalar("  include:"), "");
    }
}
