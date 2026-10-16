# 047 — A ceiling is raised by a recorded rule

## What is decided

The core's budgets are four:
- the product code's lines;
- the specification's lines;
- the width of the seams;
- the `cfg(feature)` sites.

### A budget goes down by itself

A budget goes down by itself. A pull request that cuts lines lowers a ratchet base, and the base stays lowered.

### A ceiling is raised, and a ratchet base raised with it, only when all of this holds

A ceiling is raised, and a ratchet base raised with it, only when all of this holds (lakelands#451):

1. **The code that needs the room is decisive.** One of:
   - it closes a defect;
   - it is a check that pins a rule of rights, of a refusal's kind or order, or of the integrity of the log and its answers;
   - it carries a contract a host depends on to run on a lake, named in its issue.

   Convenience is not a reason, nor is a feature no host asked for, nor code a shared helper would replace.
2. **Cutting came first.** The newest diet inventory for that budget is on record, and it does not yield the room.
3. **The raise is sized to the need:** the measured lines of the pull requests it is for, rounded up to the next 500.
4. **It is recorded.** A maintainer's word on the issue names the raise, and the raise is one row in `docs/budgets.md`: the date, the budget, from → to, the issues it is for, and the reason in one sentence. The operator may set any ceiling by a decision on lakelands#20, and that decision is a row too.
5. **The gate holds it.** `core-size`, `seam-width` and `cfg-budget` are red when a ceiling or a ratchet base is higher than on the pull request's base commit, unless the same pull request adds the row whose "to" is the new number.

### The raises are the rows of the table in `docs/budgets.md`

The raises are the rows of the table in `docs/budgets.md` (§Raises), oldest first. What a row there does not say of its raise:

- **The second raise:** the product code, from 36 000 to 36 500 lines, by the operator's decision, for what a host needs in 0.9.0 to move its data onto a lake (lakelands#481). The diet went on beside it.
- **The third raise:** the specification, from 42 000 to 42 500 lines, by a maintainer's word under the rule, for the checks of two fixes that pin a rule of rights (lakelands#509). It is the first raise the rule makes without the operator's own decision; the operator was told.
- **The fourth raise** (the specification, from 42 500 to 43 000 lines) rides in the pull request of the code it is for.
- **The fifth raise** (the specification, from 43 000 to 44 000 lines) is for three pull requests that need the room at once.
- **The eighth raise** (the specification, from 44 500 to 45 000 lines) rides in the pull request of the code it is for.
- **The ninth raise** (the product code, from 37 000 to 37 500 lines, for two fixes, lakelands#527 and lakelands#538) rides in the first of their pull requests.
- **The tenth raise** (the specification, from 45 000 to 45 500 lines) rides in the pull request of the code it is for.
- **The eleventh raise** (the product code, from 37 500 to 38 000 lines) is for five pull requests measured before any of them was pushed: sets composed by name (lakelands#552), arithmetic on durations (lakelands#549), a bucket from a chosen start (lakelands#559), an identical declaration that appends nothing (lakelands#565) and PostgreSQL's transaction written by pages (lakelands#480). It stands alone: three authors push them in an order no one of them decides, and a raise that rode in one would hold the others.
- **The twelfth raise** (the specification, from 45 500 to 46 000 lines) stands alone with the eleventh.
- **The thirteenth raise** (the specification, from 46 000 to 46 500 lines) rides in the pull request of the code it is for.
- **The fourteenth raise** (the product code, from 38 000 to 38 500 lines) and **the fifteenth** (the specification, from 46 500 to 48 000 lines) both ride in the pull request of the code they are for.
- **The sixteenth raise** (the product code, from 38 500 to 39 000 lines) is for SQLite's order index, marked by the declaration that first declares its field (lakelands#591), and for a recomputation folded as a step (lakelands#523).
- **The twenty-second raise** (the specification, from 49 500 to 50 000 lines) is for the twin of a kept view (lakelands#575): run on the store's build of the kept view it found a difference before that build merged.
- **The twenty-eighth raise** (the product code, from 41 000 to 42 500 lines) is for the store's part of a kept view (lakelands#575): 1 299 lines as measured after 105 were cut and what waited was taken out.
- **The thirtieth raise:** the specification, from 52 500 to 53 000 lines, by a maintainer's word under the rule, for the rows of an initial value (lakelands#551: the event that makes a record fills each field that declares `initial`, its refusals, the hash, and a purge that takes a record whose state an author moved). Its measure is its row's.

## Why

The budgets keep the core small enough for one reader to hold. They have done that: four diets took out about 5 000 lines without losing a check.

**A ceiling that can never move makes the wrong trade**, though, when the code that needs the room is the code that matters most. A check that pins a rights rule is exactly what a host relies on when it moves its data onto a lake. If the only ways to fit it are to weaken it or to drop a feature a host needs, the budget has stopped protecting the reader and started costing them.

**A raise must cost something to ask for**, so that it is never the easy way out of a diet. That is why it needs a decisive reason, cutting first and a size that is measured. And it must stay visible after the fact. That is why it needs a row a reader finds in one place, and a gate that refuses a number which grew without one.

## What it is not

- **It is not a second budget.** The ceilings remain the limits, and the ratchet bases still follow every cut down.
- **It is not a way to defer a diet.** A raise names the pull requests it is for, and its size is theirs, not a reserve.
- **It does not change how lines are counted** (`scripts/gates/core-size.sh`).

## How it came to be

- The first version: the rule, and the first raise beside it (lakelands#451).
- Each raise after it was written here beside its row, to the thirtieth.
- The list of raises left for the table in `docs/budgets.md`, which holds every raise as a row; what a row does not say of its raise stayed (lakelands#817).
- Corrected at the rewrite (lakelands#817): the thirtieth raise's own numbers ("444 lines as measured after 15 were cut, on a develop that stood 173 under the ceiling, 271 over it") contradicted its itemised row in `docs/budgets.md`, written in the same change: 509 lines after 51 were cut, on a develop that stood 17 under the ceiling, 492 over it. The row decides, so the record says "Its measure is its row's".
