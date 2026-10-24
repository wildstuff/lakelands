//! The second stage of a search: the first `top` hits of its ranking,
//! ranked again by the lake's reranker, and answered in that order with
//! nothing past the window
//! (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
//!
//! The reader is synchronous and the reranker is not, so a door reads the
//! window ([`Reader::rerank_window`]), calls the reranker, and reads again
//! ([`Reader::stage`]) at the position and on the clock of the first read.
//! That read, and every later page, answers the scored keys themselves,
//! each judged for its reader and never ranked again, so a later write
//! cannot push a scored hit out. A cursor carries the stage's order (a key
//! of the window and its score) and what the first page said of the first
//! stage. A key whose record its reader is not answered leaves its place
//! empty.

use lakelands_api::error::{Case, Seam};
use lakelands_api::query::{Axis, Hit, Hits, Pattern, Search, SearchMode};
use lakelands_api::read::ReadAt;
use lakelands_api::seam::RerankCandidate;
use lakelands_api::{Error, FieldName, Params, RecordKey, Result, Timestamp, TypeName, Value};
use serde::{Deserialize, Serialize};

use super::{CursorState, Judged, Pulled, Reader, encode, resume, same_read, search_read};

/// A window of the first stage, as the reranker ranks it.
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
    /// The first stage's `ranked_by`.
    pub ranked_by: String,
    /// The first stage's axes.
    pub axes: Vec<Axis>,
    /// Whether the first stage held more hits than the window.
    pub truncated: bool,
}

/// A second stage's order and what its first page said: the keys of the
/// window with the reranker's scores, best first, ties in the first
/// stage's order, and the answer's `ranked_by`, `axes` and `truncated`,
/// which every page repeats.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Staged {
    pub(super) order: Vec<(RecordKey, f64)>,
    ranked_by: String,
    axes: Vec<Axis>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

impl Staged {
    /// The order the reranker `model`'s `scores` give the window: one score
    /// per candidate, in the window's order.
    pub fn scored(window: &Window, scores: &[f64], model: &str) -> Staged {
        let keys = window.candidates.iter().map(|c| c.key.clone());
        let mut order: Vec<(RecordKey, f64)> = keys.zip(scores.iter().copied()).collect();
        // Stable: ties keep the first stage's order.
        order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut axes = window.axes.clone();
        axes.push(Axis {
            axis: "rerank".into(),
            field: None,
            candidates: Some(order.len() as u64),
            weight: None,
            model: Some(model.to_owned()),
        });
        Staged {
            order,
            ranked_by: format!("{} + rerank", window.ranked_by),
            axes,
            truncated: window.truncated,
        }
    }
}

/// The second stage of one read.
#[derive(Default)]
pub(super) struct Second {
    /// The order the door's reranker call gave, for a first page.
    pub staged: Option<Staged>,
    /// The order a pattern's cursor carried, for a later page.
    pub resumed: Option<Staged>,
    /// The order this read applied, which a cursor it hands out carries.
    pub decided: Option<Staged>,
    /// The pattern is being judged, and no record is to be read.
    pub judging: bool,
}

pub(super) struct First {
    pub hits: Vec<Hit>,
    pub ranked_by: String,
    pub axes: Vec<Axis>,
    pub offset: usize,
    pub read: String,
}

impl Reader<'_> {
    /// Hand the reader the order the reranker gave this first page's window.
    pub fn stage(&self, staged: Staged) {
        self.second.borrow_mut().staged = Some(staged);
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
            Some(case) => Err(Error::invalid_at(&"rerank".into(), case)),
        }
    }

    /// The window of a search's first stage, for the reranker, with what
    /// the first stage said of itself. The door refuses a search with no
    /// reranker bound as `Unbound` once this answers.
    pub fn rerank_window(&self, s: &Search, carried: Option<Pulled>) -> Result<Judged<Window>> {
        let Some(top) = self.rerank_top(s)? else {
            return Err(Error::invalid(Case::RerankWithoutSearch));
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
        let readable: Vec<&FieldName> = (self.sight.searched_fields(decl))
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
            ranked_by: first.ranked_by.clone(),
            axes: first.axes.clone(),
            truncated: first.hits.len() > top,
        }))
    }

    /// [`Self::rerank_window`] of a pattern's start search.
    pub fn query_window(&self, p: &Pattern, carried: Option<Pulled>) -> Result<Judged<Window>> {
        // The pattern is judged whole first, as its read will judge it, so a
        // pattern refused asks the reranker nothing (067).
        if p.start.ty.is_some() {
            self.second.borrow_mut().judging = true;
            let judged = self.query(p, None);
            self.second.borrow_mut().judging = false;
            let _ = judged?;
        }
        let Some(mode) = &p.start.search else {
            return Err(Error::invalid_at(
                &"rerank".into(),
                Case::RerankWithoutSearch,
            ));
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

    /// A page of a search with a second stage: the scored keys from the
    /// cursor's offset, each read by its key at this read's position and
    /// judged for its reader as a hit is, in the stage's order. The first
    /// stage is not ranked again. A key its reader is not answered here
    /// leaves its place empty, so nothing after it is skipped or answered
    /// twice; the page may hold fewer than `k`.
    pub(super) fn reranked(&self, s: &Search, top: usize) -> Result<Hits> {
        let (decl, _, filter, _) = self.searched(s)?;
        let read = search_read(s);
        let (offset, carried) = match &s.cursor {
            Some(c) => {
                let state = resume(c, "search", Some(&s.ty), &Params::new())?;
                same_read(&state, &read)?;
                // A cursor this lake handed out holds at most the window.
                let Some(stage) = state.stage.filter(|st| st.order.len() <= top) else {
                    return Err(Error::invalid(Case::CursorForeign));
                };
                (state.offset as usize, Some(stage))
            }
            None => (0, None),
        };
        let stage = {
            let second = self.second.borrow();
            carried
                .or_else(|| second.resumed.clone())
                .or_else(|| second.staged.clone())
        };
        let Some(stage) = stage else {
            return Err(Error::unbound(Seam::Rerank));
        };
        let k = s.k.max(1) as usize;
        let keys = stage.order.iter().skip(offset).take(k).cloned().collect();
        let hits = self.visible_hits(&decl, filter.as_ref(), keys, s.with_meta)?;
        let more = stage.order.len() > offset + k;
        self.second.borrow_mut().decided = Some(stage.clone());
        let next_cursor = more.then(|| {
            encode(&CursorState {
                at: self.position,
                after: None,
                offset: (offset + k) as u64,
                params: Params::new(),
                door: "search".into(),
                ty: Some(s.ty.clone()),
                of: read,
                now: Some(self.now),
                stage: Some(stage.clone()),
            })
        });
        Ok(Hits {
            hits,
            next_cursor,
            ranked_by: stage.ranked_by,
            truncated: stage.truncated,
            axes: stage.axes,
        })
    }
}
