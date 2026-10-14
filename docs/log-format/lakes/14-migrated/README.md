# A lake the release before wrote: a migration of a computed field

`migrated14` is a small lake that v0.13.1 (`08e86fa0`), which writes log
format 14, made through its doors on SQLite and exported (`log/`,
`export.json`):
- a `bill` type with `amount`, `note` and a derived `twice = amount * 2`;
- an object set `big` of the bills whose `twice` is over 150;
- three bills, `b1` at 100, `b2` at 50 and `b3` at 400;
- a migration of `bill` that changes only `twice`, to `amount * 1`.

v0.13.1 computed nothing again at that migration: `answers.json` keeps
what it answered after it, `b1` still at `twice` 200 and in `big`.

`upgrade::LAKE_14_MIGRATED_READ` imports the lake on every engine, which
upgrades it (`docs/log-format.md` §6), and holds that this release reads
it as v0.13.1 answered it, and again after a rebuild: a `migrated` of
format 14 is folded as that format wrote it (lakelands#1070). The
directory is never edited.
