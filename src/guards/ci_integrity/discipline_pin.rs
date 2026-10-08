//! The discipline version a pipeline runs: where it is pinned and how a change moves it.

/// A value that chooses which discipline binary a pipeline runs: the action's `uses:`
/// (with its ref), its `version`, `binary` and `download_url` inputs, a job container or
/// image, and GitLab includes of the template or component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisciplinePin {
    pub what: &'static str,
    pub value: String,
    /// The 1-based line of the pin in its file, once `locate_pins` has read the text.
    pub line: Option<usize>,
}

/// Set each pin's line: the line holding its key and value (the image or include value
/// alone), taking the n-th such line for the n-th pin with the same key and value.
/// Comment lines do not count.
pub fn locate_pins(pins: &mut [DisciplinePin], text: &str) {
    for i in 0..pins.len() {
        let (what, value) = (pins[i].what, pins[i].value.clone());
        let nth = pins[..i]
            .iter()
            .filter(|p| p.what == what && p.value == value)
            .count();
        // An include by project is recorded as `project@ref`; the file names the project.
        let needle = match (what, value.rsplit_once('@')) {
            ("include", Some((project, _))) if !text.contains(value.as_str()) => project,
            _ => value.as_str(),
        };
        let key = match what {
            "uses" | "version" | "binary" | "download_url" => Some(format!("{what}:")),
            _ => None,
        };
        let matching: Vec<usize> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| {
                let t = l.trim_start().trim_start_matches("- ").trim_start();
                !t.starts_with('#')
                    && match &key {
                        Some(k) => t.starts_with(k.as_str()) && t.contains(needle),
                        None => t.contains(needle),
                    }
            })
            .map(|(n, _)| n + 1)
            .collect();
        pins[i].line = matching.get(nth).or(matching.first()).copied();
    }
}

fn names_discipline(s: &str) -> bool {
    s.to_ascii_lowercase().contains("discipline")
}

/// The discipline pins of one workflow or pipeline document, in document order.
pub fn discipline_pins(doc: &serde_yaml::Value) -> Vec<DisciplinePin> {
    let mut pins = Vec::new();
    let mut push = |what: &'static str, v: &str| {
        pins.push(DisciplinePin {
            what,
            value: v.trim().to_string(),
            line: None,
        })
    };
    let image_of = |v: &serde_yaml::Value| -> Option<String> {
        v.as_str()
            .or_else(|| v.get("image").and_then(|i| i.as_str()))
            .or_else(|| v.get("name").and_then(|i| i.as_str()))
            .map(str::to_string)
    };
    // GitLab includes.
    if let Some(inc) = doc.get("include") {
        let items: Vec<&serde_yaml::Value> = match inc {
            serde_yaml::Value::Sequence(s) => s.iter().collect(),
            other => vec![other],
        };
        for item in items {
            for k in ["remote", "component"] {
                if let Some(v) = item.get(k).and_then(|v| v.as_str()) {
                    if names_discipline(v) {
                        push("include", v);
                    }
                }
            }
            if let Some(p) = item.get("project").and_then(|v| v.as_str()) {
                if names_discipline(p) {
                    let r = item.get("ref").and_then(|r| r.as_str()).unwrap_or("");
                    push("include", &format!("{p}@{r}"));
                }
            }
            if let Some(img) = item.pointer_image() {
                push("image", &img);
            }
        }
    }
    let Some(map) = doc.as_mapping() else {
        return pins;
    };
    // Actions jobs live under `jobs:`; GitLab jobs are top-level keys.
    let jobs: Vec<&serde_yaml::Value> = match doc.get("jobs").and_then(|j| j.as_mapping()) {
        Some(j) => j.values().collect(),
        None => map.values().filter(|v| v.is_mapping()).collect(),
    };
    for job in jobs {
        for key in ["container", "image"] {
            if let Some(img) = job.get(key).and_then(image_of) {
                if names_discipline(&img) {
                    push("image", &img);
                }
            }
        }
        for step in job
            .get("steps")
            .and_then(|s| s.as_sequence())
            .into_iter()
            .flatten()
        {
            let Some(uses) = step.get("uses").and_then(|u| u.as_str()) else {
                continue;
            };
            if !(names_discipline(uses) || uses == "./action.yml" || uses == "./") {
                continue;
            }
            push("uses", uses);
            for input in ["version", "binary", "download_url"] {
                if let Some(v) = step.get("with").and_then(|w| w.get(input)) {
                    let v = match v {
                        serde_yaml::Value::String(s) => s.clone(),
                        other => serde_yaml::to_string(other).unwrap_or_default(),
                    };
                    push(
                        match input {
                            "version" => "version",
                            "binary" => "binary",
                            _ => "download_url",
                        },
                        &v,
                    );
                }
            }
        }
    }
    pins
}

trait PointerImage {
    fn pointer_image(&self) -> Option<String>;
}

impl PointerImage for serde_yaml::Value {
    /// `include: [{ ..., inputs: { image: ... } }]`, when it names discipline.
    fn pointer_image(&self) -> Option<String> {
        self.get("inputs")
            .and_then(|i| i.get("image"))
            .and_then(|i| i.as_str())
            .filter(|s| names_discipline(s))
            .map(str::to_string)
    }
}

/// The version part of a pin: an action or component ref (`@v0.14.4`), an image tag
/// (`:v0.14.4`, a digest dropped), a release segment of a template URL, or the value.
fn pin_version(p: &DisciplinePin) -> String {
    let v = p.value.as_str();
    match p.what {
        "uses" => v.rsplit_once('@').map(|(_, r)| r).unwrap_or("").to_string(),
        "image" => {
            let v = v.split('@').next().unwrap_or(v);
            let last = v.rsplit('/').next().unwrap_or(v);
            last.rsplit_once(':')
                .map(|(_, t)| t)
                .unwrap_or("latest")
                .to_string()
        }
        "include" => {
            if let Some((_, r)) = v.rsplit_once('@') {
                return r.to_string();
            }
            v.split('/')
                .find(|seg| release(seg).is_some())
                .unwrap_or("")
                .to_string()
        }
        _ => v.to_string(),
    }
}

/// `v1.2.3` / `1.2.3` as numbers, with how many parts were given (a `v0` tag moves).
fn release(v: &str) -> Option<(Vec<u64>, usize)> {
    let t = v.trim().trim_start_matches(['v', 'V']);
    let parts: Option<Vec<u64>> = t.split('.').map(|p| p.parse().ok()).collect();
    let parts = parts.filter(|p| !p.is_empty() && p.len() <= 3)?;
    let n = parts.len();
    Some((parts, n))
}

fn is_digest(v: &str) -> bool {
    (v.len() == 40 && v.chars().all(|c| c.is_ascii_hexdigit())) || v.starts_with("sha256:")
}

/// A ref another push can move: a branch, `latest`, a major or minor tag (`v0`), none.
fn is_mutable(v: &str) -> bool {
    match release(v) {
        Some((_, n)) => n < 3,
        None => !is_digest(v),
    }
}

/// How a pin changed between base and head: `Some(true)` blocks (a downgrade, a move to
/// a ref that can move, a new binary source), `Some(false)` is a note (an upgrade, a
/// digest bump), `None` is no change.
pub fn pin_change(what: &str, base: Option<&DisciplinePin>, head: &DisciplinePin) -> Option<bool> {
    if base.is_some_and(|b| b.value == head.value) {
        return None;
    }
    if what == "binary" || what == "download_url" {
        return Some(true);
    }
    let base = base?;
    let (b, h) = (pin_version(base), pin_version(head));
    if b == h {
        // Same version, another location or digest: not a version move.
        return Some(false);
    }
    if is_mutable(&h) && !is_mutable(&b) {
        return Some(true);
    }
    match (release(&b), release(&h)) {
        (Some((bv, 3)), Some((hv, 3))) => Some(hv < bv),
        _ => Some(false),
    }
}

/// Compare the discipline pins of a file's base and head: blocking changes (each with
/// the line of its head pin) and notes.
pub fn discipline_pin_changes(
    base: &[DisciplinePin],
    head: &[DisciplinePin],
) -> (Vec<(String, Option<usize>)>, Vec<String>) {
    let (mut blocking, mut notes) = (Vec::new(), Vec::new());
    for what in [
        "uses",
        "version",
        "binary",
        "download_url",
        "image",
        "include",
    ] {
        let b: Vec<&DisciplinePin> = base.iter().filter(|p| p.what == what).collect();
        let h: Vec<&DisciplinePin> = head.iter().filter(|p| p.what == what).collect();
        for (i, hp) in h.iter().enumerate() {
            let bp = b.get(i).copied();
            let describe = || match bp {
                Some(bp) => format!("`{what}` changed from `{}` to `{}`", bp.value, hp.value),
                None => format!("`{what}` set to `{}`", hp.value),
            };
            match pin_change(what, bp, hp) {
                Some(true) => blocking.push((describe(), hp.line)),
                Some(false) => notes.push(describe()),
                None => {}
            }
        }
    }
    (blocking, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pins(yaml: &str) -> Vec<DisciplinePin> {
        discipline_pins(&serde_yaml::from_str::<serde_yaml::Value>(yaml).unwrap())
    }

    #[test]
    fn discipline_pins_are_read_from_every_place_a_pipeline_names_the_binary() {
        let actions = pins(
            "jobs:\n  gate:\n    container:\n      image: ghcr.io/orieg/discipline:v0.14.4@sha256:abc\n    steps:\n      - uses: actions/checkout@v4\n      - uses: orieg/discipline@v0.14.4\n        with:\n          version: v0.14.4\n          binary: ./bin/discipline\n",
        );
        let whats: Vec<&str> = actions.iter().map(|p| p.what).collect();
        assert_eq!(whats, vec!["image", "uses", "version", "binary"]);
        let gitlab = pins(
            "include:\n  - remote: 'https://raw.githubusercontent.com/orieg/discipline/v0.14.4/templates/discipline.gitlab-ci.yml'\n  - project: orieg/discipline\n    ref: v0.14.4\n    file: templates/discipline.gitlab-ci.yml\ngate:\n  image: ghcr.io/orieg/discipline:v0.14.4\n  script: [discipline check]\n",
        );
        let whats: Vec<&str> = gitlab.iter().map(|p| p.what).collect();
        assert_eq!(whats, vec!["include", "include", "image"]);
        assert_eq!(pin_version(&gitlab[0]), "v0.14.4");
        assert_eq!(pin_version(&gitlab[1]), "v0.14.4");
        assert_eq!(pin_version(&gitlab[2]), "v0.14.4");
        assert!(pins("jobs:\n  t:\n    steps:\n      - uses: actions/checkout@v4\n").is_empty());
    }

    #[test]
    fn each_pin_is_located_on_its_own_line() {
        let located = |yaml: &str| {
            let mut p = pins(yaml);
            locate_pins(&mut p, yaml);
            p.iter().map(|p| (p.what, p.line)).collect::<Vec<_>>()
        };
        // Two identical steps: the second pin is on the second step, and neither the
        // script nor the comment naming the input is taken for a pin.
        let actions = "jobs:\n  build:\n    steps:\n      - run: cp target/release/discipline dist/\n  smoke:\n    steps:\n      # download_url: x\n      - uses: ./\n        with:\n          download_url: x\n      - uses: ./\n        with:\n          download_url: x\n";
        assert_eq!(
            located(actions),
            vec![
                ("uses", Some(8)),
                ("download_url", Some(10)),
                ("uses", Some(11)),
                ("download_url", Some(13)),
            ]
        );
        // An include by project is found on its project line; an image on its value, not
        // on a comment naming it.
        let gitlab = "include:\n  - project: orieg/discipline\n    ref: v0.14.4\n    file: templates/discipline.gitlab-ci.yml\n# was ghcr.io/orieg/discipline:v0.14.4\ngate:\n  image: ghcr.io/orieg/discipline:v0.14.4\n";
        assert_eq!(
            located(gitlab),
            vec![("include", Some(2)), ("image", Some(7))]
        );
    }

    #[test]
    fn a_downgrade_or_a_movable_ref_blocks_and_an_upgrade_is_a_note() {
        let pin = |what: &'static str, value: &str| DisciplinePin {
            what,
            value: value.into(),
            line: None,
        };
        let uses = |r: &str| pin("uses", &format!("orieg/discipline@{r}"));
        let change = |b: &str, h: &str| pin_change("uses", Some(&uses(b)), &uses(h));
        assert_eq!(change("v0.14.4", "v0.14.4"), None);
        assert_eq!(change("v0.14.4", "v0.13.0"), Some(true), "downgrade");
        assert_eq!(change("v0.14.4", "main"), Some(true), "a branch moves");
        assert_eq!(change("v0.14.4", "v0"), Some(true), "a major tag moves");
        assert_eq!(change("v0.14.4", "v0.15.0"), Some(false), "upgrade");
        assert_eq!(
            change("v0", "v0.14.4"),
            Some(false),
            "to an immutable release"
        );
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(change("v0.14.4", sha), Some(false), "to a commit pin");
        assert_eq!(change(sha, "main"), Some(true));
        let image = |t: &str| pin("image", &format!("ghcr.io/orieg/discipline:{t}"));
        assert_eq!(
            pin_change("image", Some(&image("v0.14.4")), &image("v0.12.0")),
            Some(true)
        );
        assert_eq!(
            pin_change("image", Some(&image("v0.14.4")), &image("latest")),
            Some(true)
        );
        // A new binary source always blocks; a removed input is not a pin change here.
        assert_eq!(
            pin_change("binary", None, &pin("binary", "./bin/discipline")),
            Some(true)
        );
        let (blocking, notes) = discipline_pin_changes(
            &[uses("v0.14.4")],
            &[
                uses("v0.15.0"),
                pin("download_url", "https://example.com/dl"),
            ],
        );
        assert_eq!(
            (blocking.len(), notes.len()),
            (1, 1),
            "{blocking:?} {notes:?}"
        );
    }
}
