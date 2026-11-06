//! One step of a pattern, walked from one record, and a chain of steps.
//!
//! A step walks one relation in one direction for `min..=max` hops and
//! reports every record it reaches whose depth (the fewest hops) falls in
//! that range. The start is reported at depth 0 when `min` is 0, else only
//! if a cycle brings the walk back to it. `neighbors` is the one-hop step;
//! `traverse` chains steps, each walked from every record the previous one
//! kept, `tree_each` walks a tree of them, each step from the record a
//! name before it bound (080), `spanned` answers the graph a chain walks,
//! and `reach` what each step reaches, each record once.

use std::collections::{BTreeMap, BTreeSet};
use std::slice;

use lakelands_api::query::{Depth, Edge, Step, Walk};
use lakelands_api::{RelationName, Result};

use super::subgraph::page;
use super::{EdgeId, EdgeSource, Links, Node, PAGE, Subgraph, check_depth, edge_id, too_large};

/// Each record a walk reached, but its start, with the record it was
/// reached from and the link it came by.
type Routes = BTreeMap<Node, (Node, Edge)>;

/// A record a walk reached, and how many hops it took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reached {
    /// The record.
    pub node: Node,
    /// The fewest hops that reach it from the walk's start.
    pub depth: u32,
}

/// What a chain of steps found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Traversal {
    /// One binding per match: the record each step reached, in step
    /// order. A chain of no steps has one empty binding, the start alone.
    pub bindings: Vec<Vec<Reached>>,
    /// Whether the cap on bindings cut the answer short.
    pub truncated: bool,
}

/// How a step of a tree stands to the steps before it
/// (`docs/decisions/080-a-pattern-may-branch-and-join.md`). A slot is
/// what a row binds: 0 the start, `j + 1` the record step `j` reached.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shape {
    /// The slot the step walks from, any before its own.
    pub from: usize,
    /// For a join, the slot whose record the step must reach.
    pub join: Option<usize>,
    /// The step and every step from it, or joining it, match whole or
    /// are bound to null whole.
    pub optional: bool,
    /// The step holds only where it has no match.
    pub absent: bool,
}

impl Shape {
    /// The shapes of a chain of `n` steps: each from the one before.
    pub fn chain(n: usize) -> Vec<Shape> {
        (0..n)
            .map(|from| Shape {
                from,
                ..Shape::default()
            })
            .collect()
    }

    /// Whether the step binds the record it reaches.
    fn binds(&self) -> bool {
        self.join.is_none() && !self.absent
    }
}

/// Records at their depths, in `(depth, type, key)` order.
fn ordered(at: impl IntoIterator<Item = (Node, u32)>) -> Vec<Reached> {
    let mut out: Vec<Reached> = (at.into_iter())
        .map(|(node, depth)| Reached { node, depth })
        .collect();
    out.sort_by(|a, b| (a.depth, &a.node).cmp(&(b.depth, &b.node)));
    out
}

/// The records one hop away from `from` over one relation.
///
/// Ordered by `(type, key)`; every record carries depth 1.
pub fn neighbors<S: EdgeSource + ?Sized>(
    source: &S,
    from: &Node,
    relation: &RelationName,
    walk: Walk,
) -> Result<Vec<Reached>> {
    let one = Depth { min: 1, max: 1 };
    let walked = walk_step(
        &mut Links::new(source),
        slice::from_ref(from),
        (relation, walk, one),
        None,
        None,
        None,
    );
    Ok(walked?.remove(0))
}

/// The records one step reaches from `from`, walking `step.relation` in
/// the direction `step.walk` for `step.depth` hops.
///
/// Ordered by `(depth, type, key)`. Refused as invalid when the depth
/// range is empty or goes past `max_depth`, the lake's ceiling. The
/// step's `type` and `where` are not applied here: the type is a
/// declaration the planner checks, and the filter is the caller's.
pub fn step<S: EdgeSource + ?Sized>(
    source: &S,
    from: &Node,
    step: &Step,
    max_depth: u32,
) -> Result<Vec<Reached>> {
    check_depth(step.depth, max_depth)?;
    let walk = (&step.relation, step.walk, step.depth);
    Ok(walk_step(
        &mut Links::new(source),
        slice::from_ref(from),
        walk,
        None,
        None,
        None,
    )?
    .remove(0))
}

/// Walk a chain of steps from `from`: each step is walked from every
/// record the previous one kept.
///
/// `keep(i, node)` decides whether a record the `i`-th step reached is
/// kept; that is where the caller applies the step's `where` and its own
/// visibility rules. A dropped record is not walked from.
///
/// Bindings come depth-first, each step's records in `(depth, type, key)`
/// order. At most `max_bindings` are returned; `truncated` says whether
/// more were found. Refused as invalid, before any walking, when any
/// step's depth range is empty or goes past `max_depth`.
pub fn traverse<S, K>(
    source: &S,
    from: &Node,
    steps: &[Step],
    max_depth: u32,
    max_bindings: usize,
    keep: K,
) -> Result<Traversal>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    let shapes = Shape::chain(steps.len());
    let mut chain = Chain::new(
        source,
        (steps, &shapes),
        max_depth,
        max_bindings,
        keep,
        None,
    )?;
    chain.prime(slice::from_ref(from))?;
    chain.extend(0, from, &mut Vec::new());
    Ok(chain.out)
}

/// Walk a tree of steps from each of `starts`, each step from the slot
/// its [`Shape`] names, and hand each match to `each(start, binding)`, in
/// the starts' order: one entry a step, the record it binds, or none
/// where it binds no name or is bound to null. A [`PAGE`] of starts is
/// walked together, each hop asked for the whole page at once, so a page
/// costs a few questions of the source, not a few a start, and each
/// step's records are held once whatever the rows made of them.
///
/// `keep(i, node)` decides, as in [`traverse`], whether step `i` keeps a
/// record it reached and walks on from it; `fits(i, start, prefix, r)`
/// whether a kept one holds beside what the steps before it bound
/// (`prefix`, one entry a step). A join holds where it reaches its
/// slot's record, an absent step where nothing it keeps holds. An
/// optional branch, the optional steps joined by `from` and `join`,
/// matches whole, else binds null whole where it has no whole match.
/// Matches come in the order of the steps, each step's records by
/// `(depth, type, key)` and null after them. `each` answers whether to
/// go on; the rest of that page is walked and not handed out.
pub fn tree_each<S, K, F, E>(
    source: &S,
    starts: &[Node],
    (steps, shapes): (&[Step], &[Shape]),
    max_depth: u32,
    (keep, mut fits): (K, F),
    mut each: E,
) -> Result<()>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
    F: FnMut(usize, &Node, &[Option<Reached>], &Reached) -> Result<bool>,
    E: FnMut(&Node, &[Option<Reached>]) -> Result<bool>,
{
    let mut chain = Chain::new(source, (steps, shapes), max_depth, usize::MAX, keep, None)?;
    let mut tree = Tree::new(shapes);
    for page in starts.chunks(PAGE) {
        chain.prime(page)?;
        for start in page {
            let mut bound = vec![None; steps.len() + 1];
            bound[0] = Some(Reached {
                node: start.clone(),
                depth: 0,
            });
            let mut on = |b: &[Option<Reached>]| each(start, b);
            if !tree.grow(&chain.memo, 0, &mut bound, &mut fits, &mut on)? {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// The optional branches of a tree and which are bound to null.
struct Tree<'s> {
    shapes: &'s [Shape],
    /// Each step's branch, where it is optional.
    branch: Vec<Option<usize>>,
    /// Each branch's steps, in their order.
    steps: Vec<Vec<usize>>,
    null: Vec<bool>,
}

/// What [`Tree`] hands a kept record to: `fits`, then `each`.
type Fits<'f> = dyn FnMut(usize, &Node, &[Option<Reached>], &Reached) -> Result<bool> + 'f;
type Each<'f> = dyn FnMut(&[Option<Reached>]) -> Result<bool> + 'f;

impl<'s> Tree<'s> {
    fn new(shapes: &'s [Shape]) -> Self {
        // An optional step is one branch with every optional step whose
        // slot it walks from or joins.
        let mut root: Vec<usize> = (0..shapes.len()).collect();
        fn find(root: &mut [usize], i: usize) -> usize {
            let r = root[i];
            if r == i { i } else { find(root, r) }
        }
        for (i, s) in shapes.iter().enumerate().filter(|(_, s)| s.optional) {
            for slot in std::iter::once(s.from).chain(s.join) {
                if slot > 0 && shapes[slot - 1].optional {
                    let (a, b) = (find(&mut root, i), find(&mut root, slot - 1));
                    root[a.max(b)] = a.min(b);
                }
            }
        }
        let (mut branch, mut steps, mut of) = (Vec::new(), Vec::new(), BTreeMap::new());
        for (i, s) in shapes.iter().enumerate() {
            branch.push(s.optional.then(|| {
                let next = of.len();
                let b = *of.entry(find(&mut root, i)).or_insert(next);
                if b == steps.len() {
                    steps.push(Vec::new());
                }
                steps[b].push(i);
                b
            }));
        }
        let null = vec![false; steps.len()];
        Tree {
            shapes,
            branch,
            steps,
            null,
        }
    }

    /// Every match of steps `i..` beside `bound` (slot 0 the start), to
    /// `each`; false once it says to stop. A branch is tried matched at
    /// its first step, then bound to null.
    fn grow(
        &mut self,
        memo: &Memo,
        i: usize,
        bound: &mut Vec<Option<Reached>>,
        fits: &mut Fits<'_>,
        each: &mut Each<'_>,
    ) -> Result<bool> {
        if i == self.shapes.len() {
            return each(&bound[1..]);
        }
        let Some(b) = self.branch[i].filter(|b| self.steps[*b][0] == i) else {
            return self.step(memo, i, bound, fits, each);
        };
        if !self.step(memo, i, bound, fits, each)? {
            return Ok(false);
        }
        self.null[b] = true;
        let more = self.step(memo, i, bound, fits, each);
        self.null[b] = false;
        more
    }

    fn step(
        &mut self,
        memo: &Memo,
        i: usize,
        bound: &mut Vec<Option<Reached>>,
        fits: &mut Fits<'_>,
        each: &mut Each<'_>,
    ) -> Result<bool> {
        let null = self.branch[i].filter(|b| self.null[*b]);
        let options = match null {
            // At its last step all a null branch reads is bound: it is
            // null only where it has no whole match.
            Some(b) if *self.steps[b].last().expect("a step") == i => {
                match self.matches(memo, b, 0, bound, fits)? {
                    true => Vec::new(),
                    false => vec![None],
                }
            }
            Some(_) => vec![None],
            None => self.options(memo, i, bound, fits)?,
        };
        for o in options {
            bound[i + 1] = o;
            if !self.grow(memo, i + 1, bound, fits, each)? {
                return Ok(false);
            }
        }
        bound[i + 1] = None;
        Ok(true)
    }

    /// Whether branch `b`'s steps from its `k`-th have a whole match
    /// beside `bound`, which is left as it was.
    fn matches(
        &self,
        memo: &Memo,
        b: usize,
        k: usize,
        bound: &mut Vec<Option<Reached>>,
        fits: &mut Fits<'_>,
    ) -> Result<bool> {
        let Some(&i) = self.steps[b].get(k) else {
            return Ok(true);
        };
        for o in self.options(memo, i, bound, fits)? {
            let was = std::mem::replace(&mut bound[i + 1], o);
            let whole = self.matches(memo, b, k + 1, bound, fits);
            bound[i + 1] = was;
            if whole? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// What step `i` may bind beside `bound`: each record it keeps that
    /// fits, or for a join and an absent step, nothing where it holds.
    fn options(
        &self,
        memo: &Memo,
        i: usize,
        bound: &[Option<Reached>],
        fits: &mut Fits<'_>,
    ) -> Result<Vec<Option<Reached>>> {
        let s = self.shapes[i];
        let Some(from) = &bound[s.from] else {
            return Ok(Vec::new());
        };
        let start = &bound[0].as_ref().expect("the start is bound").node;
        let joined = s.join.map(|j| bound[j].as_ref().map(|r| &r.node));
        let mut held = Vec::new();
        for r in memo.get(&(i, from.node.clone())).into_iter().flatten() {
            if joined.is_some_and(|j| j != Some(&r.node)) {
                continue;
            }
            if fits(i, start, &bound[1..=i], r)? {
                held.push(Some(r.clone()));
                if !s.binds() {
                    break;
                }
            }
        }
        Ok(match (s.absent, s.join.is_some()) {
            (true, _) => held.is_empty().then_some(None).into_iter().collect(),
            (false, true) => held.into_iter().map(|_| None).collect(),
            (false, false) => held,
        })
    }
}

/// The graph a chain of steps walks from each of `starts`: the starts,
/// whether or not a match begins there, every record a match binds, the
/// records each was walked through (below its step's `min`, or dropped by
/// `keep`), and the links walked between them. Each record is at the
/// fewest hops any match reaches it in. Walked as [`tree_each`] walks
/// a chain.
///
/// One page of it, as [`subgraph`](super::subgraph()) pages: the records at
/// `offset..offset + limit` of the order `(depth, type, key)` and the links
/// between a record of the page and one of this or an earlier page. The
/// whole chain is walked for every page. Refused `too_large` once what it
/// holds (the starts, and every record each walk reaches, counted as it
/// grows) numbers more than `max_nodes`.
pub fn spanned<S, K>(
    source: &S,
    starts: &[Node],
    steps: &[Step],
    (max_depth, max_nodes): (u32, u32),
    offset: usize,
    limit: usize,
    keep: K,
) -> Result<Subgraph>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    let mut span = Span::default();
    for start in starts {
        span.reach(start, 0);
    }
    if starts.len() > max_nodes as usize {
        return Err(too_large(max_nodes));
    }
    let shapes = Shape::chain(steps.len());
    let mut chain = Chain::new(
        source,
        (steps, &shapes),
        max_depth,
        usize::MAX,
        keep,
        Some(span),
    )?;
    chain.held = Some((starts.len(), max_nodes));
    for page in starts.chunks(PAGE) {
        chain.prime(page)?;
        for start in page {
            chain.from = start.clone();
            chain.extend(0, start, &mut Vec::new());
        }
    }
    let span = chain.span.take().expect("a span was asked for");
    Ok(page(ordered(span.depth_of), span.edges, offset, limit))
}

/// What each step of a chain reaches from `starts`, a step at a time
/// (lakelands#991): step `i` walks from every record step `i - 1` kept (the
/// starts for the first); `keep` applies as in [`traverse`]. Each record is
/// handed to `each(step, ..)` once, at the first step (from 1) that kept
/// it, with that step's fewest hops from any record it walked from, in
/// `(depth, type, key)` order. A start is handed out only where a step
/// keeps it. `each` answers whether to go on; later steps are not walked.
/// Refused `too_large` once what it holds (the records listed, a step's
/// records so far, and each walk from a page of sources apart, counted as
/// it grows) number more than `max_nodes`; refused as invalid, before any
/// walking, when a step's depth range is empty or goes past `max_depth`.
pub fn reach<S, K, E>(
    source: &S,
    starts: &[Node],
    steps: &[Step],
    (max_depth, max_nodes): (u32, u32),
    mut keep: K,
    mut each: E,
) -> Result<()>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
    E: FnMut(usize, Vec<Reached>) -> Result<bool>,
{
    for s in steps {
        check_depth(s.depth, max_depth)?;
    }
    let mut links = Links::new(source);
    let (mut at, mut listed) = (starts.to_vec(), BTreeSet::new());
    for (i, s) in steps.iter().enumerate() {
        if at.is_empty() {
            break;
        }
        // Each record at the fewest hops any record walked from reaches it
        // in its step's range, as a chain finds it; a page of them at once.
        let mut best: BTreeMap<Node, u32> = BTreeMap::new();
        for page in at.chunks(PAGE) {
            let walk = (&s.relation, s.walk, s.depth);
            let bound = Some((best.len() + listed.len(), max_nodes));
            for r in walk_step(&mut links, page, walk, Some(i), None, bound)?
                .into_iter()
                .flatten()
            {
                let d = best.entry(r.node).or_insert(r.depth);
                *d = (*d).min(r.depth);
            }
        }
        let mut kept = Vec::new();
        for reached in ordered(best) {
            if keep(i, &reached.node)? {
                kept.push(reached);
            }
        }
        at = kept.iter().map(|r| r.node.clone()).collect();
        let first: Vec<Reached> = (kept.into_iter())
            .filter(|r| listed.insert(r.node.clone()))
            .collect();
        if !each(i + 1, first)? {
            break;
        }
    }
    Ok(())
}

/// What [`spanned`] gathers from each match.
#[derive(Default)]
struct Span {
    routes: BTreeMap<(usize, Node), Routes>,
    depth_of: BTreeMap<Node, u32>,
    edges: BTreeMap<EdgeId, Edge>,
}

impl Span {
    fn reach(&mut self, node: &Node, depth: u32) {
        let d = self.depth_of.entry(node.clone()).or_insert(depth);
        *d = (*d).min(depth);
    }

    /// Take in a match from `from`: each step's record and the route to
    /// it from the record before.
    fn add(&mut self, from: &Node, binding: &[Reached]) {
        let (mut at, mut base) = (from.clone(), 0);
        for (i, r) in binding.iter().enumerate() {
            let routes = &self.routes[&(i, at.clone())];
            let mut back = Vec::new();
            let mut here = r.node.clone();
            for d in (1..=r.depth).rev() {
                let (before, edge) = routes[&here].clone();
                back.push((here, base + d, edge));
                here = before;
            }
            for (node, depth, edge) in back {
                self.reach(&node, depth);
                self.edges.entry(edge_id(&edge)).or_insert(edge);
            }
            at = r.node.clone();
            base += r.depth;
        }
    }
}

/// What step `i` kept from a record, filled by [`Chain::prime`] before
/// a binding is made, so a record reached along several bindings is
/// walked from once.
type Memo = BTreeMap<(usize, Node), Vec<Reached>>;

/// The state of one [`traverse`], [`tree_each`] or [`spanned`].
struct Chain<'a, S: EdgeSource + ?Sized, K> {
    links: Links<'a, S>,
    steps: &'a [Step],
    shapes: &'a [Shape],
    keep: K,
    memo: Memo,
    max_bindings: usize,
    out: Traversal,
    /// The start being walked from, and, for [`spanned`], what the
    /// matches span instead of the bindings.
    from: Node,
    span: Option<Span>,
    /// For [`spanned`], what its walks hold so far and the most they may.
    held: Option<(usize, u32)>,
}

impl<'a, S, K> Chain<'a, S, K>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    fn new(
        source: &'a S,
        (steps, shapes): (&'a [Step], &'a [Shape]),
        max_depth: u32,
        max_bindings: usize,
        keep: K,
        span: Option<Span>,
    ) -> Result<Self> {
        for s in steps {
            check_depth(s.depth, max_depth)?;
        }
        Ok(Chain {
            links: Links::new(source),
            steps,
            shapes,
            keep,
            memo: BTreeMap::new(),
            max_bindings,
            out: Traversal {
                bindings: Vec::new(),
                truncated: false,
            },
            from: (Default::default(), Default::default()),
            span,
            held: None,
        })
    }

    /// Extend `prefix` (a binding of steps `0..i`, ending at `at`) with
    /// every record step `i` keeps, as primed. Returns false once the cap
    /// is hit.
    fn extend(&mut self, i: usize, at: &Node, prefix: &mut Vec<Reached>) -> bool {
        if i == self.steps.len() {
            if let Some(span) = &mut self.span {
                span.add(&self.from, prefix);
                return true;
            }
            if self.out.bindings.len() >= self.max_bindings {
                self.out.truncated = true;
                return false;
            }
            self.out.bindings.push(prefix.clone());
            return true;
        }
        for reached in self.memo[&(i, at.clone())].clone() {
            let next = reached.node.clone();
            prefix.push(reached);
            let more = self.extend(i + 1, &next, prefix);
            prefix.pop();
            if !more {
                return false;
            }
        }
        true
    }

    /// Walk every step for `nodes` and what each step keeps of them, a
    /// step's records together a hop at a time, into the memo that
    /// [`Self::extend`] and a [`Tree`] read, with each walk's routes for a
    /// span: so the starts of a page ask the source together. A step is
    /// walked from every record its shape's slot holds.
    fn prime(&mut self, nodes: &[Node]) -> Result<()> {
        let mut slots = vec![BTreeSet::new(); self.steps.len() + 1];
        slots[0] = nodes.iter().cloned().collect();
        for (i, s) in self.steps.iter().enumerate() {
            let at = &slots[self.shapes[i].from];
            let todo: Vec<Node> = (at.iter())
                .filter(|n| !self.memo.contains_key(&(i, (*n).clone())))
                .cloned()
                .collect();
            let walk = (&s.relation, s.walk, s.depth);
            let mut routes = self.span.as_ref().map(|_| vec![Routes::new(); todo.len()]);
            let (links, r) = (&mut self.links, routes.as_deref_mut());
            let walked = walk_step(links, &todo, walk, Some(i), r, self.held)?;
            // Every record a walk reaches has a route; each walk counted apart.
            if let (Some((held, _)), Some(r)) = (&mut self.held, &routes) {
                *held += r.iter().map(Routes::len).sum::<usize>();
            }
            let mut routes = routes.into_iter().flatten();
            for (node, walked) in todo.into_iter().zip(walked) {
                if let (Some(span), Some(routes)) = (&mut self.span, routes.next()) {
                    span.routes.insert((i, node.clone()), routes);
                }
                let mut kept = Vec::new();
                for reached in walked {
                    if (self.keep)(i, &reached.node)? {
                        kept.push(reached);
                    }
                }
                self.memo.insert((i, node), kept);
            }
            if self.shapes[i].binds() {
                let next = at.iter().flat_map(|n| &self.memo[&(i, n.clone())]);
                slots[i + 1] = next.map(|r| r.node.clone()).collect();
            }
        }
        Ok(())
    }
}

/// Breadth-first over one relation from each of `froms` at once, a level at
/// a time, each level's links read together for every walk. A record is
/// recorded at the first level that reaches it, so its depth is the fewest
/// hops; levels are sorted sets, so nothing depends on the source's order.
/// With `routes`, one map per walk, each record reached is kept with the
/// first record and link it was reached by.
fn walk_step<S: EdgeSource + ?Sized>(
    links: &mut Links<'_, S>,
    froms: &[Node],
    (relation, walk, depth): (&RelationName, Walk, Depth),
    step: Option<usize>,
    mut routes: Option<&mut [Routes]>,
    bound: Option<(usize, u32)>,
) -> Result<Vec<Vec<Reached>>> {
    // A start is not marked reached: a cycle back to it is reported at
    // the depth it closes, which is what a range starting at 1 asks for.
    let mut best: Vec<BTreeMap<Node, u32>> = vec![BTreeMap::new(); froms.len()];
    let mut frontier: Vec<BTreeSet<Node>> =
        froms.iter().map(|f| BTreeSet::from([f.clone()])).collect();
    for hop in 1..=depth.max {
        let nears: BTreeSet<&Node> = frontier.iter().flatten().collect();
        if nears.is_empty() {
            break;
        }
        let nears: Vec<Node> = nears.into_iter().cloned().collect();
        let around = links.around(&nears, slice::from_ref(relation), walk, step)?;
        let around: BTreeMap<&Node, Vec<(Node, Edge)>> = nears.iter().zip(around).collect();
        for (w, (best, frontier)) in best.iter_mut().zip(frontier.iter_mut()).enumerate() {
            let mut next = BTreeSet::new();
            for near in frontier.iter() {
                for (other, edge) in &around[near] {
                    if !best.contains_key(other) {
                        if let Some(routes) = routes.as_deref_mut() {
                            routes[w].insert(other.clone(), (near.clone(), edge.clone()));
                        }
                        best.insert(other.clone(), hop);
                        next.insert(other.clone());
                    }
                }
            }
            *frontier = next;
        }
        // With a bound, what the walks hold beside `held` is counted as it grows.
        if let Some((held, max)) = bound
            && held + best.iter().map(BTreeMap::len).sum::<usize>() > max as usize
        {
            return Err(too_large(max));
        }
    }
    let ends = froms.iter().zip(best).map(|(from, mut best)| {
        if depth.min == 0 {
            best.insert(from.clone(), 0);
        }
        ordered(best.into_iter().filter(|(_, d)| *d >= depth.min))
    });
    Ok(ends.collect())
}

#[cfg(test)]
mod tests;
