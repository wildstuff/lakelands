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
