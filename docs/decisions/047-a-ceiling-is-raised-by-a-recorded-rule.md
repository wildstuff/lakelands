# 047 — A ceiling is raised by a recorded rule

## What is decided

The core's budgets are four:
- the product code's lines;
- the specification's lines;
- the width of the seams;
- the `cfg(feature)` sites.

**A budget goes down by itself.** A pull request that cuts lines lowers a ratchet base, and the base stays lowered.

**A ceiling is raised, and a ratchet base raised with it, only when all of this holds** (lakelands#451):

1. **The code that needs the room is decisive.** One of:
   - it closes a defect;
   - it is a check that pins a rule of rights, of a refusal's kind or order, or of the integrity of the log and its answers;
   - it carries a contract a host depends on to run on a lake, named in its issue.

   Convenience is not a reason, nor is a feature no host asked for, nor code a shared helper would replace.
2. **Cutting came first.** The newest diet inventory for that budget is on record, and it does not yield the room.
3. **The raise is sized to the need:** the measured lines of the pull requests it is for, rounded up to the next 500.
4. **It is recorded.** Slot a's word on the issue names the raise, and the raise is one row in `docs/budgets.md`: the date, the budget, from → to, the issues it is for, and the reason in one sentence. The operator may set any ceiling by a decision on lakelands#20, and that decision is a row too.
5. **The gate holds it.** `core-size`, `seam-width` and `cfg-budget` are red when a ceiling or a ratchet base is higher than on the pull request's base commit, unless the same pull request adds the row whose "to" is the new number.

**The first raise:** the specification, from 40 000 to 42 000 lines, by the operator's decision.

**The second raise:** the product code, from 36 000 to 36 500 lines, by the operator's decision, for what a host needs in 0.9.0 to move its data onto a lake (lakelands#481). The diet went on beside it.

**The third raise:** the specification, from 42 000 to 42 500 lines, by slot a's word under the rule, for the checks of two fixes that pin a rule of rights (lakelands#509). It is the first raise the rule makes without the operator's own decision; the operator was told.

**The fourth raise:** the specification, from 42 500 to 43 000 lines, by slot a's word under the rule, for the checks of what a listing of proposals shows its reader (lakelands#499). It rides in the pull request of the code it is for.

**The fifth raise:** the specification, from 43 000 to 44 000 lines, by slot a's word under the rule, for three pull requests that need the room at once: the checks of a migration's cost and memory (lakelands#480: what the head indexes answer after a record changes, and what a fold leaves where a later read goes through a migration) and the check of a rule of rights (lakelands#540).

**The sixth raise:** the product code, from 36 500 to 37 000 lines, by slot a's word under the rule, once the diet's inventory was carried out and did not yield the room: for the fold that keeps a migration in place of the records it changed (lakelands#480) and for a value no longer handed to a principal kept from it (lakelands#538).

**The seventh raise:** the specification, from 44 000 to 44 500 lines, by slot a's word under the rule, for the fold's checks (lakelands#480), after the checks of another fix took the room the fifth raise had sized for them.

**The eighth raise:** the specification, from 44 500 to 45 000 lines, by slot a's word under the rule, for the checks of a grant that holds a field through its renames (lakelands#527), after the checks of another fix took the room the seventh raise had left. It rides in the pull request of the code it is for.

**The ninth raise:** the product code, from 37 000 to 37 500 lines, by slot a's word under the rule, for two fixes that close what was handed to a principal kept from it: a grant that holds a field through its renames (lakelands#527) and a type its author is not shown (lakelands#538). It rides in the first of their pull requests.

**The tenth raise:** the specification, from 45 000 to 45 500 lines, by slot a's word under the rule, for the checks of a type its author is not shown (lakelands#538), which the room of the eighth raise did not hold after the checks it was sized for. It rides in the pull request of the code it is for.

**The eleventh raise:** the product code, from 37 500 to 38 000 lines, by slot a's word under the rule, for five pull requests measured before any of them was pushed: sets composed by name (lakelands#552), arithmetic on durations (lakelands#549), a bucket from a chosen start (lakelands#559), an identical declaration that appends nothing (lakelands#565) and PostgreSQL's transaction written by pages (lakelands#480). It stands alone: three slots push them in an order no one of them decides, and a raise that rode in one would hold the others.

**The twelfth raise:** the specification, from 45 500 to 46 000 lines, by slot a's word under the rule, for the checks of four of those pull requests as measured; it stands alone with the eleventh.

**The thirteenth raise:** the specification, from 46 000 to 46 500 lines, by slot a's word under the rule, for the checks of a set that names a set (lakelands#552): a rule of rights and the log's format, 83 lines more than the room the twelfth raise left, after the check was cut. It rides in the pull request of the code it is for.

**The fourteenth raise:** the product code, from 38 000 to 38 500 lines, by slot a's word under the rule, for a migration that renames a relation and keeps its edges (lakelands#429): 506 lines as measured with the review's fixes, 493 over the ceiling, after what the store and the mock share, and what the fold and a proposal's listing share, was written once.

**The fifteenth raise:** the specification, from 46 500 to 48 000 lines, by slot a's word under the rule, for the checks of the same pull request: rules of rights and the integrity of the log's answers, 1 275 lines as measured with the review's rows, on a develop that stood 71 under the ceiling: 1 204 over it; three steps of 500, since two do not hold them. Both ride in the pull request of the code they are for.

**The sixteenth raise:** the product code, from 38 500 to 39 000 lines, by slot a's word under the rule, for SQLite's order index marked by the declaration that first declares its field (lakelands#591: 12 lines as measured, 5 over the ceiling) and for a recomputation folded as a step, which follows under the same row (lakelands#523: 241 lines as its build measures).

**The seventeenth raise:** the product code, from 39 000 to 39 500 lines, by slot a's word under the rule, for the evaluator's part of a view kept as its sources are written (lakelands#575): a group's folds as a state a member is put into and taken out of, and whether a view can be kept, 599 lines as measured with the sum's rule, 136 over the ceiling.

**The eighteenth raise:** the specification, from 48 000 to 48 500 lines, by slot a's word under the rule, for the checks of the same pull request: that a kept state answers the computation's bytes, what can be kept, and the sum's rule, 657 lines as measured, 497 over the ceiling.

**The nineteenth raise:** the specification, from 48 500 to 49 000 lines, by slot a's word under the rule, for the rows of a rule of rights (lakelands#594: a proposal that waited is listed by the grants on the names in force): 50 lines as measured, on a develop that stood 3 under the ceiling, 47 over it.

**The twentieth raise:** the product code, from 39 500 to 40 000 lines, by slot a's word under the rule, for a recomputation folded as a step (lakelands#523): a declaration that computes a type's derived fields again no longer holds the type in memory under the writer. 248 lines as measured, on a develop that stood 195 under the ceiling, 53 over it; the sixteenth raise had named it, and the evaluator's part of a kept view took that room first.

**The twenty-first raise:** the specification, from 49 000 to 49 500 lines, by slot a's word under the rule, for the rows of a recomputation folded as a step (lakelands#523) and the row its review asked for (a derived field that reads a setting is as of its record's last write): 256 lines as measured, 48 834 on a develop that stood 422 under the ceiling; behind the 230 lines of lakelands#595, which merges first, 49 064, 64 over it.

**The twenty-second raise:** the specification, from 49 500 to 50 000 lines, by slot a's word under the rule, for the twin of a kept view (lakelands#575: four checks in which every view is declared twice, kept and computed, and the two answer alike): 903 lines as measured, on a develop that stood 436 under the ceiling, 467 over it; run on the store's build of the kept view it found a difference before that build merged.

**The twenty-third raise:** the specification, from 50 000 to 50 500 lines, by slot a's word under the rule, for the engines of the kept view (lakelands#575: SQLite and PostgreSQL read a view's kept name by statements of its own, and one engine row holds that the same states answer alike under a kept name and under an ordinary one): 183 lines as measured, on a develop that stood 33 under the ceiling, 150 over it.

**The twenty-fourth raise:** the product code, from 40 000 to 40 500 lines, by slot a's word under the rule, for the open of a SQLite lake (lakelands#564: an open reads the end of each segment's log index and not the index, a read finds its first line by halving the index, and a handle passes by the index of a segment it has read sealed): 126 lines as measured, on a develop that stood 110 under the ceiling, 16 over it.

**The twenty-fifth raise:** the product code, from 40 500 to 41 000 lines, by slot a's word under the rule, for a purge that names one type (lakelands#562, asked by a host's integration: the purge and its preflight take one optional type, read that type and the link types of its relations, judge co-ownership there alone, refuse a type they cannot take from, and give a preflight an id of its own): 69 lines as measured, on a develop that stood 27 under the ceiling, 42 over it.

**The twenty-sixth raise:** the specification, from 50 500 to 51 000 lines, by slot a's word under the rule, for the rows of the same pull request: what a typed purge takes and leaves, co-ownership in the named type alone, the links of the named type's relations, the id of the source, the type and the head, and the refusal of a type it cannot take from: 225 lines as measured after 53 were cut, on a develop that stood 26 under the ceiling, 199 over it.

**The twenty-seventh raise:** the specification, from 51 000 to 51 500 lines, by slot a's word under the rule, for the rows of a migration's report that says every difference between the declaration in force and its `to` (lakelands#554: every member of a declaration changed alone, the report as each reader may know it from every place that answers one, a field its reader does not read by its shape alone, and the accept): 607 lines as measured, on a develop that lakelands#562 left 301 under the ceiling, 306 over it.

**The twenty-eighth raise:** the product code, from 41 000 to 42 500 lines, by slot a's word under the rule, for the store's part of a kept view (lakelands#575: the fold keeps one record a group and a state of counters, and the read answers them where the state says they are the computation's own): 1 299 lines as measured after 105 were cut and what waited was taken out, on a develop that lakelands#554 left 256 under the ceiling, 1 043 over it, so the next 500 that holds it is 42 500.

**The twenty-ninth raise:** the specification, from 51 500 to 52 500 lines, by slot a's word under the rule, for the rows of the store's kept view (lakelands#575: a view's state and the number of its kept records after each event, which no answer shows, and the twin's lines for what plants and a review showed it blind to, two readers kept from a field a view reads among them): 1 021 lines as measured, on a develop that stood 194 under the ceiling, 827 over it, so the next 500 that holds them is 52 500.

## Why

The budgets keep the core small enough for one reader to hold. They have done that: four diets took out about 5 000 lines without losing a check.

**A ceiling that can never move makes the wrong trade**, though, when the code that needs the room is the code that matters most. A check that pins a rights rule is exactly what a host relies on when it moves its data onto a lake. If the only ways to fit it are to weaken it or to drop a feature a host needs, the budget has stopped protecting the reader and started costing them.

**A raise must cost something to ask for**, so that it is never the easy way out of a diet. That is why it needs a decisive reason, cutting first and a size that is measured. And it must stay visible after the fact. That is why it needs a row a reader finds in one place, and a gate that refuses a number which grew without one.


## What it is not

- **It is not a second budget.** The ceilings remain the limits, and the ratchet bases still follow every cut down.
- **It is not a way to defer a diet.** A raise names the pull requests it is for, and its size is theirs, not a reserve.
- **It does not change how lines are counted** (`scripts/gates/core-size.sh`).
