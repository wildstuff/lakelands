#!/usr/bin/env bash
# Gate 19 — strict-doors.
#
# Record 110 owes a gate over the door crates and the contract: every
# place a text becomes a value, or a value becomes a contract type, is
# listed in `scripts/strict-doors.sites` and reads through
# `lakelands_api::strict`; and no type in `lakelands-api` whose
# `Deserialize` is derived buffers its input (`flatten`, `untagged`,
# `tag = `, `content = `), unless its own attributes name `try_from` or
# `from`. Red when a call is on the list's tree and not on the list, when
# a listed call is gone, or when a buffered type is not exempted with an
# issue number. The rules and how each is read are in
# `scripts/strict-doors.py`; the list is regenerated with `--write`.
. "$(dirname "$0")/lib.sh"

command -v python3 >/dev/null || die "python3 is needed"
python3 scripts/strict-doors.py --check
