# 024 — Probing a field needs the right to read it

The rules of this record moved to
`docs/decisions/006-grants-not-roles.md` at lakelands#817. The number
is not reused.

| what this record held | where it is now |
|---|---|
| a read that filters on a field, orders by it, aggregates over it or searches it needs the right to read it; a field its principal may not know of answers as one the type does not declare (lakelands#394) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a read at `valid_at` probes its type's valid-time fields (lakelands#372), a walk those of every type it can meet (lakelands#831, lakelands#829), a view read at a valid time those of its sources (lakelands#881) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a search probes the searchable fields it searches, in every mode, and what its vector is embedded from (lakelands#242, lakelands#662); a hybrid search is judged before it embeds (lakelands#741); a search limited to named fields (lakelands#236) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| `describe` describes a masked field by its shape alone, and no definition that names a field its reader may not read (lakelands#206) | `006 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
| Why: a probe turns a masked field into an oracle | `006 § Why` |
| What it is not: a refusal of reads of records that have such a field; a rule on what a writer may write | `006 § What it is not` |
| annotations, and a relation's link fields (lakelands#239) | `006 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
| a write's refusal tells only what its grants cover: grants before data, the version right after the record is found (lakelands#343) | `006 § A write asks first whether its writer is shown the type, checks grants before data, and tells only what the grants cover` |
| a setting the store reads is judged by what its writer is shown (lakelands#601) | `006 § A write asks first whether its writer is shown the type, checks grants before data, and tells only what the grants cover` |
| a write asks first whether its writer is shown the type, with what was measured (lakelands#607) | `006 § A write asks first whether its writer is shown the type, checks grants before data, and tells only what the grants cover` |
| a field built from another is read as that field (lakelands#387) | `006 § A field has one access, the lowest over every name it has borne and everything it is built from` |
| a hidden field is not removed by one who may not know of it (lakelands#475) | `006 § A declaration or a migration is its author's view` |
| a view's own probe names nothing (lakelands#505) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a proposed value is shown where the record's is: a patch, a payload, a write to a type its reader may not see, the proposer's own listing (lakelands#499) | `006 § A proposal is listed as its reader's view, and one that names what its reader is not shown is not there` |
| a report's sample keys are of records its reader is answered (lakelands#531) | `006 § A report is answered as its reader may know it` |
| a computed field is read as what it is computed from, listed or not (lakelands#540); an expression is read as it is checked (lakelands#652) | `006 § A field has one access, the lowest over every name it has borne and everything it is built from` |
| a declaration reads only what its author may read; not at the accept; a declaration in force stays in force (lakelands#538) | `006 § A declaration or a migration is its author's view` |
| a write's answer names a reaction to one who reads what its condition reads (lakelands#538) | `006 § A write asks first whether its writer is shown the type, checks grants before data, and tells only what the grants cover` |
| what stays: a holder of `schema` is shown every definition, and a write answers `through` (lakelands#538) | `006 § Some things are still told, on purpose` |
| a field is the same field under another name (lakelands#527) | `006 § A field has one access, the lowest over every name it has borne and everything it is built from` |
| a value type's report is shown as its listing is (lakelands#538), and names a declaration by what its reader is described of it (lakelands#861) | `006 § A report is answered as its reader may know it` |
| a named set is read by its filter's right (lakelands#552) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a field's declaration is shown by one list, wherever it is answered (lakelands#554) | `006 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
| a schema hash is that of the shape its reader is shown; a report's id is made over what is shown; what stays whole (lakelands#644) | `006 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
| the rest of a type's description is computed over the same shown shape (lakelands#662) | `006 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
| what remains says a hidden thing exists, on purpose: the version, the key, the valid times and `via`, a valid time or an inverse refused as taken, a verb on a link type, `may.retire` (lakelands#726, lakelands#692, lakelands#829, lakelands#843) | `006 § Some things are still told, on purpose` |
| a read at `valid_at` filters by the bounds its reader is shown and probes only those (lakelands#726) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a relation carried by a field is walked by its reader only (lakelands#727) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a step with a `type` probes what it keeps (lakelands#829) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a database's error quotes no row (lakelands#735, lakelands#739) | `006 § What is hidden answers at every door as what is not declared, and what is masked is named and refused as not granted` |
| a reference to a type not shown is a field not known of (lakelands#718) | `006 § A type is shown only where the grants give something of it, of what it is made of and of what it names` |
| a verb reads only what its caller may read, and a call proposal of such a verb is not there to a reader kept from what it reads (lakelands#779, lakelands#791) | `006 § A verb is callable and described only to one who may write what it sets and read what it reads` |
| a view's definition is shown to those its read answers (lakelands#798) | `006 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
| a set named in another's filter is judged by the sets its author is shown, and to propose a set under the name of one its proposer is not shown is not granted (lakelands#877) | `006 § A declaration or a migration is its author's view` |
| a read naming a set of a type its reader is not shown (lakelands#877) | `006 § A shown type's records may be found and got, and a probe needs the right to read what it probes` |
| a trigger, a verb, a function that computes a field and a tool's name that their author is not shown are not granted (lakelands#893, lakelands#896); nor, to a holder of `schema`, a function that computes a field of a type it is not shown (lakelands#917) | `006 § A declaration or a migration is its author's view` |
| a trigger, a verb and a set proposed by one who is shown the one in force are not there to an accepter kept from the type it stands on (lakelands#893) | `006 § An accept decides only what its reviewer is shown, and its refusal names only what the listing shows` |
| a value type is listed only to a reader shown a field that uses it, or holding `schema`, and a proposed migration of one it is not listed is listed without its write (lakelands#1088) | `006 § A description, a schema hash and a report's id are those of the shape their reader is shown` |
