# 063 — Every setting is in one list

## What is decided

### A setting lives where what it steers lives

A setting that steers a lake is kept in one of three places, by one test:
would two hosts opening the same lake be wrong to differ?

- **The lake.** What every host that opens the lake must find the same,
  and a host's choices about the lake that an auditor should be able to
  read the history of, is a record of `sys.setting`, in the log. The
  store judges it as it is written (`seam::check_setting`) and reads it
  as it stood at the position read. The index settings
  (`sort_index.<type>`, `vector_index.<type>`, `lookup_index.<type>`),
  `search_weights.<type>` and `view_kept.<type>` are kept here.
- **The location.** A bound on what one host spends, which changes no
  answer, is a member of the lake's `Location` (`lakes/<id>.json`), read
  when the lake is opened: the limits and the engines' tuning.
- **The root.** How the root's processes serve, age, deliver, embed,
  call a backing and mirror is a member of the root's `lakelands.json`,
  read when a process opens the root: `server`, `aging`, `deliver`,
  `embed`, `backings` and `mirror`.

What says only where or how to connect, or which keys to use, is a
binding, not a setting: the engine's directory, URL, schema and TLS, the
lake's key handle, the root's keyring, the endpoints, key files and
certificate authorities the deliverers and the embedding client reach,
the hosts and commands the backings may call, and each mirror's
connection. A binding steers no behaviour, and a lake moved or restored
names it anew. `lakelands_api::settings::BINDINGS` names each, at its
layer, in one sentence, and the page lists them apart. The rest of
`lakelands.json` is neither: `lakelands` is the file's format number,
and `owner` and `principals` say who the root's callers are and what
each may do, which is the business of grants, not of a setting.

A new switch goes where this test puts it, and nowhere else: not into
the process's environment, and not into a constant an operator cannot
change when a measured default turns out wrong for a lake.

### One list names them all

`lakelands_api::settings::SETTINGS` names every setting the store, its
engines and the root's processes read: the key, where it is kept, its default,
what values it takes, what it changes in one sentence, and what it
costs where that was measured. `docs/settings.md` is generated from the
list (`scripts/settings.py`), so the page an operator reads cannot drift
from what the code reads. The gate `settings` holds the page to the
list, and the list to the code both ways: a lake setting the store reads
that is not listed is red, and so is a listed one no product code reads.
The store names a setting's key only by its constant in `seam.rs`, so
that the gate sees the read: a literal that is a listed key, a read of
the settings by a literal key (a chain split over lines, a closure over
them and a prefix test in a walk of their keys included), a `*_SETTING`
constant outside `seam.rs`, and the settings bound by `let` to another
name are red too. A gate that reads names sees these shapes and not
every read: a key built from parts, or the map handed on under another
name, is held to the rule by review. Unit tests hold the
location's and the root's settings and bindings to `Location`, `Limits`,
the engines' tuning and the root's sections, member for member and
default for default, both ways.

A host's own keys under `sys.setting`, which its expressions read as
`setting('<key>')`, are its own and are not listed; a host declares them
in the lake (below). A key under the
prefix of a listed lake setting (the key up to its first dot) that names
none of them is refused, naming the prefix: a misspelt setting is told,
never kept as a host's key the store does not read.

### A lake setting may be read for a type

A lake setting is one of three shapes:

- **for the lake:** one value, `<key>`;
- **per type only:** one value per type, `<key>.<type>`, and none for the
  lake, as the index settings are;
- **for the lake, overridden per type:** read for a type as
  `<key>.<type>` where it stands, else `<key>`, else its default.
  Retiring the per-type record hands the type back to the lake's value.

### `describe` tells the lake's settings to a holder of `schema`

`describe` lists, to a principal holding `schema`, each listed lake
setting for the lake, with the value the lake holds and its default, and
each one the lake holds for a type, with the lake's value as its
default. It tells no other reader any: a setting says how the lake is
built, which is the business of whoever may change it. A holder of
`schema` kept from a type is told the settings as were the type absent,
and a setting that names the fields of a type it is shown is told whole
or not at all, as its record is (below). A host's own keys
it has not declared are not told there; a reader reads them through
`sys.setting` under its grants, as any record.

### A setting's record is told as `describe` tells it

A lake setting for a type is a record of `sys.setting`, and its key
names the type. So every door that reads that record tells it as
`describe` tells the setting (lakelands#1198): to a reader kept from the
type, the record is not there. A setting that names the type's fields
is told whole or not at all (lakelands#1250): it is there only to a
reader that reads the type whole, whose grants keep no field of the
type back, by the type's default or by name, and that reads every field
the type declares. To any other reader it is not there. The settings
that name fields are the index settings and `search_weights`, whose
`fields` and `text_fields` name them (lakelands#1230). The records are
told so as the engine hands them to the read, before anything reads
them, so it holds for `get`, `find` with its `where`, its order, its
total and an aggregate, a query's pages, a read as of a position, and a
proposal's write; and for `setting('<key>')` in every expression a read
evaluates for its reader: a `where`, a field computed at the read, a
view computed for it. A view whose computation names a setting, or reads
`sys.setting` or such a view, is computed, never served kept, for a
reader a setting may be withheld from, so it answers what its
computation answers. A reader kept from `secret` reads `sys.setting` as
in a lake where `secret` was never declared. A proposal's write is told
as of where it was proposed: one that tells a reader nothing of a record
it was not told then is not there to it, whatever became of the record
since.

A setting for a source names no type, and a source may feed any type:
the lake keeps no list of what each one feeds. So `key_period.<source>`
is told only to a reader from whom nothing is held, one that holds
`restricted` and reads every field of every type (lakelands#1153). To
any other reader the record is not there, at every door above and in
`describe`, and it reads `sys.setting` as in a lake that never set a
period for the source. The lake's own `key_period` names no source and
is told as any setting for the lake. Each source's period is a record of
its own, so no other source's entry shares it. A writer from whom
something is held may not write it, retire it or propose either: each
is refused as not granted before anything of the record is judged,
alike in a lake that holds it and in one that does not, since the
version a write answers would tell that it exists.

A change notification of such a record compares what its reader is told
of it before the change and after: a change to what it is not told
alone is not told, a record that becomes one it is not told is told as
retired, and one that becomes one it is told is told as the write that
made it, which names every field the record holds. Every reader is
told a setting's change so: a notification of `sys.setting` names a
field only where the value it is told changed. A set's change and a
trigger's firing over `sys.setting` are told only to one from whom
nothing is held, as a condition over a field a reader may not read is
never told it. That departs from the twin, which would tell a
condition that reads no setting narrowed to its reader; it tells less,
never more.

A reader not told such a setting may not write it, retire it or propose
either: each is refused as not granted, naming no field, whatever value
and version it gives, before anything of the record is judged. The rule
is judged from its grants and the type, never from the record, so a lake
that holds the setting and its twin without the field, or without the
setting, answer it alike. Telling it a part of the record could not be
made alike: the record's version, its value's provenance (`known_at`,
`position`, `writer`) and the `latest` order move with every write of
the key, the parts it is not told too, and a write it made over them
would answer a version its twin does not. Keeping the record whole from
it tells it nothing a twin would not; such a setting is the lake's
tuning of a type its writer does not read whole, so it is asked of one
that does. The record stays one per setting and type, and the log keeps
its form.

What the lake does with such a setting is not its record. `describe`
names in the type's lifecycle the fields an index setting sorts or
indexes only to a reader told the setting, and to any other as were it
absent. A search ranks by `search_weights` whoever reads, and names the
weight of each axis its reader is shown, as
`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`
says of a restricted setting: the weight of a field the reader is shown,
never the record's version, its provenance or a field kept from it.

A proposal of such a setting made before this rule, by a proposer that
does not read the type whole, is no longer listed to that proposer, and
accepting it is refused as not granted, since an accept judges it as its
proposer; a reviewer it is listed to may reject it, and one that reads
the type whole may propose it again.

A host that grants by allow-list loses these settings too. A principal
whose grants give a type the default `none`, by its grant for the type
or by their own default, and then name every field it may read, does
not read the type whole, however many fields it names: what decides is
whether anything is kept back, and a default of `none` keeps back every
field the type may come to declare. A grant that keeps back by name a
field the type no longer declares does not either, since a field may be
declared again under that name. Such a principal is told nothing of the
type's settings that name fields, and its writes, retires and proposals
of them are refused as not granted. Its host gives it a reading default
for the type and names no field it may not read.

A write of a type's setting by a writer it is told to is judged as it
wrote it, before its expected version is, so a type it is not shown is
refused as one that is not declared, whatever version it expects.

What the store computes at a write, a derived field stored and a view
kept, reads every setting, as the lake does; a record's grants show it.
An export and a mirror are the owner's and copy the records whole.

### A condition judged once tells what it decides only to one told its settings

Some conditions are judged once for every reader: a trigger's `when`
and a set's `where`, with the sets they name, at the write that moves
them; the `where` and measures of a kept view a trigger or a set
watches, as the lake keeps its rows; a verb's `requires` and the values
it sets, at the call; an aging rule's `when`, at the run. None can be
judged for each reader. A firing and a membership stand in the log, and
a replay decides them from the log alone, where no reader's grants
stand; a call's write and an aging move are made once, for whoever
reads the record after. So each reads `setting('<key>')` as the lake
holds it, as for a principal from whom no setting is withheld.

What such a condition decides is told only to a principal told every
setting it reads, each as `describe` tells it (above), through the sets
it names and the views under the type it watches; the judgement is the
one a read of the setting makes, never a second. A reader not told one
is answered as in a lake that never held it:

- a trigger and a set are not listed to it, a write's answer names
  neither among what it fired, joined or left, and `in_set` answers it
  that the set is not declared, as for a set over a field it may not
  read;
- a verb is not described to it, and its call is refused as not
  granted, before the record is read; a proposed call of it is not
  listed to it and is not found when it accepts it;
- an aging rule's `when` that reads a setting kept for one type or one
  source is refused as invalid for every author, naming the setting:
  the run moves records every reader reads, and the author's grants,
  which would say whether they are told it, are not in the log when
  the rule runs. A rule written before this is skipped at its run, and
  the report names the setting as the reason, as it names any other
  rule that no longer fits. The host's and the lake's own settings are
  told to everyone and stay readable there.

Telling it a narrowed answer could not be made alike: the condition
held or not as the lake holds the setting, and nothing it decided can
be decided again for one reader. Refusing or hiding what it decided
tells no more than a direct read of `sys.setting`, which tells such a
reader no record. A condition evaluated at a read, a view's `where`
among them, reads the settings as its reader is told them (above). A
constraint reads no setting, which is refused at its declaration; a
field derived at the write is the record's (above).

The reader's own write still tells it that something reacted: the
answer's `through` and the position the write lands at cover what the
write set off, a firing withheld from it among them, since positions
are no secret
(`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`).
As for a condition over a field it may not read, it learns that
something reacted, never what: not which trigger or set, nor which
setting or its value.

### `lakelands settings` shows them and writes only the lake's

`lakelands settings` prints every listed setting with the value it has
for one lake, from the lake, its location and the root's configuration;
it only reads, beside a running `serve`. `lakelands settings set` and
`unset` write and retire a lake setting through the store, so a value is
judged as any write of it is and a refusal names the setting and why.
A key of the location or the root, a setting's or a binding's, is
refused with exit status 2, naming the file that keeps it: the command
never edits a file an operator keeps. So are the last parts of such a
key (`max_limit`, `tuning.mmap_mb`), naming the full key, and a
section's name (`deliver`, `tuning`), naming its file: written to the lake, each
would be a host's own key that nothing reads.

### A host declares its own settings

A host's own key under `sys.setting` may be declared, as a type is: a
declaration of kind `setting` names the key, the `type` of its value (a
field type, a value type included), and, for people and agents, its
`unit` and `description`, and the `group` it belongs to. It is declared
again to change and never withdrawn; a bundle declares it in its
`settings/` lane, and the first value beside it, in one `apply`.

- **Its value holds its kind.** Every write of a declared key's `value`
  is judged as a write of a field of the declared type is, and kept in
  the form that write keeps: `"19%"` for a `decimal` is refused, naming
  `value` within the setting. `value` is required, and `null` is
  refused as its absence, for the owner as for anyone: a setting is
  unset by retiring its record (`lakelands settings unset`). So
  `setting('<key>')` reads a declared setting as its kind wherever an
  expression reads it, and an undeclared key reads as the JSON it holds.
- **Its key is the host's.** A key the store reads, or one under such a
  key's prefix, is refused, naming the prefix; so is a key whose held
  value the declared type refuses, naming the setting.
- **Its group decides who writes it.** `sys.setting` holds a `group`,
  which the store stamps at every write: the declaration's group for a
  declared key, `store` for the store's own settings, none for an
  undeclared key. A write naming another group is refused, naming
  `group`. A grant names who writes which settings by its `only` on
  `sys.setting`, as it admits any record, and never by a list of
  principals; a holder outside a group is refused its settings as it is
  refused an undeclared key, and learns nothing of them: to a writer
  the declaration is not told, the key is undeclared, so a value its
  kind refuses and a group guessed right or wrong are answered as they
  are for a key nobody declared. A host may not
  declare into `store`. A declaration that moves a setting to another
  group stamps its record again in the declaration's own append, so no
  record stands in a group its declaration does not name.
- **`describe` tells the declaration** (kind, unit, meaning, group), with
  the value the lake holds, to a holder of `schema` and to a principal
  that may write the setting, and to no other reader. The group itself
  stands in the log as a field of the record: a reader of `sys.setting`
  reads it under that type's grants, as it reads the value, and finds
  the records of a group by it; only the declaration is withheld.

Any expression reads any setting, whatever its reader's grant on
`sys.setting`, save a lake setting for a type or a source, which an
expression evaluated at a read reads as its reader is told it, and of
which one judged once for every reader tells what it decides only to a
reader told it (above): a value
that must be kept from a reader, a secret above all, is never a
setting. `lakelands settings` lists a lake's declared
settings beside the store's, with their group, and `settings set` judges
a value by its declared kind.

`sys.setting` gained `group` as a field added, which reshapes no record:
a writer's open declares the store's own types again, one version on,
where the lake holds an older form of them, and stamps the settings the
lake holds with their group in the same append.
