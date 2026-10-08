//! The pairing of the base and head expectations of one test ([`widened`]).
//!
//! Which expectation is reported depends on the order in which the two sides are
//! matched, so the pairing is the one written first for it (kept in the tests of this
//! file as `reference`), reached in less work. That one compared every base expectation
//! with every head expectation, and for the rewritten ones did so again at each step of
//! each search: 2,000 expectations each rewritten cost 2.0e11 instructions, and 5.9 times
//! that for twice as many.
//!
//! What two expectations answer when compared ([`is_widened`], [`interchangeable`])
//! depends on how each is written and not on where it stands ([`Written`]), so the
//! expectations of each side are sorted into those written alike and the answer is kept
//! for each pair of these. A search for a head expectation is the same search for every
//! base expectation written alike, so it goes on from where the last one of them stopped.
//!
//! What still grows with the square of the expectations: two written differently are
//! compared once for each pair of ways of writing one that a search brings together, as
//! in the first pairing, so a test whose expectations each carry a message of their own
//! costs one comparison for each pair of them; and two ways of writing one that take
//! head expectations from each other are searched again for each.

use super::super::ancestry::count;
use super::{
    effective_matcher, interchangeable, is_attribute, is_widened, type_names, ExpectedException,
    Widened, OPAQUE,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

/// What a comparison reads of an expectation: everything but its line, its skeleton and
/// the call it guards. A field a comparison comes to read belongs here: without it, two
/// expectations that differ in that field alone are answered as one.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Written<'a> {
    kind: &'a str,
    exception_type: Option<&'a str>,
    matcher: Option<&'a str>,
    whole_message: bool,
    form: &'a str,
    /// The class declarations it is read with ([`Declared`]).
    declared: usize,
}

/// The lists of class declarations the expectations of a test are read with, each given
/// a number: two lists that hold the same declarations have the same one. The
/// expectations of one file share one list, so a list is read once for where it is kept
/// and not once for each expectation.
#[derive(Default)]
struct Declared<'a> {
    kept_at: HashMap<*const Vec<(String, String)>, usize>,
    holding: HashMap<&'a [(String, String)], usize>,
}

impl<'a> Declared<'a> {
    fn number(&mut self, e: &'a ExpectedException) -> usize {
        let next = self.holding.len();
        *self
            .kept_at
            .entry(Arc::as_ptr(&e.declared))
            .or_insert_with(|| {
                count(e.declared.len());
                *self.holding.entry(e.declared.as_slice()).or_insert(next)
            })
    }
}

/// The expectations of one side, sorted into those written alike.
struct Sorted<'a> {
    /// For each expectation, which of the ways of writing one it is.
    class: Vec<usize>,
    /// One expectation of each way.
    one_of: Vec<&'a ExpectedException>,
}

impl<'a> Sorted<'a> {
    fn new(side: &[&'a ExpectedException], declared: &mut Declared<'a>) -> Self {
        let mut known: HashMap<Written<'a>, usize> = HashMap::new();
        let mut one_of = Vec::new();
        let class = side
            .iter()
            .map(|e| {
                count(1);
                let written = Written {
                    kind: &e.kind,
                    exception_type: e.exception_type.as_deref(),
                    matcher: e.matcher.as_deref(),
                    whole_message: e.whole_message,
                    form: e.form,
                    declared: declared.number(e),
                };
                *known.entry(written).or_insert_with(|| {
                    one_of.push(*e);
                    one_of.len() - 1
                })
            })
            .collect();
        Self { class, one_of }
    }
}

/// Places in a list, kept as the runs of neighbours they form: the place after a run is
/// found without reading the run.
#[derive(Default)]
struct Runs {
    /// The first place of each run, and the place after its last.
    runs: BTreeMap<usize, usize>,
}

impl Runs {
    /// The place after the run that holds `at`, when one does.
    fn after(&self, at: usize) -> Option<usize> {
        let (_, &end) = self.runs.range(..=at).next_back()?;
        (at < end).then_some(end)
    }

    fn insert(&mut self, at: usize) {
        let mut start = at;
        let mut end = at + 1;
        if let Some((&before, &before_end)) = self.runs.range(..at).next_back() {
            if before_end == at {
                start = before;
            }
        }
        if let Some(after_end) = self.runs.remove(&end) {
            end = after_end;
        }
        self.runs.insert(start, end);
    }

    fn remove(&mut self, at: usize) {
        let Some((&start, &end)) = self.runs.range(..=at).next_back() else {
            return;
        };
        if at >= end {
            return;
        }
        self.runs.remove(&start);
        if start < at {
            self.runs.insert(start, at);
        }
        if at + 1 < end {
            self.runs.insert(at + 1, end);
        }
    }
}

/// Where the search of one way of writing a base expectation stands.
#[derive(Default)]
struct Search {
    /// The search this was last entered in.
    entered: usize,
    /// The head expectation it reads next.
    next: usize,
    /// A search that began from one written this way found nothing: none will.
    spent: bool,
    /// The head expectations its search passes without reading them: the ones that stand
    /// for a base expectation written this way, and, while there is room to keep them
    /// ([`Matching::room`]), the ones found to be of no use to it for good.
    passed: Runs,
}

/// The matching of the rewritten expectations: each base expectation is given a head
/// expectation that covers it, an earlier one being moved to another when it has one.
///
/// The first pairing kept, for each head expectation, the base expectation it stood for,
/// and searched from each base expectation in turn: the head expectations in order, each
/// one not yet seen in this search that covers it, and from a taken one the search of
/// the base expectation that holds it. What the search does depends on how that base
/// expectation is written and not on which one it is, so here a head expectation is held
/// by a way of writing one, and three things follow.
///
/// - A search that reaches a head expectation held by the way it is searching for would
///   go on, in the first pairing, as the search of another base expectation written the
///   same: over the same head expectations, from where this one stands. Such a head
///   expectation is passed, and a run of them at once ([`Runs`]). So is one that does
///   not cover the way, which never will.
/// - Every search of one way of writing reads the same head expectations in the same
///   order, and within one search each is read once: a way entered again goes on from
///   where it stopped, and the ones a search has taken are passed by every way, a run
///   of them at once.
/// - A search that finds nothing has read every head expectation it could reach, all of
///   them held, and every way that holds one. No later search changes what holds them,
///   so none finds anything through them: those ways are spent, and what they hold is
///   passed from then on.
struct Matching<'a> {
    base: Sorted<'a>,
    head: Sorted<'a>,
    /// Whether a head expectation of one way covers a base expectation of another, for
    /// each pair of ways asked about.
    covers: HashMap<(usize, usize), bool>,
    /// The way of writing a base expectation that each head expectation stands for.
    holder: Vec<Option<usize>>,
    /// The head expectations taken so far in the search under way, each seen once.
    seen: Runs,
    searches: Vec<Search>,
    /// How many more head expectations of no use to a way are kept as passed by it.
    /// Keeping them costs memory for each way, so they are kept for as many as the two
    /// sides hold several times over; past that a search reads them again each time.
    room: usize,
}

impl<'a> Matching<'a> {
    fn new(base: &[&'a ExpectedException], head: &[&'a ExpectedException], room: usize) -> Self {
        let mut declared = Declared::default();
        let base = Sorted::new(base, &mut declared);
        Self {
            searches: base.one_of.iter().map(|_| Search::default()).collect(),
            base,
            head: Sorted::new(head, &mut declared),
            covers: HashMap::new(),
            holder: vec![None; head.len()],
            seen: Runs::default(),
            room,
        }
    }

    fn covers(&mut self, way: usize, head_way: usize) -> bool {
        let (b, h) = (self.base.one_of[way], self.head.one_of[head_way]);
        *self
            .covers
            .entry((way, head_way))
            .or_insert_with(|| interchangeable(&b.kind, &h.kind) && is_widened(b, h).is_none())
    }

    /// The next head expectation the search of `way` reads in search `number`: one that
    /// covers it, not seen in this search, and not held by a way that is spent or by
    /// `way` itself.
    fn next_for(&mut self, way: usize, number: usize) -> Option<usize> {
        loop {
            let at = self.searches[way].next;
            if at >= self.holder.len() {
                return None;
            }
            count(1);
            let passed = self.searches[way].passed.after(at);
            if let Some(after) = passed.or_else(|| self.seen.after(at)) {
                self.searches[way].next = after;
                continue;
            }
            self.searches[way].next = at + 1;
            if let Some(holder) = self.holder[at] {
                let held_by = &self.searches[holder];
                if held_by.spent {
                    self.pass(way, at);
                    continue;
                }
                // Passed by the search of the way that holds it, which saw it then.
                if held_by.entered == number && at < held_by.next {
                    continue;
                }
            }
            if self.covers(way, self.head.class[at]) {
                return Some(at);
            }
            self.pass(way, at);
        }
    }

    /// Keeps the head expectation `at` as passed by `way` from now on, while there is
    /// room: it does not cover the way, or a way that is spent holds it.
    fn pass(&mut self, way: usize, at: usize) {
        if self.room > 0 {
            self.room -= 1;
            self.searches[way].passed.insert(at);
        }
    }

    fn enter(&mut self, way: usize, number: usize, entered: &mut Vec<usize>) {
        let search = &mut self.searches[way];
        if search.entered != number {
            search.entered = number;
            search.next = 0;
            entered.push(way);
        }
    }

    /// Search `number` (from 1), for the base expectation `from`: whether it was given a
    /// head expectation.
    fn give(&mut self, from: usize, number: usize) -> bool {
        let mut way = self.base.class[from];
        if self.searches[way].spent {
            return false;
        }
        self.seen.runs.clear();
        let mut entered = Vec::new();
        // The ways the search went through, each with the head expectation it took.
        let mut path: Vec<(usize, usize)> = Vec::new();
        self.enter(way, number, &mut entered);
        loop {
            match self.next_for(way, number) {
                Some(at) => {
                    self.seen.insert(at);
                    path.push((way, at));
                    match self.holder[at] {
                        Some(holder) => {
                            way = holder;
                            self.enter(way, number, &mut entered);
                        }
                        None => break,
                    }
                }
                None => match path.pop() {
                    Some((before, _)) => way = before,
                    None => {
                        for way in entered {
                            self.searches[way].spent = true;
                        }
                        return false;
                    }
                },
            }
        }
        // Each way on the path takes the head expectation it reached, from the way that
        // held it.
        for (way, at) in path {
            if let Some(holder) = self.holder[at] {
                self.searches[holder].passed.remove(at);
            }
            self.searches[way].passed.insert(at);
            self.holder[at] = Some(way);
        }
        true
    }
}

/// The expectations of `side` that are not marked in `set_aside`, in order.
fn left_of<'a>(side: Vec<&'a ExpectedException>, set_aside: &[bool]) -> Vec<&'a ExpectedException> {
    side.into_iter()
        .zip(set_aside)
        .filter(|(_, aside)| !**aside)
        .map(|(e, _)| e)
        .collect()
}

/// The kind of each expectation of `side`, as a number kept in `kinds`.
fn kinds_of<'a>(side: &[&'a ExpectedException], kinds: &mut HashMap<&'a str, usize>) -> Vec<usize> {
    side.iter()
        .map(|e| {
            let next = kinds.len();
            *kinds.entry(e.kind.as_str()).or_insert(next)
        })
        .collect()
}

/// Pairs the `base` and `head` expectations of one test and returns the base expectations
/// that head checks less strictly, or no longer checks.
///
/// 1. An expectation written the same on both sides (skeleton, kind, type and matcher)
///    is unchanged, however many times it occurs and in whatever order.
/// 2. Of the rest, a skeleton left exactly once on each side is the same assertion edited
///    in place: the two are compared.
/// 3. What remains was rewritten beyond its skeleton (an edited block, a renamed binding,
///    a site replaced by another). Each base expectation needs a head expectation of its
///    own that accepts no more than it did; the assignment that satisfies the most base
///    expectations is taken. One left without is reported against a remaining head
///    expectation, or as dropped when head has none left.
pub fn widened(base: &[ExpectedException], head: &[ExpectedException]) -> Vec<Widened> {
    paired(base, head, 8 * (base.len() + head.len()) + 64)
}

/// [`widened`], with `room` for what the searches of the matching keep as passed
/// ([`Matching::room`]): what is returned does not depend on it.
fn paired(base: &[ExpectedException], head: &[ExpectedException], room: usize) -> Vec<Widened> {
    // 1. The head expectations by how each is written, in order: a base expectation
    // takes the first one left that is written as it is.
    type Same<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        bool,
        &'a str,
    );
    fn same(e: &ExpectedException) -> Same<'_> {
        (
            &e.skeleton,
            &e.kind,
            e.exception_type.as_deref(),
            e.matcher.as_deref(),
            e.whole_message,
            e.form,
        )
    }
    // The two sides written the same, one for one and in order, as those of a test the
    // change does not touch are: every expectation is unchanged.
    if base.len() == head.len() && base.iter().zip(head).all(|(b, h)| same(b) == same(h)) {
        count(base.len());
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut unchanged: HashMap<Same, VecDeque<usize>> = HashMap::new();
    for (j, h) in head.iter().enumerate() {
        count(1);
        unchanged.entry(same(h)).or_default().push_back(j);
    }
    let mut taken = vec![false; head.len()];
    let mut base_left: Vec<&ExpectedException> = Vec::new();
    for b in base {
        count(1);
        match unchanged.get_mut(&same(b)).and_then(VecDeque::pop_front) {
            Some(j) => taken[j] = true,
            None => base_left.push(b),
        }
    }
    let head_left: Vec<&ExpectedException> = head
        .iter()
        .zip(&taken)
        .filter(|(_, taken)| !**taken)
        .map(|(h, _)| h)
        .collect();

    // 2. How many times each skeleton is left on each side, and where on the head side.
    let mut base_times: HashMap<&str, usize> = HashMap::new();
    for b in &base_left {
        *base_times.entry(&b.skeleton).or_default() += 1;
    }
    let mut head_times: HashMap<&str, (usize, usize)> = HashMap::new();
    for (j, h) in head_left.iter().enumerate() {
        head_times.entry(&h.skeleton).or_insert((0, j)).0 += 1;
    }
    let mut base_in_place = vec![false; base_left.len()];
    let mut head_in_place = vec![false; head_left.len()];
    for (i, b) in base_left.iter().enumerate() {
        count(1);
        let once = base_times.get(b.skeleton.as_str()) == Some(&1);
        let Some(&(1, j)) = head_times.get(b.skeleton.as_str()).filter(|_| once) else {
            continue;
        };
        let h = head_left[j];
        base_in_place[i] = true;
        head_in_place[j] = true;
        if let Some(detail) = is_widened(b, h) {
            out.push(Widened {
                line: h.line,
                skeleton: h.skeleton.clone(),
                detail,
                dropped: false,
                guarded_call: None,
            });
        }
    }
    let (base_left, head_left) = (
        left_of(base_left, &base_in_place),
        left_of(head_left, &head_in_place),
    );

    // 3. The matching, then each base expectation without a head expectation.
    let mut matching = Matching::new(&base_left, &head_left, room);
    let given: Vec<bool> = (0..base_left.len())
        .map(|i| {
            count(1);
            matching.give(i, i + 1)
        })
        .collect();
    let mut free: Vec<bool> = matching.holder.iter().map(Option::is_none).collect();
    // The kinds of each side, and for each kind of the base side the head expectation
    // from which one that is free and can stand for that kind is looked for: a head
    // expectation is not free again once it is taken.
    let mut kinds: HashMap<&str, usize> = HashMap::new();
    let base_kind = kinds_of(&base_left, &mut kinds);
    let head_kind = kinds_of(&head_left, &mut kinds);
    let mut spare_from: HashMap<usize, usize> = HashMap::new();
    let mut stands_for: HashMap<(usize, usize), bool> = HashMap::new();
    for (i, b) in base_left.iter().enumerate() {
        if given[i] {
            continue;
        }
        let from = spare_from.entry(base_kind[i]).or_insert(0);
        let spare = loop {
            let j = *from;
            if j >= head_left.len() {
                break None;
            }
            count(1);
            if free[j]
                && *stands_for
                    .entry((base_kind[i], head_kind[j]))
                    .or_insert_with(|| interchangeable(&b.kind, &head_left[j].kind))
            {
                break Some(j);
            }
            *from += 1;
        };
        match spare {
            Some(j) => {
                free[j] = false;
                let h = head_left[j];
                out.push(Widened {
                    line: h.line,
                    skeleton: h.skeleton.clone(),
                    detail: is_widened(b, h).unwrap_or_else(|| {
                        "replaced by an expectation that accepts more".to_string()
                    }),
                    dropped: false,
                    guarded_call: None,
                });
            }
            None if !is_attribute(&b.kind) => out.push(Widened {
                line: 0,
                skeleton: b.skeleton.clone(),
                detail: match (type_names(b).first(), effective_matcher(b)) {
                    (Some(t), _) => format!("expected exception `{}` is no longer checked", t.name),
                    (None, Some(m)) => format!(
                        "expected message `{}` is no longer checked",
                        m.trim_start_matches(OPAQUE)
                    ),
                    (None, None) => "expected failure is no longer checked".to_string(),
                },
                dropped: true,
                guarded_call: b.guarded_call.clone(),
            }),
            None => {}
        }
    }
    out.sort_by_key(|w| (w.dropped, w.line));
    out
}

#[cfg(test)]
mod tests {
    use super::super::super::{ancestry, default_registry, AssertVocabulary};
    use super::super::{THROWS, THROWS_BESIDE_EXACTLY, THROWS_EXACTLY, THROWS_OF_MSTEST};
    use super::*;

    /// The pairing as it was first written, word for word: what [`widened`] must return,
    /// finding for finding and in the same order.
    mod reference {
        use super::*;

        /// Pairs the `base` and `head` expectations of one test and returns the base expectations
        /// that head checks less strictly, or no longer checks.
        ///
        /// 1. An expectation written the same on both sides (skeleton, kind, type and matcher)
        ///    is unchanged, however many times it occurs and in whatever order.
        /// 2. Of the rest, a skeleton left exactly once on each side is the same assertion edited
        ///    in place: the two are compared.
        /// 3. What remains was rewritten beyond its skeleton (an edited block, a renamed binding,
        ///    a site replaced by another). Each base expectation needs a head expectation of its
        ///    own that accepts no more than it did; the assignment that satisfies the most base
        ///    expectations is taken. One left without is reported against a remaining head
        ///    expectation, or as dropped when head has none left.
        pub fn widened(base: &[ExpectedException], head: &[ExpectedException]) -> Vec<Widened> {
            let same = |b: &ExpectedException, h: &ExpectedException| {
                b.skeleton == h.skeleton
                    && b.kind == h.kind
                    && b.exception_type == h.exception_type
                    && b.matcher == h.matcher
                    && b.whole_message == h.whole_message
                    && b.form == h.form
            };
            let mut out = Vec::new();
            let mut head_left: Vec<&ExpectedException> = head.iter().collect();
            let mut base_left: Vec<&ExpectedException> = Vec::new();
            for b in base {
                match head_left.iter().position(|h| same(b, h)) {
                    Some(i) => {
                        head_left.remove(i);
                    }
                    None => base_left.push(b),
                }
            }

            let once = |set: &[&ExpectedException], s: &str| {
                set.iter().filter(|x| x.skeleton == s).count() == 1
            };
            let in_place: Vec<(&ExpectedException, &ExpectedException)> = base_left
                .iter()
                .filter(|b| once(&base_left, &b.skeleton) && once(&head_left, &b.skeleton))
                .filter_map(|b| {
                    let h = head_left.iter().find(|h| h.skeleton == b.skeleton)?;
                    Some((*b, *h))
                })
                .collect();
            for (b, h) in &in_place {
                base_left.retain(|x| !std::ptr::eq(*x, *b));
                head_left.retain(|x| !std::ptr::eq(*x, *h));
                if let Some(detail) = is_widened(b, h) {
                    out.push(Widened {
                        line: h.line,
                        skeleton: h.skeleton.clone(),
                        detail,
                        dropped: false,
                        guarded_call: None,
                    });
                }
            }

            // `owner[j]` is the base expectation that head expectation `j` stands for.
            let covers = |b: &ExpectedException, h: &ExpectedException| {
                interchangeable(&b.kind, &h.kind) && is_widened(b, h).is_none()
            };
            let mut owner: Vec<Option<usize>> = vec![None; head_left.len()];
            for i in 0..base_left.len() {
                let mut seen = vec![false; head_left.len()];
                assign(i, &base_left, &head_left, &covers, &mut owner, &mut seen);
            }
            for (i, b) in base_left.iter().enumerate() {
                if owner.contains(&Some(i)) {
                    continue;
                }
                let spare = (0..head_left.len())
                    .find(|&j| owner[j].is_none() && interchangeable(&b.kind, &head_left[j].kind));
                match spare {
                    Some(j) => {
                        owner[j] = Some(i);
                        let h = head_left[j];
                        out.push(Widened {
                            line: h.line,
                            skeleton: h.skeleton.clone(),
                            detail: is_widened(b, h).unwrap_or_else(|| {
                                "replaced by an expectation that accepts more".to_string()
                            }),
                            dropped: false,
                            guarded_call: None,
                        });
                    }
                    None if !is_attribute(&b.kind) => out.push(Widened {
                        line: 0,
                        skeleton: b.skeleton.clone(),
                        detail: match (type_names(b).first(), effective_matcher(b)) {
                            (Some(t), _) => {
                                format!("expected exception `{}` is no longer checked", t.name)
                            }
                            (None, Some(m)) => format!(
                                "expected message `{}` is no longer checked",
                                m.trim_start_matches(OPAQUE)
                            ),
                            (None, None) => "expected failure is no longer checked".to_string(),
                        },
                        dropped: true,
                        guarded_call: b.guarded_call.clone(),
                    }),
                    None => {}
                }
            }
            out.sort_by_key(|w| (w.dropped, w.line));
            out
        }

        /// One augmenting step of a bipartite matching: gives base expectation `i` a head
        /// expectation that covers it, moving an earlier assignment aside when it has another.
        fn assign(
            i: usize,
            base: &[&ExpectedException],
            head: &[&ExpectedException],
            covers: &dyn Fn(&ExpectedException, &ExpectedException) -> bool,
            owner: &mut [Option<usize>],
            seen: &mut [bool],
        ) -> bool {
            for j in 0..head.len() {
                if seen[j] || !covers(base[i], head[j]) {
                    continue;
                }
                seen[j] = true;
                let free = match owner[j] {
                    None => true,
                    Some(other) => assign(other, base, head, covers, owner, seen),
                };
                if free {
                    owner[j] = Some(i);
                    return true;
                }
            }
            false
        }
    }

    /// A sequence of numbers that is the same on every run (a linear congruential
    /// generator, the constants of Knuth's MMIX).
    struct Sequence(u64);

    impl Sequence {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) % n as u64) as usize
        }

        fn one_of<T: Copy>(&mut self, of: &[T]) -> T {
            of[self.below(of.len())]
        }

        fn one_in(&mut self, n: usize) -> bool {
            self.below(n) == 0
        }
    }

    /// The kinds one list draws from, with the classes and the forms that go with them.
    struct Language {
        kinds: &'static [&'static str],
        types: &'static [Option<&'static str>],
        forms: &'static [&'static str],
    }

    const LANGUAGES: &[Language] = &[
        Language {
            kinds: &[
                "pytest.raises",
                "pytest.raises",
                "assertRaises",
                "assertWarns",
                "xfail",
            ],
            types: &[
                None,
                Some("ValueError"),
                Some("ValueError"),
                Some("Exception"),
                Some("BaseException"),
                Some("KeyError"),
                Some("LookupError"),
                Some("IndexError"),
                Some("IOError"),
                Some("OSError"),
                Some("ValueError, TypeError"),
                Some("KeyError, IndexError"),
                Some("OrderError"),
                Some("AppError"),
                Some("errors.TimeoutError"),
            ],
            forms: &[""],
        },
        Language {
            kinds: &[
                "assertThrows",
                "assertThrows",
                "assertThrowsExactly",
                "doesNotThrow",
                "test_expected",
            ],
            types: &[
                None,
                Some("IllegalArgumentException"),
                Some("RuntimeException"),
                Some("Exception"),
                Some("Throwable"),
                Some("FileNotFoundException"),
                Some("IOException"),
                Some("java.io.IOException"),
                Some("OrderError"),
            ],
            forms: &[""],
        },
        Language {
            kinds: &["toThrow", "toThrow", "not.toThrow"],
            types: &[
                None,
                Some("TypeError"),
                Some("RangeError"),
                Some("Error"),
                Some("AppError"),
            ],
            forms: &[""],
        },
        Language {
            kinds: &[
                "Assert.Throws",
                "Assert.Throws",
                "Assert.ThrowsAny",
                "Throws.Nothing",
            ],
            types: &[
                None,
                Some("ArgumentNullException"),
                Some("ArgumentException"),
                Some("Exception"),
                Some("System.Exception"),
                Some("InvalidOperationException"),
            ],
            forms: &[
                "",
                THROWS,
                THROWS_EXACTLY,
                THROWS_BESIDE_EXACTLY,
                THROWS_OF_MSTEST,
            ],
        },
        Language {
            kinds: &[
                "expectException",
                "expectExceptionMessage",
                "expectExceptionCode",
                "expectExceptionMessageMatches",
                "should_panic",
            ],
            types: &[
                None,
                Some("LogicException"),
                Some("\\Exception"),
                Some("DomainException"),
            ],
            forms: &[""],
        },
        Language {
            kinds: &[
                "raise_error",
                "assert_raises",
                "not.raise_error",
                "go.Error",
                "go.Panics",
                "go.NoError",
            ],
            types: &[
                None,
                Some("ArgumentError"),
                Some("StandardError"),
                Some("KeyError"),
                Some("ErrNotFound"),
            ],
            forms: &[""],
        },
    ];

    const MATCHERS: &[Option<&str>] = &[
        None,
        None,
        None,
        Some(""),
        Some("negative"),
        Some("negative value"),
        Some("value"),
        Some(".*"),
        Some("\u{1}/a.c/"),
        Some("\u{1}MSG"),
        Some("/^neg/"),
    ];

    /// One list of expectations drawn from `language`. Few skeletons, few classes and
    /// few matchers, so that many are written alike, many share a skeleton, and many are
    /// one another's ancestors.
    fn drawn(
        seq: &mut Sequence,
        language: &Language,
        declared: &[Arc<Vec<(String, String)>>],
        len: usize,
        skeletons: usize,
    ) -> Vec<ExpectedException> {
        let own = seq.one_of(&[0, 0, 0, 1, 2, 3]);
        (0..len)
            .map(|i| ExpectedException {
                // Lines that repeat: the findings of one line keep the order found.
                line: if seq.one_in(4) {
                    1 + seq.below(4)
                } else {
                    i + 1
                },
                skeleton: format!("s{}", seq.below(skeletons)),
                kind: seq.one_of(language.kinds).to_string(),
                exception_type: seq.one_of(language.types).map(str::to_string),
                matcher: seq.one_of(MATCHERS).map(str::to_string),
                whole_message: seq.one_in(5),
                declared: declared[if seq.one_in(6) {
                    seq.below(declared.len())
                } else {
                    own
                }]
                .clone(),
                guarded_call: seq.one_in(3).then(|| format!("f({})", seq.below(3))),
                form: seq.one_of(language.forms),
            })
            .collect()
    }

    /// A head side made from `base`: expectations reordered, removed, added, written
    /// again with another skeleton, with a wider or a narrower class, with another
    /// matcher.
    fn edited(
        seq: &mut Sequence,
        language: &Language,
        declared: &[Arc<Vec<(String, String)>>],
        base: &[ExpectedException],
        skeletons: usize,
    ) -> Vec<ExpectedException> {
        let mut head: Vec<ExpectedException> = base.to_vec();
        let edits = seq.below(2 * base.len() + 3);
        for _ in 0..edits {
            let at = (!head.is_empty()).then(|| seq.below(head.len()));
            match (seq.below(9), at) {
                (0, Some(at)) => {
                    let to = seq.below(head.len());
                    head.swap(at, to);
                }
                (1, Some(at)) => {
                    head.remove(at);
                }
                (2, _) => {
                    let new = drawn(seq, language, declared, 1, skeletons).remove(0);
                    let at = seq.below(head.len() + 1);
                    head.insert(at, new);
                }
                (3, Some(at)) => head[at].skeleton = format!("s{}", seq.below(skeletons)),
                (4, Some(at)) => {
                    head[at].exception_type = seq.one_of(language.types).map(str::to_string)
                }
                (5, Some(at)) => head[at].matcher = seq.one_of(MATCHERS).map(str::to_string),
                (6, Some(at)) => head[at].kind = seq.one_of(language.kinds).to_string(),
                (7, Some(at)) => {
                    let again = head[at].clone();
                    head.insert(at, again);
                }
                (8, Some(at)) => {
                    head[at].whole_message = !head[at].whole_message;
                    head[at].form = seq.one_of(language.forms);
                }
                _ => {}
            }
        }
        if seq.one_in(8) {
            head.reverse();
        }
        head
    }

    /// The class declarations a side is read with: none, a chain of two, the same chain
    /// kept in another list, and another parent for the first class.
    fn declarations() -> Vec<Arc<Vec<(String, String)>>> {
        let pair = |child: &str, parent: &str| (child.to_string(), parent.to_string());
        let chain = vec![
            pair("OrderError", "AppError"),
            pair("AppError", "Exception"),
        ];
        vec![
            Arc::new(Vec::new()),
            Arc::new(chain.clone()),
            Arc::new(chain),
            Arc::new(vec![pair("OrderError", "LookupError")]),
        ]
    }

    /// What the two pairings return for one base and one head side, asserted equal.
    fn same_as_reference(base: &[ExpectedException], head: &[ExpectedException]) -> Vec<Widened> {
        let (got, wanted) = (widened(base, head), reference::widened(base, head));
        assert_eq!(got, wanted, "base {base:#?}\nhead {head:#?}");
        // With no room to keep what a search passes, and with room for a few.
        for room in [0, 3] {
            assert_eq!(
                paired(base, head, room),
                wanted,
                "room {room}\nbase {base:#?}\nhead {head:#?}"
            );
        }
        got
    }

    /// The pairing returns what the first one written returns, finding for finding and
    /// in the same order, for 60,000 pairs of lists drawn from a fixed sequence.
    #[test]
    fn the_pairing_returns_what_the_first_pairing_written_returns() {
        const CASES: usize = 60_000;
        let declared = declarations();
        let mut seq = Sequence(0x0672);
        // What the cases reach, so that a sequence that stops reaching it is noticed.
        let (mut reported, mut dropped, mut against, mut one_empty) = (0, 0, 0, 0);
        for case in 0..CASES {
            let language = &LANGUAGES[seq.below(LANGUAGES.len())];
            let len = match case % 50 {
                0 => 20 + seq.below(30),
                1..=9 => 6 + seq.below(10),
                _ => seq.below(7),
            };
            let skeletons = seq.one_of(&[1, 2, 3, 5, 9, 40]);
            let base = drawn(&mut seq, language, &declared, len, skeletons);
            let head = if seq.one_in(5) {
                let len = seq.below(len + 3);
                drawn(&mut seq, language, &declared, len, skeletons)
            } else {
                edited(&mut seq, language, &declared, &base, skeletons)
            };
            let got = same_as_reference(&base, &head);
            reported += usize::from(!got.is_empty());
            dropped += usize::from(got.iter().any(|w| w.dropped));
            against += usize::from(got.iter().any(|w| !w.dropped));
            one_empty += usize::from(base.is_empty() || head.is_empty());
            // The same lists the other way round are another case.
            same_as_reference(&head, &base);
        }
        assert!(reported > CASES / 4, "{reported} cases with a finding");
        assert!(
            dropped > CASES / 20,
            "{dropped} cases with a dropped expectation"
        );
        assert!(
            against > CASES / 20,
            "{against} cases with a wider expectation"
        );
        assert!(
            one_empty > CASES / 100,
            "{one_empty} cases with an empty side"
        );
    }

    /// One expectation of a generated list: `call(i)` guarded by `pytest.raises(class)`.
    fn raises(i: usize, call: &str, class: &str) -> ExpectedException {
        ExpectedException {
            line: i + 1,
            skeleton: format!("with pytest.raises(#): {call}({i})"),
            kind: "pytest.raises".to_string(),
            exception_type: Some(class.to_string()),
            ..Default::default()
        }
    }

    /// The lists of `n` expectations the scaling tests and the measurements use, by
    /// name: the base side, and the head side.
    type Shape = (&'static str, fn(usize) -> Vec<ExpectedException>);
    const BASE: fn(usize) -> Vec<ExpectedException> =
        |n| (0..n).map(|i| raises(i, "f", "ValueError")).collect();
    const SHAPES: &[Shape] = &[
        ("unchanged", BASE),
        ("in another order", |n| BASE(n).into_iter().rev().collect()),
        ("each wider in place", |n| {
            (0..n).map(|i| raises(i, "f", "Exception")).collect()
        }),
        ("each rewritten", |n| {
            (0..n).map(|i| raises(i, "g", "ValueError")).collect()
        }),
        ("each rewritten and wider", |n| {
            (0..n).map(|i| raises(i, "g", "Exception")).collect()
        }),
        ("each rewritten, every other one wider", |n| {
            (0..n)
                .map(|i| {
                    raises(
                        i,
                        "g",
                        if i % 2 == 0 {
                            "ValueError"
                        } else {
                            "Exception"
                        },
                    )
                })
                .collect()
        }),
        ("each rewritten, half of them gone", |n| {
            (0..n / 2).map(|i| raises(i, "g", "ValueError")).collect()
        }),
        ("each rewritten as one of two classes", |n| {
            (0..n)
                .map(|i| raises(i, "g", if i % 2 == 0 { "ValueError" } else { "KeyError" }))
                .collect()
        }),
    ];

    /// The generated lists are paired as the first pairing written pairs them, at every
    /// length up to 40 and at 120, each shape against the base side and against every
    /// other shape.
    #[test]
    fn generated_lists_are_paired_as_the_first_pairing_written_pairs_them() {
        for n in (0..=40).chain([120]) {
            for (_, base) in SHAPES {
                for (_, head) in SHAPES {
                    same_as_reference(&base(n), &head(n));
                }
            }
        }
    }

    /// Sources of `n` sites in one test, for each language the comparison reads: a path,
    /// what stands before and after the sites, and one site calling `call(i)` and
    /// expecting the narrow class or the wide one.
    type Sites = (
        &'static str,
        &'static str,
        &'static str,
        fn(usize, &str, bool) -> String,
    );
    const SOURCES: &[Sites] = &[
        ("tests/test_m.py", "def test_t():\n", "", |i, call, wide| {
            let class = if wide { "Exception" } else { "ValueError" };
            format!("    with pytest.raises({class}):\n        {call}({i})\n")
        }),
        ("tests/m.test.js", "test('t', () => {\n", "});\n", |i, call, wide| {
            let class = if wide { "Error" } else { "TypeError" };
            format!("  expect(() => {call}({i})).toThrow({class});\n")
        }),
        (
            "src/test/java/MTest.java",
            "class MTest {\n    @Test\n    void t() {\n",
            "    }\n}\n",
            |i, call, wide| {
                let class = if wide { "RuntimeException" } else { "IllegalArgumentException" };
                format!("        assertThrows({class}.class, () -> {call}({i}));\n")
            },
        ),
        (
            "src/test/kotlin/MTest.kt",
            "class MTest {\n    @Test\n    fun t() {\n",
            "    }\n}\n",
            |i, call, wide| {
                let class = if wide { "RuntimeException" } else { "IllegalArgumentException" };
                format!("        assertFailsWith<{class}> {{ {call}({i}) }}\n")
            },
        ),
        (
            "tests/MTests.cs",
            "using Xunit;\npublic class MTests {\n    [Fact]\n    public void T() {\n",
            "    }\n}\n",
            |i, call, wide| {
                let class = if wide { "ArgumentException" } else { "ArgumentNullException" };
                format!("        Assert.ThrowsAny<{class}>(() => {call}({i}));\n")
            },
        ),
        (
            "tests/MTest.php",
            "<?php\nclass MTest extends TestCase {\n    public function testT(): void {\n",
            "    }\n}\n",
            |i, call, wide| {
                let class = if wide { "Exception" } else { "LogicException" };
                format!("        $this->expectException({class}::class);\n        {call}({i});\n")
            },
        ),
        (
            "spec/m_spec.rb",
            "RSpec.describe M do\n  it \"t\" do\n",
            "  end\nend\n",
            |i, call, wide| {
                let class = if wide { "StandardError" } else { "ArgumentError" };
                format!("    expect {{ {call}({i}) }}.to raise_error({class})\n")
            },
        ),
        (
            "tests/m_test.cpp",
            "#include <gtest/gtest.h>\nTEST(M, T) {\n",
            "}\n",
            |i, call, wide| {
                let class = if wide { "std::exception" } else { "std::out_of_range" };
                format!("  EXPECT_THROW({call}({i}), {class});\n")
            },
        ),
        (
            "m_test.go",
            "package m\n\nimport (\n\t\"testing\"\n\n\t\"github.com/stretchr/testify/require\"\n)\n\nfunc TestT(t *testing.T) {\n",
            "}\n",
            |i, call, wide| {
                if wide {
                    format!("\trequire.Error(t, {call}({i}))\n")
                } else {
                    format!("\trequire.ErrorIs(t, {call}({i}), ErrNotFound)\n")
                }
            },
        ),
        (
            "Tests/MTests/MTests.swift",
            "import XCTest\n\nfinal class MTests: XCTestCase {\n    func testT() {\n",
            "    }\n}\n",
            |i, call, wide| {
                if wide {
                    format!("        XCTAssertThrowsError(try {call}({i}))\n")
                } else {
                    format!("        XCTAssertThrowsError(try {call}({i})) {{ error in\n            XCTAssertTrue(error is MError)\n        }}\n")
                }
            },
        ),
        (
            "src/test/scala/MSpec.scala",
            "class MSpec extends AnyFunSuite {\n  test(\"t\") {\n",
            "  }\n}\n",
            |i, call, wide| {
                let class = if wide { "RuntimeException" } else { "IllegalArgumentException" };
                format!("    intercept[{class}] {{ {call}({i}) }}\n")
            },
        ),
        (
            "Tests/MTests.m",
            "@interface MTests : XCTestCase\n@end\n\n@implementation MTests\n- (void)testT {\n",
            "}\n@end\n",
            |i, call, wide| {
                if wide {
                    format!("    XCTAssertThrows([m {call}:{i}]);\n")
                } else {
                    format!("    XCTAssertThrowsSpecific([m {call}:{i}], MException);\n")
                }
            },
        ),
    ];

    /// The expectations of the one test of a source of `n` sites: site `i` calls `g` when
    /// `renamed(i)` and expects the wide class when `wide(i)`.
    fn read_sites(
        (path, before, after, site): &Sites,
        n: usize,
        renamed: fn(usize) -> bool,
        wide: fn(usize) -> bool,
    ) -> Vec<ExpectedException> {
        let sites: String = (0..n)
            .map(|i| site(i, if renamed(i) { "g" } else { "f" }, wide(i)))
            .collect();
        let src = format!("{before}{sites}{after}");
        let registry = default_registry();
        let pack = registry.find_pack(path).expect("a pack for the path");
        let facts = pack
            .extract(path, &src, &AssertVocabulary::default())
            .unwrap_or_else(|e| panic!("{path} is read: {e:#}"));
        assert_eq!(facts.tests.len(), 1, "{path}");
        facts.tests[0].expected_exceptions.clone()
    }

    /// The expectations each language's reader records for a test of many sites are
    /// paired as the first pairing written pairs them: unchanged, rewritten, wider, and
    /// some of each.
    #[test]
    fn expectations_read_from_sources_are_paired_as_the_first_pairing_written_pairs_them() {
        type Edit = (fn(usize) -> bool, fn(usize) -> bool);
        let edits: [Edit; 6] = [
            (|_| false, |_| false),
            (|_| false, |_| true),
            (|_| true, |_| false),
            (|_| true, |_| true),
            (|i| i % 2 == 0, |i| i % 3 == 0),
            (|i| i % 3 != 0, |i| i % 4 == 1),
        ];
        for source in SOURCES {
            let base = read_sites(source, 12, |_| false, |_| false);
            assert_eq!(base.len(), 12, "{}: {base:#?}", source.0);
            let mut reported = 0;
            for (renamed, wide) in edits {
                let head = read_sites(source, 12, renamed, wide);
                reported += same_as_reference(&base, &head).len();
                same_as_reference(&head, &base);
                // A site more on one side, and one fewer.
                same_as_reference(&base[1..], &head);
                same_as_reference(&base, &head[..7]);
            }
            assert!(reported > 0, "{}: nothing is reported", source.0);
        }
    }

    /// The steps one pairing counts.
    fn steps_of(base: &[ExpectedException], head: &[ExpectedException]) -> u64 {
        ancestry::steps(|| widened(base, head)).1
    }

    /// Pairing the expectations of a test costs steps in proportion to their number.
    /// Before #672 every base expectation was compared with every head expectation, and
    /// the rewritten ones again at each step of each search. Counted in comparisons
    /// alone, for 40 expectations and for 160: each rewritten, 820 and 12,880; each
    /// rewritten and wider, 1,640 and 25,760; each rewritten and every other one wider,
    /// 10,360 and 613,440.
    #[test]
    fn pairing_costs_steps_in_proportion_to_the_expectations() {
        for (name, head) in SHAPES {
            let (few, many) = (
                steps_of(&BASE(40), &head(40)),
                steps_of(&BASE(160), &head(160)),
            );
            assert!(
                few > 0 && many < 5 * few,
                "{name}: {few} steps for 40, {many} for 160"
            );
        }
    }

    /// A way of writing a base expectation whose search found nothing is not searched
    /// again: with no room to keep what a search passes, the expectations that are each
    /// rewritten and wider still cost steps in proportion to their number.
    #[test]
    fn a_way_whose_search_found_nothing_is_not_searched_again() {
        let steps = |n: usize| {
            let head: Vec<_> = (0..n).map(|i| raises(i, "g", "Exception")).collect();
            ancestry::steps(|| paired(&BASE(n), &head, 0)).1
        };
        let (few, many) = (steps(40), steps(160));
        assert!(
            few > 0 && many < 5 * few,
            "{few} steps for 40, {many} for 160"
        );
    }

    /// Two expectations are compared once for each pair of ways of writing them, however
    /// many are written each way.
    #[test]
    fn expectations_written_alike_are_compared_once() {
        let compared = |n: usize| {
            let head: Vec<_> = (0..n).map(|i| raises(i, "g", "Exception")).collect();
            ancestry::steps(|| {
                let (base, head): (Vec<_>, Vec<_>) = (BASE(n), head);
                let (b, h): (Vec<&_>, Vec<&_>) = (base.iter().collect(), head.iter().collect());
                let mut matching = Matching::new(&b, &h, 0);
                for i in 0..n {
                    matching.give(i, i + 1);
                }
                matching.covers.len()
            })
            .0
        };
        assert_eq!((compared(3), compared(300)), (1, 1));
    }

    fn lines(found: &[Widened]) -> Vec<(usize, &str, bool)> {
        found
            .iter()
            .map(|w| (w.line, w.skeleton.as_str(), w.dropped))
            .collect()
    }

    /// Which expectation is reported follows from the order of the two sides: a base
    /// expectation takes the first head expectation that covers it, an earlier one is
    /// moved to another when it has one, and the one left without is the last written.
    #[test]
    fn the_order_of_the_two_sides_decides_which_expectation_is_reported() {
        let site = |line: usize, skeleton: &str, class: &str| ExpectedException {
            line,
            skeleton: skeleton.to_string(),
            kind: "pytest.raises".to_string(),
            exception_type: Some(class.to_string()),
            ..Default::default()
        };
        let (lookup, key, other) = (
            site(1, "b", "LookupError"),
            site(2, "a", "KeyError"),
            site(3, "c", "KeyError"),
        );
        let (narrow, wide) = (site(5, "y", "KeyError"), site(6, "x", "LookupError"));
        // `b` takes `y`, the first that covers it; `a` is covered by `y` only, so `b`
        // is moved to `x`.
        let found = same_as_reference(
            &[lookup.clone(), key.clone()],
            &[narrow.clone(), wide.clone()],
        );
        assert_eq!(lines(&found), vec![]);
        // A third base expectation has no head expectation left: it is the one
        // reported, as dropped.
        let found = same_as_reference(
            &[lookup.clone(), key.clone(), other.clone()],
            &[narrow.clone(), wide.clone()],
        );
        assert_eq!(lines(&found), vec![(0, "c", true)]);
        assert_eq!(
            found[0].detail,
            "expected exception `KeyError` is no longer checked"
        );
        // Two that only `y` covers: the first written keeps it, and the second is
        // reported against the head expectation left, which accepts more.
        let found = same_as_reference(
            &[key.clone(), other.clone()],
            &[wide.clone(), narrow.clone()],
        );
        assert_eq!(lines(&found), vec![(6, "x", false)]);
        assert_eq!(
            found[0].detail,
            "expected exception type widened from `KeyError` to `LookupError`"
        );
        // With `b` first, `y` goes to `a`, `x` to `b`, and `c` is left without.
        let found = same_as_reference(
            &[lookup, key, other],
            &[wide, narrow, site(7, "z", "Exception")],
        );
        assert_eq!(lines(&found), vec![(7, "z", false)]);
        assert_eq!(
            found[0].detail,
            "expected exception type widened from `KeyError` to `Exception`"
        );
    }

    /// Places kept as runs: the place after a run, as places are added and taken away.
    #[test]
    fn runs_of_places_answer_the_place_after_each_run() {
        let mut seq = Sequence(7);
        let mut runs = Runs::default();
        let mut held = [false; 24];
        for _ in 0..4_000 {
            let at = seq.below(held.len());
            if seq.one_in(2) {
                // The pairing adds a place that is not held, and takes away one that is.
                if !held[at] {
                    runs.insert(at);
                    held[at] = true;
                }
            } else if held[at] {
                runs.remove(at);
                held[at] = false;
            }
            for (from, is_held) in held.iter().enumerate() {
                let after = held[from..]
                    .iter()
                    .position(|h| !h)
                    .map_or(held.len(), |more| from + more);
                assert_eq!(
                    runs.after(from),
                    is_held.then_some(after),
                    "{held:?} at {from}"
                );
            }
        }
    }
}
