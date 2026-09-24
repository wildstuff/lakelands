//! The store facade over an engine.
//!
//! Opens a lake at a location, holds its single writer, appends to the
//! log and keeps the projection in step with it, and answers reads with
//! the position they were read at. Engine-agnostic: an engine plugs in
//! through the engine seam declared in `lakelands-api`.
//!
//! Empty until the store is imported.
