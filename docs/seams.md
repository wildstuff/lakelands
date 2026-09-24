# Seams

*Stub.* The contract step (lakelands#2) writes this document in full and
states the seam roster there; this page repeats that roster and does not
decide it. lakelands#15 is the review pass over it.

A seam is a trait in `lakelands-api` through which Lakelands reaches
anything outside its deterministic core: an engine, an executor, an
embedding backend, a resolver of principals. Whatever must reproduce
byte-identically from the log lives inside the store; whatever needs the
outside world lives behind a seam, configured through a `Location`.
