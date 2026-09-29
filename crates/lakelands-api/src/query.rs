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

use crate::expr::Condition;
use crate::id::{Cursor, FieldName, RecordKey, RelationName, TypeName};
use crate::read::{Aggregate, Order, ReadAt};
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
pub struct Start {
    /// The start's type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// One record by key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<RecordKey>,
    /// Records matching a filter.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub filter: Option<Condition>,
    /// Records a search finds, best first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchMode>,
}

/// One step of a pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// The relation walked; it must be declared on the type in scope.
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
}

/// What a pattern returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// `Rows` holds a start only when a step reaches it
    /// (`docs/decisions/026-the-store-walks-the-graph.md`).
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
    /// `(type, key)`. A walk that would reach more than the lake's
    /// `max_graph_nodes` is refused `too_large`, never cut
    /// (`docs/decisions/026-the-store-walks-the-graph.md`).
    Components,
}

impl Default for Projection {
    fn default() -> Self {
        Projection::Rows { fields: Vec::new() }
    }
}

/// The one pattern query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// For rows: sort keys over projected paths, most significant first.
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
                ty,
                key: Some(key),
                filter: None,
                search: None,
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

    /// The records one hop away over one relation.
    pub fn neighbors(ty: TypeName, key: RecordKey, relation: RelationName, walk: Walk) -> Pattern {
        let step = Step {
            relation,
            walk,
            ty: None,
            filter: None,
            depth: Depth::default(),
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
    /// multi-relation walk.
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
            })
            .collect();
        Pattern::from(ty, key, steps, Projection::Graph)
    }

    /// The shortest walk from one record to another over the given
    /// relations.
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
        /// The links walked.
        edges: Vec<Edge>,
        /// Whether the limit cut the graph short.
        truncated: bool,
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
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SearchMode {
    /// Full text over the type's searchable fields, or the ones named.
    Lexical {
        /// The words searched for.
        text: String,
        /// Match only in these searchable fields; absent, in every one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<FieldName>>,
    },
    /// Nearest neighbours of a vector the caller supplies.
    Vector {
        /// The vector field searched.
        field: FieldName,
        /// The query vector.
        vector: Vec<f32>,
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
        /// every one. The vector half is not narrowed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<FieldName>>,
    },
}

/// `search`: ranked retrieval over one type. Unlike `find`, the answer
/// is ordered by relevance and every hit carries a score.
///
/// Relevance is the order by definition, so `search` takes no
/// `order_by`: `k` is its limit and `cursor` pages through the ranking,
/// bound to the position of the first page. A caller who wants the
/// matches in another order uses `find`, or a pattern query that starts
/// from a search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// field is one the lake asks an index for, whose set the hits are.
    pub ranked_by: String,
}

/// One search hit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// The record.
    pub record: Record,
    /// Higher is more relevant; comparable within one answer only.
    pub score: f64,
}
