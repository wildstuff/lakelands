#!/usr/bin/env bash
# Gate 16 — settings.
#
# Every setting that steers a lake is in one list, `SETTINGS` of
# `crates/lakelands-api/src/settings.rs`
# (`docs/decisions/063-every-setting-is-in-one-list.md`). Red when
# `docs/settings.md` is not what the list says; when a lake setting the
# store reads (a `*_SETTING` constant of `seam.rs`) is not listed; when
# a listed lake setting has no such constant, or no product file reads
# it; or when a product file names a listed setting's key by a literal,
# or reads the settings by a literal key. The rules and how each is read are in `scripts/settings.py`;
# a location's and the server's settings are held to their structs by
# unit tests.
. "$(dirname "$0")/lib.sh"

command -v python3 >/dev/null || die "python3 is needed"
python3 scripts/settings.py --check
