//! The vector index: every vector of one field of one type, searched by
//! brute force, keeping only the best `k` as it goes.
//!
//! Similarity is the cosine of the angle between the query and a stored
//! vector, so a higher score is closer and the length of a vector does
//! not matter. A vector of zero length (all zeros) points nowhere and
//! scores 0 against everything. Results are best first; equal scores are
//! ordered by record key, so the answer is the same on every run.
//! Vectors are indexed at the head only; filtering the candidates by
//! what a view at an earlier position could see is the store's.

use std::collections::BTreeMap;

use lakelands_api::error::Result;
use lakelands_api::id::{RecordKey, TypeName};

use crate::invalid;

/// Every indexed vector, by type and field, then by record key.
#[derive(Debug, Default)]
pub(crate) struct Vectors(BTreeMap<(TypeName, String), BTreeMap<RecordKey, Vec<f32>>>);

impl Vectors {
    /// Indexes one record's vector for one field, replacing any before.
    pub(crate) fn put(&mut self, ty: &TypeName, key: &RecordKey, field: &str, vector: &[f32]) {
        self.0
            .entry((ty.clone(), field.to_owned()))
            .or_default()
            .insert(key.clone(), vector.to_vec());
    }

    /// Removes one record's vector for one field, if there is one.
    pub(crate) fn remove(&mut self, ty: &TypeName, key: &RecordKey, field: &str) {
        if let Some(field) = self.0.get_mut(&(ty.clone(), field.to_owned())) {
            field.remove(key);
        }
    }

    /// The `k` vectors closest to `query`, best first.
    pub(crate) fn nearest(
        &self,
        ty: &TypeName,
        field: &str,
        query: &[f32],
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
        let Some(vectors) = self.0.get(&(ty.clone(), field.to_owned())) else {
            return Ok(Vec::new());
        };
        let mut top = lakelands_core::engines::TopK::new(k);
        for (key, v) in vectors {
            if v.len() != query.len() {
                return Err(invalid(format!(
                    "the query vector has {} dimensions, but `{ty}.{field}` holds vectors of {} \
                     (record `{key}`)",
                    query.len(),
                    v.len()
                )));
            }
            top.push(key, lakelands_api::query::cosine(query, v));
        }
        Ok(top.into_ranked())
    }

    /// How many vectors one field holds.
    pub(crate) fn count(&self, ty: &TypeName, field: &str) -> u64 {
        self.0
            .get(&(ty.clone(), field.to_owned()))
            .map_or(0, |v| v.len() as u64)
    }

    /// Forgets every vector.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Vectors {
        let mut v = Vectors::default();
        let ty = TypeName::from("doc");
        v.put(&ty, &"east".into(), "emb", &[1.0, 0.0]);
        v.put(&ty, &"north".into(), "emb", &[0.0, 1.0]);
        v.put(&ty, &"north-east".into(), "emb", &[1.0, 1.0]);
        v.put(&ty, &"also-east".into(), "emb", &[3.0, 0.0]);
        v
    }

    #[test]
    fn the_nearest_come_best_first_with_ties_by_key() {
        let hits = index()
            .nearest(&"doc".into(), "emb", &[2.0, 0.0], 3)
            .unwrap();
        let keys: Vec<&str> = hits.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["also-east", "east", "north-east"]);
        assert!((hits[0].1 - 1.0).abs() < 1e-12);
        assert!((hits[2].1 - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn put_replaces_remove_forgets_and_count_counts() {
        let mut v = index();
        let ty = TypeName::from("doc");
        assert_eq!(v.count(&ty, "emb"), 4);
        v.put(&ty, &"east".into(), "emb", &[0.0, 1.0]);
        v.remove(&ty, &"north".into(), "emb");
        v.remove(&ty, &"absent".into(), "emb");
        assert_eq!(v.count(&ty, "emb"), 3);
        assert_eq!(v.count(&ty, "other"), 0);
        let best = v.nearest(&ty, "emb", &[0.0, 1.0], 1).unwrap();
        assert_eq!(best[0].0.as_str(), "east");
    }

    #[test]
    fn a_zero_vector_scores_zero_and_a_wrong_length_is_refused() {
        let v = index();
        let ty = TypeName::from("doc");
        let hits = v.nearest(&ty, "emb", &[0.0, 0.0], 10).unwrap();
        assert!(hits.iter().all(|(_, s)| *s == 0.0));
        assert!(v.nearest(&ty, "emb", &[1.0], 10).is_err());
        assert!(v.nearest(&ty, "none", &[1.0], 10).unwrap().is_empty());
    }
}
