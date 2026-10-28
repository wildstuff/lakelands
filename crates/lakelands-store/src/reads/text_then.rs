//! A search ranked by the text at the position it reads (089): the records
//! its `where` selects there, read as `find` reads them, tokenised and
//! scored by the store's one pipeline over those records alone.

use super::*;

use lakelands_api::engine::TextCounts;
use lakelands_core::engines::Counts;

impl<'a> Reader<'a> {
    /// The first stage of a search with `text_as_of`: its page and one
    /// more hit. Its candidates are bounded by the lake's `max_text_as_of`,
    /// refused past it and never cut: a ranking of part of them would
    /// answer a wrong best page. A field is ranked and matched on a record
    /// only where its reader is shown it there both at the position and at
    /// the head (025).
    pub(super) fn text_then(&self, s: &Search) -> Result<First> {
        let (decl, named, filter, ranks) = self.searched(s)?;
        let SearchMode::Lexical { text, .. } = &s.mode else {
            unreachable!("a search with `text_as_of` is lexical (`Search::misfit`)");
        };
        let (read, mut offset) = (search_read(s), 0);
        if let Some(c) = &s.cursor {
            let state = resume(c, "search", Some(&s.ty), &Params::new())?;
            same_read(&state, &read)?;
            offset = state.offset as usize;
        }
        let max = self.lake.limits.max_text_as_of;
        let mut rows = self.page(&decl, filter.as_ref(), &[], None, max as usize + 1)?;
        if rows.len() > max as usize {
            let said = Said::from(Case::LimitRange { max }).within(Place::Where);
            return Err(Error::invalid_at(&"limits.max_text_as_of".into(), said));
        }
        let keys: Vec<RecordKey> = rows.iter().map(|r| r.key.clone()).collect();
        if let Some(admitted) = self.head_admitted(&decl, &keys, true)? {
            rows.retain(|r| admitted.contains(&r.key));
        }
        let named = named.map(|n| n.iter().collect::<Vec<_>>());
        let named = named.unwrap_or_else(|| self.sight.searched_fields(&decl).collect());
        // Searchable at the position, as `searched` holds, and at the head (025).
        let searched = |n: &&FieldName| ranks.fields.get(*n).is_some_and(|f| f.searchable);
        let fields: Vec<FieldName> = named.into_iter().filter(searched).cloned().collect();
        // Each record as its reader is told it at the position, without a
        // field a hiding lowers on it at the head.
        let now = self.lowered_now(&ranks, &keys)?;
        let told: Vec<StoredRecord> = (rows.iter().filter_map(|r| self.sight.told(r)))
            .map(|mut t| {
                let lowered = now.get(&t.key).into_iter().flatten();
                lowered.for_each(|f| _ = t.to_mut().fields.remove(f));
                t.into_owned()
            })
            .collect();
        let key = format!("{}{}", lakelands_api::seam::SEARCH_WEIGHTS_SETTING, s.ty);
        let mut weights = lakelands_api::seam::text_field_weights(self.shape.settings.get(&key));
        let own = own_text_weights(&s.mode).into_iter().flatten();
        weights.extend(own.map(|(f, w)| (f.to_string(), *w)));
        let weight = |f: &FieldName| Some((f.clone(), *weights.get(f.as_str())?));
        let weights = fields.iter().filter_map(weight).collect();
        let (mut counts, asked) = (TextCounts::default(), (text.as_str(), Some(&fields[..])));
        core_engines::text_stats(&decl, &told, asked, &mut counts);
        let counted = Counts::Counted(&counts);
        let ranked =
            core_engines::lexical(&decl, &told, text, asked.1, &weights, counted, u32::MAX);
        let tag = decl.language.clone().unwrap_or_default();
        let query = core_engines::tokens(text, &tag);
        let (mut hits, page) = (Vec::new(), s.k.max(1) as usize + 1);
        for (key, score) in ranked.into_iter().skip(offset).take(page) {
            let r = rows.iter().find(|r| r.key == key);
            let Some(record) = r.and_then(|r| self.sight.visible(&decl, r, s.with_meta)) else {
                continue;
            };
            let shown = |f: &&FieldName| now.get(&key).is_none_or(|l| !l.contains(*f));
            let fields: Vec<FieldName> = fields.iter().filter(shown).cloned().collect();
            let matches = match s.matches {
                true => core_engines::matched(&decl, &record, &fields, &query, &tag),
                false => Default::default(),
            };
            hits.push(Hit {
                record,
                score,
                matches,
            });
        }
        let axis = Axis {
            axis: "lexical".into(),
            field: None,
            candidates: None,
            weight: None,
            model: None,
        };
        let (ranked_by, axes, marking) = ("lexical".into(), vec![axis], None);
        Ok(First {
            hits,
            ranked_by,
            axes,
            offset,
            read,
            skipped: offset,
            marking,
        })
    }

    /// The fields, by their names at the read's position, a hiding lowers
    /// on each of `keys` of the type `ranks` ranks as the head holds the
    /// record (006); none for a read at the head.
    fn lowered_now(
        &self,
        ranks: &Ranks,
        keys: &[RecordKey],
    ) -> Result<BTreeMap<RecordKey, BTreeSet<FieldName>>> {
        let mut now = BTreeMap::new();
        let Some(h) = self.head.as_ref().filter(|_| !self.at_head) else {
            return Ok(now);
        };
        let sees = Sight::held(&h.who, h.shape.as_ref());
        if !sees.hides(&ranks.ty) {
            return Ok(now);
        }
        let moved: BTreeMap<RecordKey, &RecordKey> = (keys.iter())
            .map(|k| (h.since.key(&ranks.ty, k), k))
            .collect();
        let held = (h.view).get_many(&ranks.ty, &moved.keys().cloned().collect::<Vec<_>>())?;
        for r in held {
            let low = sees.lowered(&ranks.ty, &r.fields);
            let then = (ranks.fields.iter()).filter(|(_, f)| low.contains_key(&f.name));
            if let Some(k) = moved.get(&r.key) {
                now.insert((*k).clone(), then.map(|(n, _)| n.clone()).collect());
            }
        }
        Ok(now)
    }
}
