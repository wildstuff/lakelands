# 090 — A setting value may hold from a date

**Proposed** on lakelands#1549, for 0.21.0. The operator reviews it on the issue; merging it does not accept it.

## What is decided

A setting holds one value at a time. Its history is the log's: transaction
time, which says what the lake knew and when, and never from which date
the world held a value. A host that keeps a VAT rate as a setting and
changes it from 1 July can only overwrite it. From that write on, every
expression reads the new rate. That includes a June invoice written
again, and a read at 30 June. No expression can ask for the rate that
was in force on an invoice's date.

This record adds forms to log format 21, which 0.21.0 writes for the
first time and which stays open until its tag
(`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`), so
it needs no step of its own. It extends
`docs/decisions/063-every-setting-is-in-one-list.md`, whose "A host
declares its own settings" gains the periods, the writes and their
telling, and `docs/decisions/017-one-expression-grammar.md`, whose
`setting` gains an optional argument and no new function. Each comes into
force with the build, which rewrites those records where they stand.

Below, *the day* of an instant is the calendar date that the last
section of "What is decided" names. That section holds the one
question left open.

### A declared setting holds its periods on its one record

A host's declared setting (063) holds one or more *periods*. Each period
is a value and the day it holds from, and it holds until the next
period's day. The first period holds from the start, so there are no
gaps and no overlaps, and every day has exactly one value. A period is
named by the date it holds from, and the first is named `start`.

They are kept on the setting's one record of `sys.setting`:
- `value` is the value of the first period, as it is today;
- a new field, `periods` (`json`, not required, absent where the setting
  holds no dated period), holds the later periods. It is a list of
  `{"from": "<date>", "value": <value>}`, ordered by `from`, strictly
  ascending. Each `from` is a date in the one form
  (`docs/log-format.md` §1.3), and each value is in the form that its
  declared kind keeps.

So every record written before this release is a setting of one period
from the start, and it reads as it did. The key stays one record. Its
grants by `only` and group, its hold, its group stamp, the rules by
which it is told (063) and `describe` all stay those of one record, and
the record's history tells every list it held.

Only a host's declared setting holds periods. A write that gives a period
to any other key is refused (`setting_period_undeclared`, below):
- the store's own settings steer how the lake is built at a position,
  not what a record's date is judged by;
- an undeclared key has no kind to judge a period's value by.

As a result, no setting kept for a type or a source has periods, and
063's rules for withholding such settings never meet one.

**The stored shape.** The declaration of `sys.setting`
(`lakelands_api::seam::setting_type`) gains `periods`. A writer's open
of a lake of format 20 declares `sys.setting` again, one version on,
after `lake_upgraded`. It does so for every store type the lake holds in
an older form (063). No record is written again and no fold changes, so
the projection stays. A lake needs no rewrite.

**What a format-21 event carries.** A period is carried by the
`record_upserted` of `sys.setting` that the write appends. Its fields
hold `periods` whole, as the store resolved it (below), and never the
write members `from` or `period`. A proposed write is the one other
place: the `write_proposed` event's patch keeps `from` or `period` as
written, until its accept resolves it (below). A reader refuses the
following in a log of a format before 21, naming the format:
- `periods` on a write of `sys.setting`;
- `from` or `period` in the patch of a `write_proposed` of
  `sys.setting`;
- an expression in a `declared` that calls `setting` with two arguments.

### `setting('key', at)` reads the value in force on a date

`setting` takes one argument or two. The first is the quoted key, as
today (`parse_setting_key`, `call_takes_quoted`). The second, `at`, may
be any expression: typically a field of the record, such as
`setting('vat_rate', supplied_on)`.

When the call is evaluated:
- **`at` is an instant.** That covers a `date` (its `YYYY-MM-DD` text,
  which is its UTC midnight), a `date_time`, a text that denotes an
  instant, and an instant that an expression computes (`known_at(f)`,
  `d + P1D`). The call answers the value of the period that holds the
  instant's day. That value is read as its kind, as `setting('key')`
  reads it (063).
- **`at` is null.** The call answers null, as every function does on
  null
  (`docs/decisions/023-one-evaluator-decides-what-an-expression-means.md`).
- **`at` is no instant.** A number, a boolean, a duration, a JSON value,
  or a text that denotes no instant is a misfit. Its case is
  `setting_takes_date`, naming the value, and the caller handles it as
  any expression that cannot be evaluated (023): a derived field is
  cleared (`docs/decisions/012-writes-touch-only-their-fields.md`), a
  filter does not match, a group key is null and a guard does not
  fire.
- **The key has no live record.** The call answers null, as today.

The null and misfit rules apply whether or not the key holds periods, so
a misfit does not stay hidden until a period is written. Where the kind
checker already knows `at` to be a boolean, a number or a duration, the
expression is refused `kind_takes_date`, naming it, where it is declared
or read, before any record runs, as `kind_takes_number` is today. A text
is judged value by value.

Neither form of `setting` is a clock function. `Expr::reads_clock` still
names `now()` and `today()` alone. 052's `computed_at` rule, 049's
reading of what can be kept and 066's `set_reads_clock` judge every
declaration as they did.

### `setting('key')` reads the value in force on the expression's clock

Without `at`, the call reads the period that holds the day of the clock
that `now()` and `today()` read:
- **in a fold:** the `known_at` of the event folded. This covers a
  derived field computed at the write, a trigger's `when`, a verb's
  `requires` and the values it sets at the call, a set's `where`
  decided at the write, and a kept view's `where` and measures;
- **in an aging run:** the date aged
  (`docs/decisions/036-aging-runs-by-date-once.md`);
- **in a read:** the read's clock
  (`docs/decisions/052-a-field-may-be-computed-at-the-read.md`):
  `valid_at`, else the host's clock, else the `known_at` at `as_of`,
  else the wall clock. A cursor's pages read its first page's clock.

So in a read `valid_at` 30 June, `setting('vat_rate')` answers June's
rate, and a field computed at the read answers it too.

A setting of one period answers the same on every day. So a lake that
holds only undated values answers exactly as 0.20.0 did:
- `setting('key')` is `value`;
- `setting('key', at)` with an instant `at` is `value`;
- nothing is a clock function that was not one before.

The 0.20.0 checks hold this unchanged, line for line, on every engine:
- `settings::SETTING_AS_OF`
- `settings::SETTING_AS_OF_THE_WRITE`
- `settings::SETTING_RESTRICTED`
- `settings::SETTINGS_TOLD`
- `settings::SETTING_RECORDS_TOLD`
- `settings::SETTING_RECORDS_PAGED`
- `settings::SETTING_IN_A_CONDITION`
- `host_settings::HOST_SETTING_KIND`
- `host_settings::HOST_SETTING_GROUP`

The new `settings::SETTING_UNDATED_AS_BEFORE` holds what those checks
cannot see (see "Checks").

### What the fold keeps does not drift at a period's boundary

Two things hold what they decided until a write moves them: a kept view
(`docs/decisions/049-a-view-the-fold-keeps.md`), and a set decided at
the write (`docs/decisions/066-a-set-holds-a-kept-view.md`). Nothing
writes again when the clock passes a day. Suppose such a view or set
reads `setting('key')` without `at`, and the key holds periods. Its
records would then answer the period of the write that last moved
them, and a read computing the view, or a read's `in_set`, would answer
the period of the read's clock. That is the drift that 066 refuses for
`now()`.

So three writes are refused `setting_dated_kept`, naming the view or the
set and the key, with nothing appended:
- a write that gives a key its first dated period, where a kept view
  (one its `view_kept` asks for, or one a set or a kept trigger holds
  kept) or a set decided at the write reads `setting('<key>')` without
  `at`, at any depth;
- a write of `view_kept.<view>` with `true` for a view that reads,
  without `at`, a key that holds periods. It is refused as a view that
  cannot be kept is (049), with this case as the reason;
- a declaration of a set decided at the write that reads such a key so,
  and a declaration of a kept view that reads it so.

The check is made where the write is judged, as 066's is, and the fold
judges nothing. The host's way out is the one the case names: read the
date, as `setting('<key>', <date>)`, or decide the set at the read.

A derived field computed at the write, and a trigger, are not refused.
Each is decided once, at its event, and a field holds what its last
write computed (`settings::SETTING_AS_OF_THE_WRITE`).

### A fold reads the periods as of the event it folds

When the fold hands an expression the lake's settings, it hands it the
records of `sys.setting` as of the event folded: `periods` included,
with the earlier writes of its append
(`docs/decisions/073-record-writes-share-one-append-in-a-batch.md`).
The day comes from `at`'s value or from the event's `known_at`. Both
are in the log, and the day of an instant is a function of the log
alone (below). So a replay, a rebuild, a catch-up and an import compute
what the write computed
(`docs/decisions/002-determinism-is-the-boundary.md`).

A period added or corrected later changes no stored value until the
record is written again, as a changed setting changes none today. A read
as of a position reads the periods of that position, so a correction is
seen from its own position on, and the read before it answers as
before.

### Two writes add a period and correct one, each an upsert of `sys.setting`

There is no new door, no new facade method and no new event kind. A
write of a period is an upsert of `sys.setting`, and its patch carries
one of two *write members* beside `key` and `value`. The store reads
them and never stores them on the record; only a proposal's event keeps
them, as written (Proposals, below):

- **`from: "<date>"` adds a period.** `value` holds from that date until
  the next period, or from then on. The store writes the record's list
  as it stands when the write appends, with the new period in its place,
  into the patch's `periods`. The patch carries no `value` field, so the
  first period is untouched (012).
- **`period: "start" | "<date>"` corrects the named period.** With
  `start`, the patch writes `value`. With a date, it writes `periods`,
  in which the period from that date holds the new value.
- **With neither,** the write is today's write: it writes `value`, the
  first period's. It is taken where the record holds no dated period, so
  every write a 0.20.0 host makes means what it meant. Where the record
  holds dated periods, it is refused `setting_periods_named`. A host
  that thinks of the setting as one value, and writes this month's rate
  undated, would otherwise rewrite the rate that held before every
  period.

The store resolves both members under the lake's one writer, against
the record as the head and the earlier writes of the append leave it
(`docs/decisions/076-a-lake-keeps-one-writer-and-a-dataset-that-outgrows-it-becomes-several-lakes.md`,
073). So two callers that each add a period both land. An expected
version is judged as it is for any upsert. A first value and a period
in one batch are two writes, the first creating the record.

`from` and a date `period` are judged as the value of a `date` field is.
The doors take an instant at a UTC midnight and write its date, and they
refuse one at any other time of day, naming the member. `value` is judged
by the setting's declared kind, and `null` is refused as its absence, as
063 already says of every write.

Every refusal is `invalid`, at the setting, naming the member, and
appends nothing:

| code | members | message |
|---|---|---|
| `setting_period_undeclared` | — | only a setting a host declares holds a value from a date |
| `setting_period_first` | — | a setting's first value holds from the start; write it without `from` |

`setting_period_first` answers `from` on a key with no live record: one
never written, or retired.
| `setting_period_held` | `from` (String) | a value already holds from {from}; correct it with `period` |
| `setting_period_unknown` | `period` (String) | no value of this setting holds from {period}; name `start` or a date one holds from |
| `setting_periods_named` | — | it holds values from dates; name the one to correct with `period`, `start` for the first, or the date a new one holds from with `from` |
| `setting_periods_stamped` | — | the store writes `periods` from `from` and `period`; leave it out |
| `setting_from_or_period` | — | a write adds a value `from` a date or corrects one `period`, not both |
| `setting_dated_kept` | `key` (String), `name` (String) | `{name}` is kept by the fold and reads `{key}` without a date, which holds values from dates, so it would drift at each period's start: read it as setting('{key}', <date>), or decide it at the read |

Two cases are not refusals of a write. They are the expression's
(above):

| code | members | message |
|---|---|---|
| `setting_takes_date` | `value` (Operand) | setting() reads the value in force on a date or an instant, not {value} |
| `kind_takes_date` | `expr` (String), `kind` (Kind) | `{expr}` has no meaning: it takes a date or an instant, not {kind} |

**Grants.** Each write is judged as any write of `sys.setting` is: by
the grant on the type, by `only` on its group (063) and by field rights
on the fields the resolved patch touches (`periods` for a dated period,
`value` for `start`). This changes what today's grants admit: a
`periods` field is new, so a grant whose field rights list `value` and
not `periods` corrects the first value and is refused an add or a
correction of a dated period, until its holder lists `periods` too. A
grant with no field rights admits both, as it admits `value` today. Suppose a writer is not told the setting's
declaration, as a holder of another group is not. It is answered
`setting_period_undeclared`, whatever it names, as 063 answers it for a
key nobody declared, so a group guessed right or wrong tells it
nothing.

**Proposals.** A proposed write keeps `from` or `period` as written,
in the patch of its `write_proposed` event: a format-21 form (above). It
is listed as written and resolved at its accept, against the head
there. A pinned proposal's accept is refused once the record changed
after it, as for any pinned proposal, so only an unpinned one is
resolved after another write.

**History.** Each write is one `record_upserted`. The record's history
and a read as of a position tell each list and value, and nothing is
rewritten or removed. Retiring the record retires the setting with all
its periods. This record removes no single period. A host that added
one by mistake corrects it to the value before it, or retires the
setting and writes it again.

**Its kind.** A declaration that gives the setting another kind judges
every period's value as 063 judges `value` today. A hold on the record
hides its periods as it hides its value, never from what the lake
computes (`settings::SETTING_RESTRICTED`).

### `describe` names each setting's periods

`SettingValue.periods: Vec<SettingPeriod>` (serde default, skipped when
empty), with `SettingPeriod { from: Option<String>, value: Value }`. It
lists every period in order, the first without `from`, and only for a
setting that holds a dated period. It is told to whoever is told the
value (063): a holder of `schema` and a principal that may write the
setting. `value` is then the value in force on the day of the describe's
clock (052). For a setting of one period, that is the record's value, as
today. So a host can say "19 % from 1 July" to a person, with the unit
from the declaration.

`lakelands settings` prints a dated setting's periods after its line,
one a line: `from start: 16 %`, `from 2026-07-01: 19 %`.

### The day of an instant: the operator's open question

`at` may be a `date_time`, and the clock is an instant. Each has to be
judged against a calendar date. **Open question for the operator: is the
day of an instant its date in UTC, or its date in a time zone declared
on the lake?**

**Recommended: UTC.** The rest of this record reads with this sentence:

> **The day of an instant is its calendar date in UTC; a `date` is its
> own day.**

Under the other answer, only that sentence changes, to:

> **The day of an instant is its calendar date in the time zone the
> lake's `day_zone` setting names, as of the event folded or the
> position read; a `date` is its own day.**

The options:
- **UTC.** The expression language already counts days in UTC.
  `today()` is the UTC midnight of the clock, `date_bucket` cuts UTC
  buckets, and a `date` is its UTC midnight (`docs/log-format.md` §1.3).
  The day of a value is then a function of the value alone, so no table
  of time zone rules enters a fold. What a host gives up: an instant near
  midnight in its own zone falls on the UTC date. A host that judges by
  its local day passes a `date`, which its record usually holds already
  (an invoice's supply date), and a date is exact under either answer.
- **A zone on the lake**, as a store setting `day_zone` for the lake,
  holding an IANA zone name and read as of the event folded. It costs:
  - The zone rules that the release ships would enter the fold. A
    rebuild under a later release whose rules moved an offset could
    then put an instant on another day than the write did, which 002
    does not allow, unless the version of the rules stands in the log.
  - An instant that an expression computes from a date (`d + P1D`) is
    an instant and no longer a date, so west of UTC it would fall on the
    day before.
  - `today()` and `date_bucket` would stay in UTC and answer another day
    than `setting` near midnight. Moving them would change every lake's
    answers.
  - A write of `day_zone` would change which period every later fold
    reads.

### It finds no records and recomputes none

This record does not find the records a period's change affects, and it
recomputes nothing by period. A host that needs its records to follow a
change finds them and writes them again, as it does today.

A `recompute` limited to the records whose `at` falls in a changed
period is left open. `at` is an expression, so finding those records is
a scan of the type unless an index already orders it. No host has
measured that it needs it, so this record does not decide it, and
nothing here closes it: the period a write changed is the difference
between the record's two lists.

### The doors carry the members, and the pages say them

- **MCP.** No tool is added, and no tool name changes. An upsert of
  `sys.setting` carries `from` or `period`. The catalog generator's
  schema of that upsert names both and leaves `periods` out, and
  `describe`'s output carries `periods`.
- **REST.** The same route carries the same members (`docs/rest.md`).
- **CLI.** `lakelands settings set <key> <value> --from <date>` adds a
  period, and `--period <start|date>` corrects one. The two are
  exclusive. With neither, the command runs as today. `lakelands
  settings` lists the periods (above).
- **Client.** Its upsert carries the members as any patch does, and its
  description types gain `periods` from `lakelands-api`.
- **WIT.** `setting-value` gains `periods: list<setting-period>`, with
  `record setting-period { %from: option<string>, value: json }`. An
  upsert's patch is JSON already, so nothing else moves.
- **Refusals.** The ten cases above, in the catalog of
  `lakelands_api::error::Case`, and `docs/refusals.md` and
  `docs/conformance.md` regenerated.
- **Settings.** The prose that `scripts/settings.py` writes in
  `docs/settings.md` on a host's own keys says that a declared key may
  hold values from dates, and how they are written and read.
  `docs/hosting.md` names `at` where it names `setting('<key>')`.
- **`docs/log-format.md`.**
  - §1.3: a period's `from` is a `date` in the one form, while a
    setting's value stays JSON.
  - Appendix A, the `setting` declaration's row: its records carry
    `periods` from format 21.
  - Appendix B: `setting` takes one or two arguments.
  - §6, the step 20 → 21: `periods` on `sys.setting`, `from` and
    `period` in the patch of a `write_proposed` of `sys.setting`, the
    declaration of `sys.setting` one version on at a writer's open, and
    `setting` with two arguments.
  - The format-21 vector: a dated setting, its correction, and a
    derived field that reads it with `at`. Where the vector needs a
    reader of its own, it joins lakelands#1534.

## Why

A rate that changes from a date is judged by the date of the record that
reads it, not by when the lake learned it. That is business time, and a
setting had none: transaction time alone cannot say that a June invoice
takes June's rate once July's is known. A host's workaround is a type of
its own for rates and a lookup in every expression, which is no
expression the grammar has. Or it writes its records again on every
change, and each one then takes today's rate whatever its date.

Periods on the one record keep the rest of what a setting is unchanged:
one key, one group, one grant, one hold and one history. The fold already
reads a setting as of the event it folds, so a period is deterministic
by the same rule, with nothing new in the fold but the choice of a
period by a day.

The store resolves `from` and `period` against the head, and the caller
does not send the list. One writer then decides each add and each
correction, a period cannot be lost to a stale list, and a refusal names
the intent the caller sent, not a list it had to build. A write without
either member keeps its 0.20.0 meaning where it can, and is refused
where that meaning would surprise the host.

`setting('key')` without `at` reads the clock's day, so a setting with
periods answers today's value in a read. That matches what an agent
asking "what is the rate" means. Not counting it as a clock read keeps
every 0.20.0 declaration valid, and the one place where that would let
an answer drift, a kept view or a kept set, is refused at the write.

## Rejected

- **One record per period** (a key such as `vat_rate@2026-07-01`, or a
  type `sys.setting_period`). The grants by group, the hold, 063's
  telling rules, `describe` and `setting()`'s lookup would each have to
  gather several records, and "no gaps" would become a rule across
  records that a retirement of one breaks. It also adds a reserved
  system type.
- **The caller writes `periods` whole.** Two callers that add a period
  race, and a list read before another's add drops that period without a
  word, unless every add carries an expected version and retries. The
  refusals would judge a list, not what was meant.
- **Periods chosen by the read's `valid_at` alone.** A read has one
  business time, and the invoices of one page hold different dates. A
  derived field computed at the write has no `valid_at` at all.
- **`setting('key')` as a clock read.** At its next declaration, that
  would refuse every derived field and kept view that read a setting in
  0.20.0 (052, 049, 066), and it would change no answer for them.
- **A period with an end (`until`).** It would allow gaps and overlaps,
  and the issue asks for neither. A period holds until the next one.
- **An undated write that silently corrects the first period** on a
  record that holds dated ones (above).

## What it is not

- **Not a search for affected records, and not a recompute.** See "It
  finds no records and recomputes none".
- **Not a valid time of `sys.setting`.** A type's `valid_from` and
  `valid_to` filter records in a read at `valid_at`. Periods are values
  within one record, which a read at any business time reads, and the
  record stays one.
- **Not provenance for each period.** `with_meta` names the last writer
  of `periods` as a whole. The record's history names who wrote each
  list.
- **Not for every key.** The store's own settings and undeclared keys
  hold one value.
- **Not a time zone.** Whatever the open question decides, a `date`
  stays its own day.

## Checks

On every engine. Periods from `2000-01-01` and `9000-01-01` put the
clock's day of every write the suite makes inside one known period.

1. `settings::SETTING_AT_A_DATE`: the case of lakelands#1549. A
   declared `vat_rate` (`decimal`) holds 0.16 and, from `2026-07-01`,
   0.19, and `invoice` derives `vat = net * setting('vat_rate',
   supplied_on)`. Invoices supplied on `2026-06-30` and `2026-07-01`,
   written after the period was added, or written again after it, hold
   16 and 19 on 100. One supplied on `2026-07-01` and written before the
   period was added keeps 16 until it is written again (Check 5). A
   `date_time` `at` of `2026-06-30T23:59:59Z` reads 0.16. A rebuild and
   a reopen answer the same.
2. `settings::SETTING_ON_THE_CLOCK`: `rate` holds 1, 2 from
   `2000-01-01` and 3 from `9000-01-01`. A derived field written now
   reads 2. A `find` whose `where` reads `setting('rate')` answers by 1
   at `valid_at` `1999-12-31`, by 2 at the head, and by 3 at `valid_at`
   `9000-01-01`, and a field computed at the read answers the same.
3. `settings::SETTING_UNDATED_AS_BEFORE`: an undated setting gives
   `setting('k', d)` equal to `setting('k')` for dates before, at and
   after its write. `describe` tells no `periods`, and the record has
   none. A lake imported from a format-20 export and opened by a writer
   declares `sys.setting` one version on and writes no other event.
4. `settings::SETTING_AT_MISFIT`: an `at` that is null answers null. An
   `at` of the text `soon`, of a number field and of a JSON field
   clears a derived field and matches no filter, each with
   `setting_takes_date`, also on a key with no periods. An `at` of a
   number literal is refused `kind_takes_date` at the declaration.
5. `settings::SETTING_PERIODS_AS_OF_THE_EVENT`: a record written while
   the period from `2026-07-01` held 0.19 keeps 19 after that period is
   corrected to 0.20, live, after a rebuild and after a reopen, and
   takes 20 when written again. A read as of before the correction
   tells `periods` with 0.19.
6. `host_settings::HOST_SETTING_PERIOD_WRITES`: an add, a second add
   before the first, a correction of a date and of `start`, and an
   undated write before any period are each taken, with the list
   ordered. Each refusal of the table is answered by its case, naming
   its member, with nothing appended: `from` on a retired key, `from`
   on a held date, `period` on an unknown date, an undated write on a
   dated record, a patch naming `periods`, both members, and `from` on
   `sort_index.<type>` and on an undeclared key. A first value and a
   period in one batch land. An unpinned proposed add is resolved at
   its accept, after another add; a pinned one is refused there, as any
   pinned proposal whose record changed.
7. `host_settings::HOST_SETTING_PERIOD_GRANTS`: a holder of `only
   {group: finance}` on `sys.setting` adds and corrects a period of a
   `finance` setting. For an `operator` setting, it is answered
   `setting_period_undeclared`, as in a twin where nobody declared it.
   A writer granted `value` and not `periods` corrects `start` and is
   refused an add.
8. `host_settings::HOST_SETTING_PERIODS_TOLD`: `describe` tells the
   periods in order, with the value in force on its clock, to a holder
   of `schema` and to one that may write the setting, and to no other
   reader.
9. `settings::SETTING_DATED_NOT_KEPT`: with a kept view and a set
   decided at the write, each reading `setting('rate')` without `at`,
   the first add to `rate` is refused `setting_dated_kept`, naming the
   view and then, with the view no longer kept, the set. With `rate`
   dated, `view_kept` `true` for the view and the declaration of the set
   are refused the same way. A view reading `setting('rate', d)` is kept
   and moves at each add.
10. `export::SETTING_PERIODS_FROM_21`: a reader refuses `periods` on a
    `sys.setting` write of format 20, `from` or `period` in the patch of
    a `write_proposed` of `sys.setting` of format 20, and a `declared` of
    format 20 that calls `setting` with two arguments, naming the
    format. The format-21
    vector carries a dated setting, its correction and a derived field
    reading it with `at`.

Unit rows beside `src/`: the parser's and checker's arity and kind of
`at`, and the evaluator's choice of a period at each edge (the day
before a `from`, the `from` day, the last period).

## Cost

Estimated from the nearest siblings, 063's declared settings and their
group (judged and stamped in `seam::check_setting`), and a function's
argument read by the evaluator. Seam width does not move, the facade
gains no method, and the engines do not move.

- **Product (core crates), est.: +350 to +500.**
  - `lakelands-api`: the field of `setting_type`, `SettingPeriod` and
    its member on `SettingValue`, the ten cases, the arity and the
    checks of `at`, and the scope's `setting` taking a day, about +110.
  - `lakelands-core`: the evaluator's day and choice of a period and its
    misfit, and the reader's two format refusals, about +70.
  - `lakelands-store`: `from` and `period` resolved and refused in the
    write of `sys.setting`, `setting_dated_kept` at its three writes,
    the scopes handed the periods, and `describe`, about +220 to +320.
- **Specification (core crates), est.: +450 to +650**, the ten checks
  and the unit rows.
- **Outside the core:** the CLI's two flags and listing, about +40; the
  catalog generator's schema of the upsert, about +15; WIT +5; the
  pages above.
- **Budget rows.** Core product stands at 93 972 of its 94 000 ceiling,
  and the open build of 089 (+200 to +300) takes the room above it with
  a row of its own. The build of this record needs its own product row
  of +500 over the ceiling that 089's build leaves: 94 500 → 95 000,
  where 089 lands first, as 0.21.0's order expects. It also needs a
  specification row of +500 over the ceiling that 089's build leaves
  (173 265 now, and 089's +250 to +400 take 173 500 → 174 000):
  174 000 → 174 500, which holds the top of this estimate. Each row is
  sized to the build's measure, by the rule of
  `docs/decisions/047-a-ceiling-is-raised-by-a-recorded-rule.md`.

## How it came to be

- Proposed on lakelands#1549, for 0.21.0.
