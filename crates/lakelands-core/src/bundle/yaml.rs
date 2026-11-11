//! The one place a bundle's YAML is read.
//!
//! A document's text is checked once when it is read, and then read to
//! one value the document holds: `name` reads it, and every document
//! type is read from the text through `strict`'s verdict, so a struct
//! written as a sequence is refused with the contract's sentence, and
//! from the text itself, so the order an author wrote a field in is the
//! order it is read in. Every use of the YAML library is in this file,
//! so replacing it is a change to this file alone. The library is
//! `serde-saphyr`, a maintained, pure-Rust YAML 1.2 reader.

use serde::de::{DeserializeOwned, IgnoredAny};

/// One YAML document, read.
pub(super) struct Document {
    text: String,
    value: serde_json::Value,
}

impl Document {
    /// Read a document's text: it must be one well-formed YAML document.
    pub(super) fn read(text: &str) -> Result<Document, String> {
        serde_saphyr::from_str::<IgnoredAny>(text).map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_saphyr::from_str(text).map_err(|e| e.to_string())?;
        Ok(Document {
            text: text.to_owned(),
            value,
        })
    }

    /// The document as one of the strict document types.
    ///
    /// The value is read once for the verdict a sequence gets: it sorts
    /// a map's members, so it is not the value given back. The text is
    /// read as it is written, which keeps a field's order and words a
    /// refusal in the order the author met it.
    pub(super) fn to<T: DeserializeOwned>(&self) -> Result<T, String> {
        match lakelands_api::strict::from_value::<T>(self.value.clone()) {
            Err(refused)
                if matches!(
                    refused.inner(),
                    lakelands_api::strict::Fault::Positional { .. }
                ) =>
            {
                Err(refused.to_string())
            }
            _ => serde_saphyr::from_str(&self.text).map_err(|e| e.to_string()),
        }
    }

    /// Its top-level `name`, if it has one that is text.
    pub(super) fn name(&self) -> Option<String> {
        self.value.get("name")?.as_str().map(str::to_owned)
    }
}
