# Budgets

Four numbers keep the core small enough to read. Each is held by a gate
in `scripts/gates/`:

| Budget | Gate | Holds |
|---|---|---|
| product | `core-size` | the core crates' product code, in lines |
| specification | `core-size` | the core crates' conformance suite and unit tests, in lines |
| seam width | `seam-width` | the methods of the store facade and the engine traits |
| cfg(feature) sites | `cfg-budget` | the `cfg(feature)` sites of the workspace |

A budget goes down by itself. A pull request that cuts lines lowers a
ratchet base, and the base stays lowered.

A ceiling, or a ratchet base above where it stood, is raised only by the
rule of `docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md`:
- **The code that needs the room is decisive.** It closes a defect; or it
  is a check that pins a rule of rights, of a refusal's kind or order, or
  of the integrity of the log and its answers; or it carries a contract
  a host depends on to run on a lake, named in its issue.
- **Cutting came first,** and the newest diet inventory for that budget
  does not yield the room.
- **The raise is sized to the need:** the measured lines of the pull
  requests it is for, rounded up to the next 500.
- **It is recorded,** as one row below. Slot a's word names the raise on
  its issue, or the operator sets a ceiling by a decision on lakelands#20.

The budget gates are red when a number is higher than on the pull
request's base commit, unless the same pull request adds the row whose
"to" is that number. A raise can ride in the pull request of the code it
is for, or stand alone.

## Raises

| Date | Budget | From → to | For | Reason |
|---|---|---|---|---|
| 2026-10-03 | specification | 40 000 → 42 000 | lakelands#434, lakelands#436, lakelands#431, lakelands#419, lakelands#388, lakelands#428, lakelands#437 | The operator's decision on lakelands#20: the checks of a rights fix alone measure 224 lines, and the diets had already taken out about 5 000 lines without losing a check. |
| 2026-10-03 | product | 36 000 → 36 500 | lakelands#461, lakelands#428, lakelands#477, lakelands#437, lakelands#465, lakelands#466, lakelands#473, lakelands#474, lakelands#475, lakelands#476 | The operator's decision on lakelands#20: a host moving onto a lake found four more things it needs in 0.9.0 after the room was spoken for; the diet had taken out 142 lines and its remaining inventory of about 265 does not cover the peak. |
| 2026-10-03 | specification | 42 000 → 42 500 | lakelands#505, lakelands#504 | Slot a's word on lakelands#505, by the rule: the checks pin a rule of rights (a refusal names nothing its reader is not shown), the specification's diet inventory was carried out in full in this cycle (lakelands#427, 557 lines), and lakelands#505's check alone measures 68 lines over 42 000. |
| 2026-10-03 | specification | 42 500 → 43 000 | lakelands#499 | Slot a's word on lakelands#499, by the rule: the checks pin a rule of rights (a listing of proposals names nothing its reader may not know of, and an accept names no field hidden from the accepter), the specification's diet inventory was carried out in full in this cycle (lakelands#427), and the checks measure 297 lines, 147 over 42 500. |
| 2026-10-04 | specification | 43 000 → 44 000 | lakelands#480, lakelands#540 | Slot a's word on lakelands#480, by the rule: the checks of lakelands#480 pin the integrity of the log and its answers (what the head indexes answer after a record changes, and what a fold leaves when a later read goes through a migration it folded) and lakelands#540's pin a rule of rights; the specification's diet inventory was carried out in full in this cycle (lakelands#427); the checks measure 76 lines (the index rows), 325 (the fold) and 374 (lakelands#540), 775 over develop's 42 960: 43 735. |
| 2026-10-04 | product | 36 500 → 37 000 | lakelands#480, lakelands#538 | Slot a's word on lakelands#480, by the rule: the fold is the remedy of lakelands#480's defect (a migration holds a page of records, not the type) and lakelands#538 closes a value handed to a principal kept from it; cutting came first (the product's diet inventory of lakelands#518 is carried out, 57 of its 62 candidates, 337 lines since the tag); the fold measures 113 lines over develop's 36 461, which holds lakelands#538's first pull request (121 lines over 36 340): 36 574, 74 over 36 500. |
| 2026-10-04 | specification | 44 000 → 44 500 | lakelands#480 | Slot a's word on lakelands#480, by the rule: the fold's checks pin the integrity of the log and its answers (what a fold leaves when a later read goes through a migration it folded); the specification's diet inventory was carried out in full in this cycle (lakelands#427); the fold's checks measure 325 lines over develop's 43 734, which holds 324 lines of lakelands#538's checks that came after the last sizing: 44 059, 59 over 44 000. |
| 2026-10-04 | specification | 44 500 → 45 000 | lakelands#527 | Slot a's word on lakelands#527, by the rule: the checks pin a rule of rights (a grant by name holds a field through its renames); the specification's diet inventory was carried out in full in this cycle (lakelands#427); the checks measure 595 lines over develop's 44 314, which holds 255 lines of checks that came after the last sizing (lakelands#524's and lakelands#480's preflight): 44 909, 409 over 44 500. |
| 2026-10-04 | product | 37 000 → 37 500 | lakelands#527, lakelands#538 | Slot a's word on lakelands#527, by the rule: both close a value or a name handed to a principal kept from it; cutting came first (the product's diet inventory of lakelands#518, carried out in this cycle); lakelands#527 measures 275 lines over develop's 36 718 and lakelands#538's second pull request 37 more: 37 030, 30 over 37 000. |
| 2026-10-04 | specification | 45 000 → 45 500 | lakelands#538 | Slot a's word on lakelands#538, by the rule: the checks pin a rule of rights (a type its author is not shown is a type that is not declared); the specification's diet inventory was carried out in full in this cycle (lakelands#427), and the check was cut before the raise was taken; it measures 107 lines over develop's 44 909: 45 016, 16 over 45 000. |
| 2026-10-04 | product | 37 500 → 38 000 | lakelands#552, lakelands#549, lakelands#559, lakelands#565, lakelands#480 | Slot a's word on lakelands#552, by the rule: lakelands#552, lakelands#549, lakelands#559 and lakelands#565 carry what a host needs to run on a lake, each named in its issue by the first host's integration, and the engines' pull request of lakelands#480 is the remedy of its defect on PostgreSQL; cutting came first (the product's diet inventory of lakelands#518, carried out in this cycle); as measured by their slots on develop's 37 032: lakelands#549 271, lakelands#559 48, lakelands#565's rule 28, lakelands#552 257, the engines 39: 37 675, 175 over 37 500. It stands alone, so that no one of the five carries it. |
| 2026-10-04 | specification | 45 500 → 46 000 | lakelands#549, lakelands#559, lakelands#565, lakelands#480 | Slot a's word on lakelands#552, by the rule: the checks pin refusals' kinds (a derived field that can never be filled, a bucket without its origin), the integrity of the log (an identical declaration appends nothing; a transaction of many puts leaves what putting them one by one leaves) and the log's format; the specification's diet inventory was carried out in full in this cycle (lakelands#427); as measured by their slots on develop's 45 016: lakelands#549 298, lakelands#559 114, lakelands#565's rule 91, the engines 127: 45 646, 146 over 45 500. lakelands#552's checks are not measured yet and are not in it. |
| 2026-10-04 | specification | 46 000 → 46 500 | lakelands#552 | Slot a's word on lakelands#552, by the rule: the checks pin a rule of rights (a named set is read by the right to read what its filter reads; a reader who reads everything is listed every set and told of the sets a write leaves) and the log's format (`in_set` in format 11); the specification's diet inventory was carried out in full in this cycle (lakelands#427), and the check went from 399 lines to 336 while it gained the rows of the reader who reads everything and of a pattern; measured 370 lines over develop's 45 713, which holds lakelands#549, lakelands#559, lakelands#565's rule and door and lakelands#577: 46 083, 83 over 46 000. |
