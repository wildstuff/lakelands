//! The one pattern query, and search.
//!
//! A pattern starts somewhere (a type, a key, a filter or a search),
//! walks declared relations step by step, and projects the result as
//! rows, as a graph, as aggregates, or as a whole-graph measure (degree
//! centrality, connected components). `neighbors`, `traverse`, `path`
//! and `subgraph` are named special cases of it, not doors of their own;
//! a later query-language translator compiles into it. The planner knows
//! the ontology: a step over a relation its type does not declare is
//! refused, not empty, and grants apply per step because every step has a
//! type.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::Case;
use crate::expr::Condition;
use crate::id::{Cursor, FieldName, RecordKey, RelationName, TypeName};
use crate::read::{Aggregate, Order, Params, ReadAt};
use crate::record::{Record, Value};

/// Which way a step walks a relation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Walk {
    /// From the declaring type to the target.
    #[default]
    Out,
    /// From the target back to the declaring type.
    In,
    /// Either way.
    Both,
}

/// How many hops a step takes: `min..=max`. A `max` beyond the lake's
/// `max_depth`, or a `min` above `max`, is refused as invalid, never
/// silently shortened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Depth {
    /// The fewest hops.
    pub min: u32,
    /// The most hops; bounded by the lake's `max_depth`.
    pub max: u32,
}

impl Default for Depth {
    fn default() -> Self {
        Depth { min: 1, max: 1 }
    }
}

/// Where a pattern starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    /// The start's type. Only `components` may leave it out, and then
    /// grows from every record that holds a link over the steps'
    /// relations, in every type (`docs/decisions/018-one-pattern-query.md`).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeName>,
    /// One record by key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<RecordKey>,
    /// Records matching a filter.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<Condition>,
    /// Records a search finds, best first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchMode>,
    /// A second stage for `search`: the pattern starts from the reranked
    /// window, best first (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank: Option<Rerank>,
    /// The values bound to the parameters of a view the pattern starts on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: Params,
    /// The name a row binds the start's record under; its type's name
    /// when left out.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub named: Option<String>,
}

/// One step of a pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// The relation walked; it must be declared on the type in scope, or
    /// be the `inverse` of one, which walks that relation the other way.
    pub relation: RelationName,
    /// Which way.
    #[serde(default)]
    pub walk: Walk,
    /// The type reached; checked against the relation's declaration.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeName>,
    /// Which reached records to keep.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<Condition>,
    /// How many hops.
    #[serde(default)]
    pub depth: Depth,
    /// The name a row binds the reached record under; its type's name
    /// when left out.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub named: Option<String>,
    /// The name the step walks from: the start's, or one a step listed
    /// before it binds; the previous step's name when left out
    /// (`docs/decisions/080-a-pattern-may-branch-and-join.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// A name bound before it: the step holds only where it reaches that
    /// name's record, and binds no name of its own (080).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    /// The step keeps its row where it has no match, its name bound to
    /// null; it and the steps from it match whole or bind null whole (080).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    /// The step keeps its row only where it has no match, and binds no
    /// name (080).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
}

/// What a pattern returns.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "as", rename_all = "snake_case")]
pub enum Projection {
    /// One row per match: the listed paths (`invoice.amount`), or every
    /// readable field of every step when empty. For agents.
    Rows {
        /// The paths projected.
        #[serde(default)]
        fields: Vec<String>,
    },
    /// Nodes and edges. For viewers. A graph always includes the start
    /// node, the root a viewer draws from, whatever the steps' `min`;
    /// `Rows` holds a start only where its steps reach a match from it.
    /// A chain's graph is walked whole for every page, and one whose
    /// records would number more than the lake's `max_graph_nodes` is
    /// refused `too_large` (`docs/decisions/026-the-store-walks-the-graph.md`).
    Graph,
    /// Aggregates over the matches.
    Aggregate(Aggregate),
    /// Degree centrality: one row per start record, `type`, `key` and
    /// `degree`, the number of its links over the steps' relations, each
    /// in its step's direction, to a record the principal may see.
    /// Ordered by degree, highest first, then by key.
    Degree,
    /// Connected components: one row per record, `component`, `size`,
    /// `type` and `key`, for every record reached from the start records
    /// over the steps' relations taken both ways. Components are ordered
    /// by size, largest first, then by their least `(type, key)`, and
    /// numbered from 0 in that order; a component's records by
    /// `(type, key)`. With no start type, every record that holds a link it
    /// may see over the steps' relations is a start, and no record stands
    /// alone. A walk that would reach more than the lake's
    /// `max_graph_nodes` is refused `too_large`, never cut
    /// (`docs/decisions/026-the-store-walks-the-graph.md`).
    Components,
    /// What each step reaches: one row per record, `step`, `depth`,
    /// `type` and `key`. Step `i` walks from every record step `i - 1`
    /// kept, a dead end included; each record is listed once, at the
    /// first step that keeps it, with that step's fewest hops, and the
    /// start only where a step reaches it. Ordered by `(step, depth, type,
    /// key)`; a page that would walk past the lake's `max_graph_nodes` is
    /// refused `too_large` (`docs/decisions/018-one-pattern-query.md`).
    Reach,
}

impl<'de> Deserialize<'de> for Projection {
    /// Serde reads a tagged unit variant and drops the rest of its map, so
    /// a projection without members is read as an empty struct, which
    /// refuses any member beside `as` (lakelands#563).
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Projection, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, tag = "as", rename_all = "snake_case")]
        enum Wire {
            Rows {
                #[serde(default)]
                fields: Vec<String>,
            },
            Graph {},
            Aggregate(Aggregate),
            Degree {},
            Components {},
            Reach {},
        }
        Ok(match Wire::deserialize(d)? {
            Wire::Rows { fields } => Projection::Rows { fields },
            Wire::Graph {} => Projection::Graph,
            Wire::Aggregate(a) => Projection::Aggregate(a),
            Wire::Degree {} => Projection::Degree,
            Wire::Components {} => Projection::Components,
            Wire::Reach {} => Projection::Reach,
        })
    }
}

impl Default for Projection {
    fn default() -> Self {
        Projection::Rows { fields: Vec::new() }
    }
}

/// The one pattern query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    /// Where it starts.
    pub start: Start,
    /// The steps, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// If set, only the shortest walk from the start to this record
    /// (`path`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<(TypeName, RecordKey)>,
    /// What it returns.
    #[serde(default)]
    pub project: Projection,
    /// For rows alone: sort keys over the rows' paths, most significant
    /// first, each by its value: a row is no record, and a key by
    /// `latest`, `known_at` or `seen_at` is refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_by: Vec<Order>,
    /// At most this many rows or nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Continue after an earlier page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// The time axes.
    #[serde(default, flatten)]
    pub at: ReadAt,
}

impl Pattern {
    fn from(ty: TypeName, key: RecordKey, steps: Vec<Step>, project: Projection) -> Pattern {
        Pattern {
            start: Start {
                ty: Some(ty),
                key: Some(key),
                filter: None,
                search: None,
                rerank: None,
                params: Default::default(),
                named: None,
            },
            steps,
            to: None,
            project,
            order_by: Vec::new(),
            limit: None,
            cursor: None,
            at: ReadAt::default(),
        }
    }

    /// The records one hop away over one relation. A row binds one record
    /// of one type under a name: over a relation to the start's own type
    /// its caller names the step with `as`, and over one that reaches
    /// several types the step's `type`.
    pub fn neighbors(ty: TypeName, key: RecordKey, relation: RelationName, walk: Walk) -> Pattern {
        let step = Step {
            relation,
            walk,
            ty: None,
            filter: None,
            depth: Depth::default(),
            named: None,
            from: None,
            join: None,
            optional: false,
            absent: false,
        };
        Pattern::from(ty, key, vec![step], Projection::default())
    }

    /// The records reached by walking a chain of relations.
    pub fn traverse(ty: TypeName, key: RecordKey, steps: Vec<Step>) -> Pattern {
        Pattern::from(ty, key, steps, Projection::default())
    }

    /// Everything within `depth` hops over the given relations, as a
    /// graph. The steps it builds are one walk over any of the relations,
    /// not a chain: a planner reads a pattern whose projection is
    /// [`Projection::Graph`] and whose steps all start at depth 0 as one
    /// multi-relation walk, each relation in its own step's direction to
    /// its own step's `max`; any other graph pattern is a chain, walked as
    /// its rows are.
    pub fn subgraph(
        ty: TypeName,
        key: RecordKey,
        relations: Vec<RelationName>,
        depth: u32,
    ) -> Pattern {
        let steps = relations
            .into_iter()
            .map(|relation| Step {
                relation,
                walk: Walk::Both,
                ty: None,
                filter: None,
                depth: Depth { min: 0, max: depth },
                named: None,
                from: None,
                join: None,
                optional: false,
                absent: false,
            })
            .collect();
        Pattern::from(ty, key, steps, Projection::Graph)
    }

    /// The shortest walk from one record to another over the given
    /// relations, walked both ways; none given walks none.
    pub fn path(
        from: (TypeName, RecordKey),
        to: (TypeName, RecordKey),
        relations: Vec<RelationName>,
        max_depth: u32,
    ) -> Pattern {
        let mut p = Pattern::subgraph(from.0, from.1, relations, max_depth);
        p.to = Some(to);
        p
    }
}

/// The answer to a pattern query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum QueryResult {
    /// Bindings per step, one row per match.
    Rows {
        /// The rows, keyed by path.
        rows: Vec<BTreeMap<String, Value>>,
        /// Where the next page starts, if there is one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_cursor: Option<Cursor>,
    },
    /// Nodes and edges.
    Graph {
        /// The records reached.
        nodes: Vec<Record>,
        /// Each node's depth, in the order of `nodes`: the fewest hops that
        /// reach it from a start, 0 for a start; along a path, its hop.
        #[serde(default)]
        depths: Vec<u32>,
        /// The links walked.
        edges: Vec<Edge>,
        /// Whether the limit cut the graph short.
        truncated: bool,
        /// Where the next page starts, exactly when `truncated`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_cursor: Option<Cursor>,
    },
    /// Aggregates over the matches.
    Groups {
        /// One row per group.
        rows: Vec<crate::read::Group>,
    },
}

/// One link between two records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// The declaring side.
    pub from: (TypeName, RecordKey),
    /// The relation.
    pub relation: RelationName,
    /// The target side.
    pub to: (TypeName, RecordKey),
    /// The link's own fields, if the relation declares any.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<FieldName, Value>,
}

/// How a search ranks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "mode", rename_all = "snake_case")]
pub enum SearchMode {
    /// Full text over the type's searchable fields, or the ones named.
    Lexical {
        /// The words searched for.
        text: String,
        /// Match only in these searchable fields; absent, in every one the
        /// caller reads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<FieldName>>,
        /// A weight for a match of the text in each searchable field named,
        /// winning over its type's `search_weights` setting's for that
        /// field; a field it does not name takes the setting's, else 1.
        /// A field `fields` leaves out is not searched, so weighs nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_fields: Option<BTreeMap<FieldName, f64>>,
    },
    /// Nearest neighbours of a vector the caller supplies.
    Vector {
        /// The vector field searched.
        field: FieldName,
        /// The query vector.
        vector: Vec<f32>,
        /// The weight of `field`'s ranking against `also`'s; absent, 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weight: Option<f64>,
        /// Further vector fields, each ranked by its own query vector
        /// and fused with `field`'s ranking by weighted rank.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        also: Vec<VectorTerm>,
        /// The model that made the query vector, if the caller names it:
        /// over a field declared as an embedding, it must be the field's
        /// own model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Both, fused by rank. Without a vector, the store asks the lake's
    /// embedding backend for one, or falls back to lexical and says so.
    Hybrid {
        /// The words searched for.
        text: String,
        /// The vector field searched.
        field: FieldName,
        /// The query vector, if the caller has one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vector: Option<Vec<f32>>,
        /// Match the text only in these searchable fields; absent, in
        /// every one the caller reads. The vector half is not narrowed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<FieldName>>,
        /// A weight for a match of the text in each searchable field named,
        /// winning over its type's `search_weights` setting's for that
        /// field; a field it does not name takes the setting's, else 1.
        /// A field `fields` leaves out is not searched, so weighs nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_fields: Option<BTreeMap<FieldName, f64>>,
        /// The weight of the vector half; absent, 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weight: Option<f64>,
        /// The weight of the lexical half; absent, 1.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_weight: Option<f64>,
        /// The model that made the query vector, if the caller names it:
        /// over a field declared as an embedding, it must be the field's
        /// own model. It names the caller's `vector`, so without one it is
        /// refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

/// The second stage of a search: the first `top` hits of its ranking,
/// ranked again by the lake's reranker against a text
/// (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rerank {
    /// How many of the first stage's hits the stage ranks, 1 at least and
    /// at most the lake's `max_rerank_top`. The answer ends at this window.
    pub top: u32,
    /// What the window is ranked against; absent, the search's own text.
    /// A vector search has none, so it must give one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Rerank {
    /// What the stage asks of `mode` that cannot be: no window, or no text
    /// to rank against, as a blank one is none.
    pub fn misfit(&self, mode: &SearchMode) -> Option<Case> {
        match self.top {
            0 => Some(Case::RerankTopZero),
            _ if self.query(mode).trim().is_empty() => Some(Case::RerankNeedsText),
            _ => None,
        }
    }

    /// The text the window is ranked against: its own, else the search's.
    pub fn query<'a>(&'a self, mode: &'a SearchMode) -> &'a str {
        match (&self.text, mode) {
            (Some(text), _) => text,
            (_, SearchMode::Lexical { text, .. } | SearchMode::Hybrid { text, .. }) => text,
            (_, SearchMode::Vector { .. }) => "",
        }
    }
}

/// One further vector field of a vector search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VectorTerm {
    /// The vector field searched.
    pub field: FieldName,
    /// The query vector for it.
    pub vector: Vec<f32>,
    /// The weight of its ranking; absent, 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    /// The model that made its query vector, if the caller names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl SearchMode {
    /// A lexical search over every searchable field. A host that builds
    /// its search modes through these constructors, and matches them with
    /// `..`, survives a member added to a variant later.
    pub fn lexical(text: impl Into<String>) -> SearchMode {
        SearchMode::Lexical {
            text: text.into(),
            fields: None,
            text_fields: None,
        }
    }

    /// A vector search over `field`.
    pub fn vector(field: impl Into<FieldName>, vector: Vec<f32>) -> SearchMode {
        SearchMode::Vector {
            field: field.into(),
            vector,
            weight: None,
            also: Vec::new(),
            model: None,
        }
    }

    /// A hybrid search over `field`, its query vector left to the lake's
    /// embedding backend, over every searchable field.
    pub fn hybrid(text: impl Into<String>, field: impl Into<FieldName>) -> SearchMode {
        SearchMode::Hybrid {
            text: text.into(),
            field: field.into(),
            vector: None,
            fields: None,
            text_fields: None,
            weight: None,
            text_weight: None,
            model: None,
        }
    }

    /// The model each query vector the caller brings names, by its field,
    /// in the order [`SearchMode::vectors`] names them: none where the
    /// caller names none, and none for a hybrid search's vector the lake
    /// makes.
    pub fn models(&self) -> Vec<(&FieldName, Option<&str>)> {
        match self {
            SearchMode::Lexical { .. } => Vec::new(),
            SearchMode::Hybrid {
                field,
                vector: Some(_),
                model,
                ..
            } => vec![(field, model.as_deref())],
            SearchMode::Hybrid { vector: None, .. } => Vec::new(),
            SearchMode::Vector {
                field, model, also, ..
            } => std::iter::once((field, model.as_deref()))
                .chain(also.iter().map(|t| (&t.field, t.model.as_deref())))
                .collect(),
        }
    }

    /// The vector rankings the mode names, in the order they are fused:
    /// each field, its query vector (none where a hybrid search leaves it
    /// to the lake) and its weight, if it names one.
    pub fn vectors(&self) -> Vec<(&FieldName, &[f32], Option<f64>)> {
        match self {
            SearchMode::Lexical { .. } => Vec::new(),
            SearchMode::Hybrid {
                field,
                vector,
                weight,
                ..
            } => {
                vec![(field, vector.as_deref().unwrap_or(&[]), *weight)]
            }
            SearchMode::Vector {
                field,
                vector,
                weight,
                also,
                ..
            } => {
                let also = also.iter().map(|t| (&t.field, &t.vector[..], t.weight));
                std::iter::once((field, &vector[..], *weight))
                    .chain(also)
                    .collect()
            }
        }
    }

    /// What the mode asks that no ranking can do: the fault, with the
    /// field it names if any. A weight weighs one ranking against
    /// another, so it is above 0 and at most 1 000 000, a vector search
    /// of one field takes none, no field is named twice, every query
    /// vector has a direction ([`no_direction`]), and a `model` names one
    /// the caller brings.
    pub fn misfit(&self) -> Option<(Option<FieldName>, Case)> {
        let terms = self.vectors();
        let lone = matches!(self, SearchMode::Vector { also, .. } if also.is_empty());
        let fits = |w: Option<f64>| w.is_none_or(weight_fits);
        for (i, (field, vector, weight)) in terms.iter().enumerate() {
            let field = (*field).clone();
            // An empty vector is a hybrid search's that brings none, or
            // one of the wrong length, which the field's check names.
            let aimless = no_direction(vector).filter(|_| !vector.is_empty());
            let fault = if let Some(why) = aimless {
                let field = field.clone();
                Case::VectorAimless { field, why }
            } else if lone && weight.is_some() {
                Case::WeightAlone
            } else if terms[..i].iter().any(|t| *t.0 == field) {
                Case::VectorFieldTwice {
                    field: field.clone(),
                }
            } else if !fits(*weight) {
                Case::WeightRange {
                    field: field.clone(),
                }
            } else {
                continue;
            };
            return Some((Some(field), fault));
        }
        if let SearchMode::Hybrid {
            field,
            vector: None,
            model: Some(_),
            ..
        } = self
        {
            return Some((Some(field.clone()), Case::ModelWithoutVector));
        }
        let text = matches!(self, SearchMode::Hybrid { text_weight, .. } if !fits(*text_weight));
        text.then_some((None, Case::TextWeightRange))
    }
}

/// Why `vector` gives a search no direction to rank along, if it gives
/// none: a component that is not a finite number, or every one 0. Near a
/// vector of no direction is nothing, and its ranking would be every
/// record tied. A cosine of finite `f32`s summed in `f64` neither
/// overflows nor underflows, so this is the whole condition.
pub fn no_direction(vector: &[f32]) -> Option<crate::error::Aimless> {
    use crate::error::Aimless;
    if !vector.iter().all(|x| x.is_finite()) {
        Some(Aimless::NotFinite)
    } else if vector.iter().all(|&x| x == 0.0) {
        Some(Aimless::Zero)
    } else {
        None
    }
}

/// Whether `w` weighs one ranking against another: above 0 and at most
/// 1 000 000, in a search and in its type's `search_weights` alike.
pub fn weight_fits(w: f64) -> bool {
    w > 0.0 && w <= 1e6
}

/// `search`: ranked retrieval over one type. Unlike `find`, the answer
/// is ordered by relevance and every hit carries a score.
///
/// Relevance is the order by definition, so `search` takes no
/// `order_by`: `k` is its limit and `cursor` pages through the ranking,
/// bound to the position of the first page. A caller who wants the
/// matches in another order uses `find`, or a pattern query that starts
/// from a search.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Search {
    /// The type searched.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// How it ranks.
    #[serde(flatten)]
    pub mode: SearchMode,
    /// At most this many hits per page.
    #[serde(default = "default_k")]
    pub k: u32,
    /// Continue after an earlier page of the same ranking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// Only records matching this filter.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<Condition>,
    /// A second stage over the first `top` hits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank: Option<Rerank>,
    /// Return each field's provenance with its value.
    #[serde(default)]
    pub with_meta: bool,
    /// The time axes.
    #[serde(default, flatten)]
    pub at: ReadAt,
}

fn default_k() -> u32 {
    20
}

/// A search's own members as it is read, apart from its mode's: serde
/// refuses no unknown member beside a flattened enum, so each is read by
/// itself and refuses what it does not take (lakelands#563).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchWire {
    #[serde(rename = "type")]
    ty: TypeName,
    #[serde(default = "default_k")]
    k: u32,
    #[serde(default)]
    cursor: Option<Cursor>,
    #[serde(default, rename = "where")]
    filter: Option<Condition>,
    #[serde(default)]
    rerank: Option<Rerank>,
    #[serde(default)]
    with_meta: bool,
    #[serde(default, flatten)]
    at: ReadAt,
}

impl<'de> Deserialize<'de> for Search {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Search, D::Error> {
        use serde::de::Error;
        let mut own = serde_json::Map::deserialize(d)?;
        let members = [
            "mode", "text", "fields", "field", "vector", "weight", "also", "model",
        ];
        let mode = (members.iter().chain(&["text_weight", "text_fields"]))
            .filter_map(|m| own.remove_entry(*m));
        let mode =
            serde_json::from_value(Value::Object(mode.collect())).map_err(D::Error::custom)?;
        let w: SearchWire = serde_json::from_value(Value::Object(own)).map_err(D::Error::custom)?;
        let (ty, k, cursor, filter, rerank, with_meta, at) =
            (w.ty, w.k, w.cursor, w.filter, w.rerank, w.with_meta, w.at);
        Ok(Search {
            ty,
            mode,
            k,
            cursor,
            filter,
            rerank,
            with_meta,
            at,
        })
    }
}

/// The answer to `search`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hits {
    /// Best first.
    pub hits: Vec<Hit>,
    /// Where the next page of the ranking starts, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
    /// The mode that actually ranked them (a hybrid search without a
    /// vector or a backend is lexical, and says so here): `lexical`,
    /// `vector`, `hybrid`, or `index` and `hybrid (index)` when the vector
    /// field is one the lake asks an index for, whose set the hits are; a
    /// search with a second stage adds ` + rerank`.
    pub ranked_by: String,
    /// Whether the first stage holds more hits than the second stage's
    /// window took, unranked and not answered. Only a reranked search
    /// ends at a window.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Each axis that ranked, in the order the fusion sums them.
    #[serde(default)]
    pub axes: Vec<Axis>,
}

/// One axis a search ranked by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Axis {
    /// `lexical`, `vector` or `rerank`.
    pub axis: String,
    /// The vector field it ranked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<FieldName>,
    /// Where the search fuses, the candidates this axis handed the fusion
    /// for this reader, after the search's `where` and the rights that
    /// decide a hit, up to the lake's `max_limit`: never a count of the
    /// lake's matches. A search of one axis gives none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<u64>,
    /// Where the search fuses, the weight this axis ranked by: the
    /// search's own, else its type's `search_weights` setting's, else 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    /// The model the caller named for this axis's query vector, as it
    /// was named (for the `rerank` axis, the reranker's own): over a field
    /// declared as an embedding the field's own, and over any other field
    /// not judged, since the lake knows no model of its own there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// One search hit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// The record.
    pub record: Record,
    /// Higher is more relevant; comparable within one answer only.
    pub score: f64,
}

/// The score of a vector search: the cosine of the angle between two
/// vectors of one length, computed in `f64`; 0 when either is all zeros.
/// Every engine scores with this one definition,
/// so a ranking never depends on whose arithmetic made it.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}
