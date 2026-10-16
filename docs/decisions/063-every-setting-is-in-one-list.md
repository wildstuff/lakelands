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
`schema` kept from a type or a field is told the settings as were the
type or the field absent: nothing of a type it is not shown, and of one
it is shown only the fields it reads, a setting left naming none not
told at all. A host's own keys
it has not declared are not told there; a reader reads them through
`sys.setting` under its grants, as any record.

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
`sys.setting`: a value that must be kept from a reader, a secret above
all, is never a setting. `lakelands settings` lists a lake's declared
settings beside the store's, with their group, and `settings set` judges
a value by its declared kind.

`sys.setting` gained `group` as a field added, which reshapes no record:
a writer's open declares the store's own types again, one version on,
where the lake holds an older form of them, and stamps the settings the
lake holds with their group in the same append.
