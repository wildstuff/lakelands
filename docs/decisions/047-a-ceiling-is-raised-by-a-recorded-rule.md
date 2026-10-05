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

## Why

The budgets keep the core small enough for one reader to hold. They have done that: four diets took out about 5 000 lines without losing a check.

**A ceiling that can never move makes the wrong trade**, though, when the code that needs the room is the code that matters most. A check that pins a rights rule is exactly what a host relies on when it moves its data onto a lake. If the only ways to fit it are to weaken it or to drop a feature a host needs, the budget has stopped protecting the reader and started costing them.

**A raise must cost something to ask for**, so that it is never the easy way out of a diet. That is why it needs a decisive reason, cutting first and a size that is measured. And it must stay visible after the fact. That is why it needs a row a reader finds in one place, and a gate that refuses a number which grew without one.

## What it is not

- **It is not a second budget.** The ceilings remain the limits, and the ratchet bases still follow every cut down.
- **It is not a way to defer a diet.** A raise names the pull requests it is for, and its size is theirs, not a reserve.
- **It does not change how lines are counted** (`scripts/gates/core-size.sh`).
