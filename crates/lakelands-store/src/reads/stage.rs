//! The second stage of a search: the first `top` hits of its ranking,
//! ranked again by the lake's reranker, and answered in that order with
//! nothing past the window
//! (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
//!
//! The reader is synchronous and the reranker is not, so a door reads the
//! window ([`Reader::rerank_window`]), calls the reranker, and reads again
//! with its scores ([`Reader::stage`]) at the position and on the clock of
//! the first read, so both read one window. A cursor carries the stage's
//! order, each entry a key of the window and its score, so a later page
//! applies it without a second call. A page answers only what its own
//! window holds: a record held since, or one its reader is not shown, is
//! not answered, as a search without the stage leaves it out.

use std::collections::BTreeMap;

use lakelands_api::error::{Case, Seam};
use lakelands_api::query::{Axis, Hit, Pattern, Search, SearchMode};
use lakelands_api::read::ReadAt;
use lakelands_api::seam::RerankCandidate;
use lakelands_api::{Error, FieldName, RecordKey, Refusal, Result, Timestamp, TypeName, Value};
use lakelands_core::{invalid, rights};

use super::{Judged, Pulled, Reader};

/// What a reranker scored, by the key of the candidate.
pub type Scores = BTreeMap<RecordKey, f64>;

/// A window of the first stage, as the reranker is asked to rank it.
pub struct Window {
    /// The searched type.
    pub ty: TypeName,
    /// What the candidates are ranked against.
    pub query: String,
    /// The candidates, in first-stage order.
    pub candidates: Vec<RerankCandidate>,
    /// The clock the window was read on, which the read that ranks it
    /// reads on too.
    pub now: Timestamp,
}

/// An order a second stage decided over a window: each hit's key with its
/// score, best first.
pub(super) type Decided = Vec<(RecordKey, f64)>;

/// The second stage of one read: what the door handed it, what a pattern's
/// cursor carried, and what the read decided.
#[derive(Default)]
pub(super) struct Second {
    /// The scores of the reranker's call, for a first page.
    pub scores: Option<Scores>,
    /// The order a pattern's cursor carried, for a later page.
    pub resumed: Option<Decided>,
    /// The order this read applied, which a cursor it hands out carries.
    pub decided: Option<Decided>,
    /// The model of the reranker that scored the window.
    pub model: Option<String>,
}

/// The first stage of a search as ranked.
pub(super) struct First {
    pub hits: Vec<Hit>,
    pub ranked_by: String,
    pub axes: Vec<Axis>,
    pub offset: usize,
    pub read: String,
    pub resumed: Option<super::CursorState>,
}

impl Reader<'_> {
    /// Hand the reader the scores the reranker answered for the window of
    /// this read's first page.
    pub fn stage(&self, scores: Scores, model: Option<String>) {
        let mut second = self.second.borrow_mut();
        (second.scores, second.model) = (Some(scores), model);
    }

    /// The window a search with a second stage asks, or `None` for a
    /// search with none: refused for a `top` the lake does not allow and
    /// for a stage that has no text to rank against.
    pub(super) fn rerank_top(&self, s: &Search) -> Result<Option<usize>> {
        let Some(r) = &s.rerank else {
            return Ok(None);
        };
        let max = self.lake.limits.max_rerank_top;
        let case = r.misfit(&s.mode).or(match r.top > max {
            true => Some(Case::RerankTopAbove { max }),
            false => None,
        });
        match case {
            None => Ok(Some(r.top as usize)),
            Some(case) => Err(Error::Refused {
                field: Some("rerank".into()),
                reason: Refusal::Invalid(Box::new(case.into())),
            }),
        }
    }

    /// The window of a search's first stage, for the reranker. No reranker
    /// bound refuses the search as `Unbound`, once it is judged and before
    /// anything is ranked.
    pub fn rerank_window(&self, s: &Search, carried: Option<Pulled>) -> Result<Judged<Window>> {
        let Some(top) = self.rerank_top(s)? else {
            return Err(invalid(Case::RerankWithoutSearch));
        };
        let first = match self.first_stage(s, Some(top), carried)? {
            Ok(first) => first,
            Err(moved) => return Ok(Err(moved)),
        };
        let decl = self.named_type(&s.ty)?;
        let named = match &s.mode {
            SearchMode::Lexical { fields, .. } | SearchMode::Hybrid { fields, .. } => {
                fields.as_deref()
            }
            SearchMode::Vector { .. } => None,
        };
        // Only a field its reader reads, and the masked record holds no
        // value of any other.
        let readable: Vec<&FieldName> = rights::searched_fields(self.who, decl)
            .filter(|f| named.is_none_or(|n| n.contains(f)))
            .collect();
        let text = |h: &Hit| {
            let texts = readable
                .iter()
                .filter_map(|f| match h.record.fields.get(*f) {
                    Some(Value::String(t)) => Some(t.as_str()),
                    _ => None,
                });
            texts.collect::<Vec<_>>().join("\n\n")
        };
        let window = first.hits.iter().take(top);
        let candidates = window.map(|h| RerankCandidate {
            key: h.record.key.clone(),
            text: text(h),
        });
        let query = s.rerank.as_ref().map_or("", |r| r.query(&s.mode));
        Ok(Ok(Window {
            ty: s.ty.clone(),
            query: query.to_owned(),
            candidates: candidates.collect(),
            now: self.now,
        }))
    }

    /// [`Self::rerank_window`] of a pattern's start search.
    pub fn query_window(&self, p: &Pattern, carried: Option<Pulled>) -> Result<Judged<Window>> {
        let Some(mode) = &p.start.search else {
            return Err(invalid(Case::RerankWithoutSearch));
        };
        self.rerank_window(&self.start_search(p, mode)?, carried)
    }

    /// The search a pattern's start is: the best hits, one page of the
    /// lake's `max_limit`.
    pub(super) fn start_search(&self, p: &Pattern, mode: &SearchMode) -> Result<Search> {
        Ok(Search {
            ty: super::start_type(p)?.clone(),
            mode: mode.clone(),
            k: self.lake.limits.max_limit,
            cursor: None,
            filter: p.start.filter.clone(),
            rerank: p.start.rerank.clone(),
            with_meta: false,
            at: ReadAt::default(),
        })
    }

    /// The order of the first `top` of the first stage's `hits` in the
    /// second: as the reranker scored them on a first page, as the cursor
    /// carried it after. With whether the first stage held more.
    pub(super) fn staged(
        &self,
        hits: &[Hit],
        top: usize,
        resumed: Option<Decided>,
    ) -> Result<(Decided, bool)> {
        let window = &hits[..hits.len().min(top)];
        let carried = resumed.or_else(|| self.second.borrow().resumed.clone());
        let order = match carried {
            Some(order) => order,
            None => {
                let second = self.second.borrow();
                let Some(scores) = &second.scores else {
                    return Err(Error::unbound(Seam::Rerank));
                };
                let mut order: Decided = (window.iter())
                    .filter_map(|h| Some((h.record.key.clone(), *scores.get(&h.record.key)?)))
                    .collect();
                // Stable: ties keep the first stage's order.
                order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                order
            }
        };
        Ok((order, hits.len() > top))
    }
}
