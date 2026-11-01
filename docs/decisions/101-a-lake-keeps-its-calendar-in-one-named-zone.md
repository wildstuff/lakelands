# 101 — A lake keeps its calendar in one named zone

Decided on lakelands#1665 by the operator (2026-11-01), for 0.23.0.

## What is decided

**Decided** on 2026-11-01. It amends 036 and 063, and the meaning of
`today()` in the expressions.

### A lake names one zone, and it is a setting in the log

A lake reads its calendar day in one zone, named as the IANA time-zone
database names it (`Europe/Berlin`). The zone is the lake setting
`zone`: a record of `sys.setting` keyed `zone` whose `value` is the
name. The store judges it as it is written (`seam::check_setting`): a
name the database the build carries does not know is refused
`setting_zone`. A lake that holds no `zone`, or holds one retired, reads
its day in `UTC`, which is what every lake did before this record.

The zone holds one value at a time. It holds no values from dates (090):
a change of zone takes effect at the position that writes it, not from a
day of the calendar the zone itself defines.

### It is given when the lake is created, and nowhere else outside the log

A new lake starts in the zone its `Location` names (`zone`, in
`lakes/<id>.json`; `lakelands lake new --zone <name>` writes it). The
writer's first open records it as the lake's `zone` setting, written by
the store for itself as any setting is written, where the lake has never
recorded a `zone`, live or retired. From then on the log alone says the
zone: no later open reads the location's, so a location copied for a
moved or restored lake changes nothing. Only an open that creates or
writes the lake records it: a rebuild's open refolds the log and writes
nothing of its own, and a standby that takes the lease over finds the
zone the lake's first writer recorded. A zone the database does not name
refuses the open before the engine is opened, so a create that is
refused makes no lake, and `lake new` refuses it before writing the
location.

No zone comes from the process's environment (`TZ`), from the host's
clock, or from the root's `lakelands.json`. The fixed `aging.utc_offset`
the root held before is gone; a root that still names it is refused when
it is opened, saying where the zone is set now. 0.23.0 starts lakes
afresh, so nothing is migrated.

### It changes through the setting's door

The zone changes as any lake setting does: an upsert of `sys.setting`
`zone`, through the door the tool catalog generates for `sys.setting`
(`sys_setting_upsert`; the REST upsert of the type; `lakelands settings
set zone Europe/Berlin`), and a retire of it (`lakelands settings unset
zone`) goes back to `UTC`. Who may write it is who may write a lake
setting (063).

### The event that records it is a setting's

A change of zone is the `record_upserted` of `sys.setting` `zone`, and
going back to `UTC` its `record_retired`. No event kind is added and the
log format does not change: the zone's history is the log's, in the
order the lake learned it.

### A write, a rebuild and `verify` find the zone in force at a position

The zone in force at a position is the `zone` setting as the lake's
settings stand there, folded from the log as every setting is (063):
the one the last write of `zone` before the position left, `UTC` where
none did.

- **A write.** `today()` is the day of the lake's calendar holding the
  write's clock, the `known_at` it is written at: that clock cut at
  local midnight in the zone in force where the write is made. A write
  made after a change of zone reads the new zone. The day is held as a date is held, at its UTC midnight
  (011), so `datum <= today()` compares two dates.
- **A rebuild.** The fold reads each event with the settings as they
  stood at it, so a field computed at the write that reads `today()`
  comes out as its write made it, in the zone of then, also after the
  zone changed. A constraint is judged by its write alone and by no walk
  (059), so a write admitted in one zone and a write refused in another
  stay what they were.
- **`verify`.** It checks the chain over the log, and every change of
  zone is an event in it: the zone of any position is what the verified
  log says, and nothing outside the log moves it.
- **A read.** `today()` in a read cuts the read's clock (052) in the
  zone in force at the position read.
- **Aging.** The host chooses the date it hands `age` (036). `serve`
  reads the zone in force at the head (`LakeStore::zone`) at each look
  and cuts the host's clock in it, so the day rolls over at the lake's
  midnight; inside a rule's condition `today()` is the date handed, not
  cut again.

`today()` and the zone of `date_bucket(t, unit, zone)` share one
implementation (`lakelands_api::expr::zone`), on the time-zone database
bundled with the build.

## Why

A lake's day is the one the people who keep it live in. An invoice due
on 15 March is due on the 15th in Berlin, and at half past midnight
there the clock in UTC still reads the 14th: a lake cutting days in UTC
refuses the local today as "in the future" for an hour or more each
night, and the fixed offset aging read moved its rollover by an hour
twice a year. A named zone carries the clock changes with it.

The zone is a setting because every host that opens the lake must cut
the same day, and an auditor must be able to read when the lake's day
changed: that is what 063 puts in the log. Being in the log, the zone of
any write is fixed by the log alone, so a replay reproduces every value
a write computed (002), and no host can make the same log mean another
day by its environment.

The location gives only the start because a new lake must be able to
start in its zone before anyone writes it, yet two homes of one lake
must never disagree about it: the first open turns it into the log's,
and the log decides from then on.

## What it is not

- **Not a zone per type, field, source or principal.** A lake has one
  calendar. A reading wanted in another zone names it: `date_bucket`
  takes a zone.
- **Not a change to instants or dates.** `now()` is an instant, as
  before; a `date` field still holds a calendar date, at its UTC
  midnight; a `date_time` is still UTC on the wire.
- **Not a setting from a date.** A zone changed today does not reach
  back to writes already made, and cannot be scheduled for a later day.
- **Not the host's.** The environment, the host's locale and its
  system zone are never read.
- **Not every calendar reading.** `date_bucket(t, unit)` with no zone
  cuts in UTC, as before; a zone is named as its third argument. A
  function backed by an expression (`expr`) computes from its inputs
  alone, with no lake settings, so `today()` in it is UTC's; a computed
  field that wants the lake's day reads `today()` itself.
- **Not a frozen database.** The zone database is the one the build
  bundles. A later database that redraws a zone's past offsets would
  cut an instant of that past differently in a field a rebuild computes
  again; the rules of the zones in use today are settled for the years
  they cover.
