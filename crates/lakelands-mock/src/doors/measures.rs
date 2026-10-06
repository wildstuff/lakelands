//! The whole-graph measures, as the mock answers them: degree centrality
//! and connected components over the relations a pattern's steps name,
//! by the store's rules (the far end visible to the principal, a
//! restricted link only with the grant, one order, refused past the
//! lake's `max_graph_nodes`), computed over the mock's own state.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::error::{Refusal, Result};
use lakelands_api::id::{RecordKey, RelationName, TypeName};
use lakelands_api::principal::Principal;
use lakelands_api::query::{Depth, Pattern, Projection, Walk};
use lakelands_api::read::ReadAt;
use lakelands_api::record::Value;

use super::{candidates, invalid, links, refused};
use crate::state::State;

type Node = (TypeName, RecordKey);
type EdgeId = (Node, RelationName, Node);

/// What a measure refuses, as the store does.
pub(super) fn check(p: &Pattern) -> Result<()> {
    if p.to.is_some() || !p.order_by.is_empty() || p.steps.is_empty() {
        return Err(invalid("a measure takes steps, no `to` and no `order_by`"));
    }
    for s in &p.steps {
        if s.depth != Depth::default() || s.filter.is_some() {
            return Err(invalid("a measure's steps take no `depth` and no `where`"));
        }
        if p.project == Projection::Components && s.walk != Walk::Both {
            return Err(invalid("`components` walks every step `both`"));
        }
    }
    Ok(())
}

/// The graph a principal sees at one state.
struct Seen<'a> {
    st: &'a State,
    who: &'a Principal,
    at: ReadAt,
    visible: BTreeMap<TypeName, BTreeSet<RecordKey>>,
}

impl<'a> Seen<'a> {
    fn new(st: &'a State, who: &'a Principal, at: ReadAt) -> Self {
        Seen {
            st,
            who,
            at,
            visible: BTreeMap::new(),
        }
    }

    fn is_visible(&mut self, node: &Node) -> Result<bool> {
        if !self.visible.contains_key(&node.0) {
            let keys = match self.st.visible(self.who, &node.0) {
                Some(t) => candidates(self.st, self.who, t, self.at, None)?
                    .into_iter()
                    .map(|r| r.key.clone())
                    .collect(),
                None => BTreeSet::new(),
            };
            self.visible.insert(node.0.clone(), keys);
        }
        Ok(self.visible[&node.0].contains(&node.1))
    }

    /// The links of `near` over `relation`, walked `walk`, to a far end the
    /// principal sees, each with its far end.
    fn around(
        &mut self,
        near: &Node,
        relation: &RelationName,
        walk: Walk,
    ) -> Result<Vec<(EdgeId, Node)>> {
        let mut raw: Vec<EdgeId> = Vec::new();
        let decls: Vec<_> = self
            .st
            .types
            .values()
            .flat_map(|o| {
                o.decl
                    .relations
                    .iter()
                    .filter(|r| &r.name == relation)
                    .map(|r| (o.decl.name.clone(), r.clone()))
            })
            .collect();
        for (owner, r) in decls {
            let out = matches!(walk, Walk::Out | Walk::Both) && owner == near.0;
            let inn = matches!(walk, Walk::In | Walk::Both) && r.target == near.0;
            if let Some(d) = &r.derived {
                let Some(here) = self.st.live(&near.0, &near.1) else {
                    continue;
                };
                for rec in self.st.records.values().filter(|x| !x.retired) {
                    if out && rec.ty == r.target && super::derived_match(&d.pairs, here, rec) {
                        raw.push((
                            near.clone(),
                            r.name.clone(),
                            (r.target.clone(), rec.key.clone()),
                        ));
                    }
                    if inn && rec.ty == owner && super::derived_match(&d.pairs, rec, here) {
                        raw.push((
                            (owner.clone(), rec.key.clone()),
                            r.name.clone(),
                            near.clone(),
                        ));
                    }
                }
                continue;
            }
            match &r.via {
                Some(via) => {
                    if out
                        && let Some(k) = self
                            .st
                            .live(&near.0, &near.1)
                            .and_then(|rec| rec.fields.get(via))
                            .and_then(Value::as_str)
                    {
                        raw.push((near.clone(), r.name.clone(), (r.target.clone(), k.into())));
                    }
                    if inn {
                        for rec in self.st.records.values() {
                            if rec.ty == owner
                                && !rec.retired
                                && rec.fields.get(via).and_then(Value::as_str)
                                    == Some(near.1.as_str())
                            {
                                raw.push((
                                    (owner.clone(), rec.key.clone()),
                                    r.name.clone(),
                                    near.clone(),
                                ));
                            }
                        }
                    }
                }
                None => {
                    let lt = links::name(&owner, &r.name);
                    for rec in self.st.records.values() {
                        // A link its walker's `only` does not admit is
                        // not walked (051).
                        if rec.ty != lt
                            || rec.retired
                            || (rec.restricted && !self.who.grants.restricted)
                            || !self.st.admitted(self.who, rec)
                        {
                            continue;
                        }
                        let end = |f: &str| {
                            rec.fields
                                .get(f)
                                .and_then(Value::as_str)
                                .map(RecordKey::from)
                        };
                        let (Some(from), Some(to)) = (end(links::FROM), end(links::TO)) else {
                            continue;
                        };
                        if out && from == near.1 {
                            raw.push((
                                near.clone(),
                                r.name.clone(),
                                (r.target.clone(), to.clone()),
                            ));
                        }
                        if inn && to == near.1 {
                            raw.push(((owner.clone(), from), r.name.clone(), near.clone()));
                        }
                    }
                }
            }
        }
        let mut out = Vec::new();
        for id in raw.into_iter().collect::<BTreeSet<_>>() {
            let far = if id.0 == *near {
                id.2.clone()
            } else {
                id.0.clone()
            };
            if self.is_visible(&far)? {
                out.push((id, far));
            }
        }
        Ok(out)
    }
}

/// The measure's rows, in its one order, before paging.
pub(super) fn rows(
    st: &State,
    who: &Principal,
    p: &Pattern,
    seeds: Vec<Node>,
    max_nodes: u32,
) -> Result<Vec<BTreeMap<String, Value>>> {
    let mut seen = Seen::new(st, who, p.at);
    if p.project == Projection::Degree {
        let mut scored = Vec::new();
        for node in seeds {
            let mut ids = BTreeSet::new();
            for s in &p.steps {
                for (id, _) in seen.around(&node, &s.relation, s.walk)? {
                    ids.insert(id);
                }
            }
            scored.push((std::cmp::Reverse(ids.len()), node));
        }
        scored.sort();
        return Ok(scored
            .into_iter()
            .map(|(std::cmp::Reverse(d), (ty, key))| {
                BTreeMap::from([
                    ("type".to_owned(), Value::from(ty.as_str())),
                    ("key".to_owned(), Value::from(key.as_str())),
                    ("degree".to_owned(), Value::from(d)),
                ])
            })
            .collect());
    }
    let mut member: BTreeSet<Node> = BTreeSet::new();
    let mut comps: Vec<BTreeSet<Node>> = Vec::new();
    for seed in seeds {
        if member.contains(&seed) {
            continue;
        }
        let mut comp = BTreeSet::new();
        let mut todo = vec![seed];
        while let Some(n) = todo.pop() {
            if !member.insert(n.clone()) {
                continue;
            }
            if member.len() > max_nodes as usize {
                return Err(refused(Refusal::TooLarge { max: max_nodes }));
            }
            for s in &p.steps {
                for (_, far) in seen.around(&n, &s.relation, Walk::Both)? {
                    todo.push(far);
                }
            }
            comp.insert(n);
        }
        comps.push(comp);
    }
    comps.sort_by(|a, b| {
        b.len()
            .cmp(&a.len())
            .then_with(|| a.first().cmp(&b.first()))
    });
    let mut out = Vec::new();
    for (i, c) in comps.iter().enumerate() {
        for (ty, key) in c {
            out.push(BTreeMap::from([
                ("component".to_owned(), Value::from(i)),
                ("size".to_owned(), Value::from(c.len())),
                ("type".to_owned(), Value::from(ty.as_str())),
                ("key".to_owned(), Value::from(key.as_str())),
            ]));
        }
    }
    Ok(out)
}
