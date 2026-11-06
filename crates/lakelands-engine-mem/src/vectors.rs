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

/// One write of the index: a record's vector for one field, or its end.
pub(crate) type Indexed = (TypeName, RecordKey, String, Option<Vec<f32>>);

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

    /// Applies one write.
    pub(crate) fn set(&mut self, (ty, key, field, vector): Indexed) {
        match vector {
            Some(v) => self.put(&ty, &key, &field, &v),
            None => self.remove(&ty, &key, &field),
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
                let (query, held) = (query.len(), v.len());
                return Err(lakelands_core::engines::dimensions(query, held, ty, field));
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
mod tests;
