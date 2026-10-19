# 058 — A source may be forgotten

## What is decided

Decided by the operator on lakelands#1042; it is built in 0.15.0. A
lake 0.14.0 writes holds every value in plaintext, and the last rule
below says what that means.

### A field may be declared erasable, and a source's values for it are kept encrypted under that source's key

- **The declaration:** a field declares `erasable: true`.
- **The encryption:** every value a source writes to an erasable field is encrypted before the event is made. The key is the source's own: one data key per source id.
- **What stays plaintext:**
  - the event's envelope, its kind, the type, the record key and `version`;
  - every non-erasable field;
  - every value an author or the lake itself wrote.

The unit is the source, as for a purge (039), because a source is what a host can name when a person or a device must be forgotten. A person who reached the lake through several sources is forgotten source by source; a host that wants a person to be one unit gives them their own sources.

### A host forgets a source in one act, and the act is in the log

`forget_source(source)` works in this order, under the writer's lock:
1. It appends one `source_forgotten` event: the source, the versions of its key, the writer and the time, with a `record_retired` for each record the source alone brought. The act is recorded as a purge is, and nothing leaves the log.
2. The source's keys are dropped from memory, and the host's key store is asked to destroy them, every version.
3. The projection is rebuilt from the log (below).
4. The engine reclaims the storage the erased values held (below).

**A crash between the steps** leaves the event in the log. The writer's next open finishes what is owed, and each step is idempotent: destroying what is destroyed changes nothing, and a rebuild folds the same log to the same state. Nothing reads the source's values from the log in between, since its keys are out of memory; the projection holds them until the rebuild, and an ingest from the source is refused from the event on. The projection keeps, under a name of the store's own, which acts are owed, so the open knows what is left, and a repeat of the act finishes it too, appending nothing again. A handle opened only to read finishes nothing: after a crash between the event and the rebuild it answers the plaintext the projection still holds until a writer opens, and `hosting.md` tells the host to open one.

**After the act:**
- every value the source wrote to an erasable field reads as **erased**, at every position, `as_of` included;
- the records the source alone brought are retired, as a purge retires them;
- a co-owned record stays: its other values are another hand's, so it is not refused whole as a purge refuses it.

A forgotten source id is never given a key again, so an ingest from it is refused. A device that comes back is a new source.

### Keys are a seam the host configures

**The seam:** a key store with four calls:
- `create(source)`, which answers a key id;
- `key(source, id)`, which answers the key or "destroyed";
- `destroy(source)`;
- `destroy_version(source, id)`, which destroys that version with every older one (lakelands#1153);
- `ids(source)`, its versions, for rotation.

**Where it sits:** it is an eighth seam beside the seven of `docs/seams.md`, configured through `Location` and the server's configuration like every other, never the environment (005). The CLI and `serve` bind the file keyring the root's `lakelands.json` names (`"keys": {"keyring": "<directory>"}`), and refuse one inside a lake's directory.

**What Lakelands ships:**
- a file keyring, for one machine and for tests: one file per lake, kept outside the lake directory, with a key per source;
- the trait, for the host's own KMS (cloud KMS, Vault, an HSM).

**Where the keys live:** never in the lake. Not in its log, its projection or its export: a key in the log could not be destroyed, and a wrapped key in the log would make the wrapping key the one to destroy, for every source at once.

**What the operator configures:**
- the key store and its location;
- that its backups follow the store's destroys: a key restored from a backup restores what it encrypted, and only the operator can keep backups honest.

**The writer** asks for a source's key at its first erasable value and holds it in memory for the open lake. `forget_source` drops it from memory before it destroys it.

**Versions die oldest first.** A version of a source's key is destroyed only with every older one. An output is sealed under the oldest version among the values its call reads, so with versions destroyed in that order the output goes with the first of its values to lose its key, and never outlives one. A version destroyed with no act in the log is a lost key, as below: a value or a digest under it fails closed. The lake holds the keys it was given in memory while it is open, so a version is destroyed outside an act only with the lake closed. A key period (`key_period`, for the lake or for a source: `day`, `week` or `month`) makes the writer ask for a new version at a source's first value past each period's start, so each period's values are sealed apart. A source's own period is told only to a reader from whom nothing is held, since a source may feed any type (063).

### What is encrypted, and how a duplicate is still a duplicate

**Values only.** The members that make the log readable and walkable stay plaintext, and so do those that make the log verifiable. So these are **refused `erasable` at declaration**:
- a key field: its value is the record key, in every event and every reference;
- a `reference` field: its value is another record's key, and a walk reads it.

Also refused: `erasable` on a field only authors or the fold write, whose
values the key of no source seals, and on a relation's field. A field
once erasable is never declared without it: a source's later values would
enter the log in plaintext, and a declaration cannot unseal what is
sealed (lakelands#1042).

A type whose key holds personal data is given a surrogate key. `hosting.md` says so, and also asks for **opaque source ids**: the source id is plaintext in every event's envelope, and in `source_forgotten` itself.

**The value form.** An event marks out of band which patch members are ciphertext, in a new member `sealed: { "key": <key id>, "fields": [..] }`. The marking cannot be in the value: a `json` field may hold any object, so an in-band marker could be taken for a value. The member holds the base64 of the AEAD's output.

**The cipher:** AES-256-GCM-SIV. Its nonce is derived from the source, the type, the key, `version_before` and the field, so the same value against the same state of the record encrypts to the same bytes. This keeps the content address of `docs/log-format.md` §3:
- **The ingest id** is computed over the patch as stored, ciphertext included. No digest of a plaintext value enters the log; a digest of a birth date could be reversed by trying every date.
- **A redelivery** against the same state carries the same ciphertext, so it gets the same id and appends nothing.
- **A genuine return** (A, B, A) meets another `version_before`, so another nonce and another id, as today.
- **"An ingest that changes nothing appends nothing"** still compares the incoming patch with the record as the fold holds it, in plaintext, before anything is encrypted.
- **Equality does not leak** across records or versions, since the nonce differs. Within one record at one state it would, which is the redelivery the id is for.

**Recorded outputs.** A function backing or an embedding that reads an erasable value records an output computed from it (`output_recorded`). That output is encrypted under the same source's key, and is erased with it.
- Its `input`, the digest a replay finds the output by, is an HMAC under a key derived from the source's key, the version that seals the output: the oldest of those that sealed the values the call reads. So the output lives as long as the oldest key period it read, and destroying a version, with every older one, takes the outputs of its values. A SHA-256 of the call would be a digest of plaintext that outlives the key (lakelands#1042).
- **What equality still tells.** A value's own ciphertext tells no equality across records or versions. An output is recorded once for one call (053), so its `input` is the same for two records of one source whose call reads the same values: the log tells that they are equal, never what they are, and never across sources. That is the price of recording once.
- An output whose inputs hold erasable values of two sources records no value, and names the reason in its `error`. One key per output is all the first version carries.
- **What a reaction still tells.** A `trigger_fired` or an `object_set_changed` in the log, an export or the raw feed tells that its condition held for a record at a position. The condition is declared and readable, so a trigger over an erasable field tells something of the value after its key is destroyed, as equality does. The doors and the deliverer do not tell one the act overruled (065). A host whose triggers read an erasable field should know that the log keeps their firings.

### The projection holds plaintext, and the act rebuilds it

**Why plaintext:** the projection decrypts at the fold, so reads, filters, orders, search and indexes work on values as they do today. It never holds a key.

**The act scrubs by rebuilding.** After the keys are destroyed, the projection is folded again from the log. The fold meets each sealed value of the source with no key to open it, and a `source_forgotten` of that source in the log, so it folds the value as erased: cleared, its provenance kept and marked. So every state the projection keeps holds the erasure, history included, and with it:
- the derived fields and views computed from the value, which the fold computes without it;
- an output a backing recorded under the source's key, which is dropped, and a fill that names it, which fills nothing;
- the index entries the value made: the text and sort indexes, full text, vectors, which the rebuild never makes;
- a membership the log holds, decided at a write over the value, which the fold decides again without it (065).

**A key the key store does not give, with no act in the log,** is lost, not destroyed: the fold fails closed, and the value is never read as erased. A write or a declaration whose fold would meet such a value, a re-plan of an output digested under the lost version among them, plans it before it appends and is refused `key_missing`, naming the source and the version, with nothing appended: the head never moves past an event that does not fold (lakelands#1218). A rebuild meets the values before the act that forgot them, so where a key is missing it looks ahead in the log for that act.

The projection is the fold of the log and the key store by construction (001), the log including the act: there is no second path, an incremental scrub of the states kept, that would have to come out equal to a rebuild.

**Reads after:**
- **An erased value is absent:** the field holds no value, as if cleared.
- **With `with_meta`,** its provenance stays: the origin `source`, the source, the position. It gains `erased: true`, so a reader tells "erased" from "never written", and keeps it over a later migration that keeps the field (lakelands#1228). This is not masked: a mask says the value exists and is withheld from this reader, while an erased value exists for no reader.
- **A record** whose every value was erased is retired (above). A co-owned one is read with the other hands' values.

### The engine's storage is reclaimed

Deleting a row leaves its bytes in free pages until the engine reclaims them. So after the rebuild, the act runs the engine's own reclaim, and its report says what it measured:
- **SQLite:** `VACUUM`, which writes the database anew, so no freed page survives it, then a WAL truncate, which drops the old pages from the journal;
- **PostgreSQL:** `VACUUM` of the tables it touched;
- **pgvector:** the `VACUUM` repairs the index graph after deletes (040's one exception).

A reclaim that fails is owed, never a lock-out: the values are erased already, so the writer's open goes on without it, says so, and tries it again at the next open (lakelands#1161). The act answers its report all the same, naming the cause in `reclaim_owed`, and `rebuild()` answers too; an import answers, and the writer's next open tells the cause. A compact that runs is the reclaim, and clears it (lakelands#1179).

**Out of the lake's reach:** a PostgreSQL WAL archive, a disk snapshot, a backup. They are the host's.

### Search, order indexes, vectors and full text

All four are built from the projection, which holds plaintext. They work on erasable fields unchanged, and the scrub removes what the erased values put there.

**What nothing makes possible** is searching the log itself without the keys: a verifier or an export reader without them sees the ciphertext and the plaintext members.

### Exports and mirrors made before

**The feed** delivers events as the log holds them, sealed values
included: an export reads through it and its hashes need the stored
bytes. Beside them it names the reactions the act overruled (065). A
reader that needs plaintext reads the records through the doors, or
opens the values with the key store (lakelands#1042).

**Exports:**
- An export carries the log, so one made after the feature holds the source's values only as ciphertext, and the destroyed key covers it.
- Importing an export needs the key store. Without one, an export that holds the act imports, and the forgotten source's values arrive erased; one that holds a sealed value of any other source is refused as unbound before anything is written, since its values would be lost (lakelands#1042).
- An export made before the feature, or of plaintext data, is out of reach. The act cannot know where it went.

**Mirrors:**
- A mirror writes its copies into another lake through `ingest`, as that lake's source `mirror:<lake>:<type>` (042). There an erasable field is encrypted under that mirror source's key.
- A mirror cannot tell the target which of its copies came from the forgotten source.
- The mirror reads its records through the doors, as its principal, so it needs no key, and the feed does not say which records a source wrote. So **a mirror that reads `source_forgotten`** stops **each mapped type whose source type has an erasable field**, the only fields whose copies the act would have reached, past the act, and names the source: it fails closed as at a migration it cannot follow (042). The host then copies the type again, or forgets the mirror source in the target, and restarts; a type with no erasable field goes on (lakelands#1042).

The first version carries no per-copy erasure across lakes.

### An erasure is read before it is made

`forget_preflight(source)` answers what the act would take and appends nothing. Its report carries an id that `forget_source` may repeat, as a purge's may (039); the act refuses a report of another head. Its report:
- per type, the records holding a value of the source in an erasable field: a count, and the first ten keys;
- the records it would retire, which the source alone brought, with the co-owned records named apart;
- per field, the values it would erase, and the recorded outputs;
- the derived fields and views that recompute;
- the mirror connections whose types it would stop;
- **the values of the source that are plaintext in the log**: written before the field was erasable, or before the feature. Shredding cannot reach them, so the act refuses while they exist and names the count;
- **the time it expects to hold the writer's lock**: the log's events at the engine's measured rebuild rate, so a host can choose its moment. Every write waits while the act rebuilds.

The preflight and the act each count from the source's own events, which the feed index names (`ProjectionView::of_source`), not from a walk of the whole log; where the index decides nothing, as while the projection is rebuilt, they walk the log from there. The judgement is the same either way: a field erasable now, under the names it bears now.

### A lake 0.14.0 writes holds plaintext, and the feature starts where it starts

The feature is a format step: the `sealed` member, the declaration's `erasable`, and `source_forgotten`. From 0.14.0 on, every format step carries its converter (054). A lake 0.14.0 wrote is upgraded and read, and its events keep their bytes, so its values stay plaintext.

**Data written before the feature cannot be shredded.** `forget_preflight` names it, and `forget_source` refuses it. The way out is the host's own:
- export the lake;
- leave the source out;
- import that as a new lake.

The new lake has its own log and its own hashes. Lakelands does not rewrite a log for it.

**0.14.0 reserves nothing in its format.**
- The marking is out of band, in a member a later format adds, so no value 0.14.0 writes can be mistaken for ciphertext.
- `declared` gains `erasable` by the same step.
- What 0.14.0 can already do is in `hosting.md`: opaque source ids, and surrogate keys for types that will hold personal data. Both make a lake ready to opt in later.

Part of a source, what it wrote before a time or in one delivery, is
taken back by an act of its own, which redacts and, where the key
periods allow, destroys versions of this key
(`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).

## Why

A host that ingests from devices people use holds personal data, and a person may ask to be forgotten. Retiring is not forgetting: `as_of` still reads what the source wrote.

Removing the events would make the log a second truth (001, 054), and break every hash, checkpoint, export and mirror position that anyone holds (020).

Destroying a key forgets the values without touching a byte of the log:
- the chain still verifies;
- exports and mirrors stay valid;
- the act itself is an event, with who did it and when.

What it gives up is plaintext in the log for the fields that opt in. Those fields are declared, so a host that does not need them pays nothing.

## What it is not

- **It is not a door.** An agent cannot forget a source. The act is the host's, beside `purge_source`, `rebuild` and `age`.
- **It is not encryption at rest.** The projection holds plaintext, and the engine's own encryption, or the disk's, is still the way to protect it at rest (020).
- **It does not reach** data written before a field was erasable, copies outside the lake (backups, archives, a host's own copies, an export made before), or values a person authored.
- **It does not erase keys, types, positions or source ids**, which stay plaintext by design.
- **It is not a signature, and not access control.** Grants and masks are unchanged.

## How it came to be

- The act scrubs by rebuilding the projection after the keys are destroyed, where it was first written as an incremental scrub of every kept state before the keys were destroyed. The operator decided it (lakelands#1042): the projection is then the fold of the log and the key store by construction, and no second path has to stay equal to a rebuild. The price is the rebuild's time under the writer's lock, which the preflight names; an incremental scrub, for hosts that forget often on large lakes, is its own later step.
- The preflight and the act read the source's events by the feed index, where each walked the whole log, the act under the writer's lock: about 6 s at 200 000 events, on either engine, to count what the source's 2 000 held (lakelands#1151).
