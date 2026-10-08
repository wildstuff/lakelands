//! Why a lake cannot answer now, and which seam a call needs: closed sets
//! of cases with their parameters as data. A case's sentence is its
//! template (`catalog!`, in `case.rs`), and a plain name's is written
//! beside its name (`named!`); neither is built elsewhere. A host
//! renders the case in its users' language, and the sentence is the
//! fallback and what is logged
//! (`docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
//! extended).
//!
//! On the wire a case is an object: `code`, its parameters, and the
//! `message` its sentence makes. A code this build does not know is read
//! as [`Unknown`], keeping the code, its parameters and its message.

use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::{Error, Param, catalog, render};
use crate::id::{Position, ProposalId, RecordKey, TypeName};
use crate::time::Timestamp;
use crate::tools::NameProblem;

/// A case read from a newer lake, whose code this build does not know.
/// Only the wire's reader makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unknown {
    code: String,
    params: Map<String, Value>,
    message: String,
}

impl Unknown {
    /// The case's code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Its parameters, as the wire carried them.
    pub fn params(&self) -> &Map<String, Value> {
        &self.params
    }

    /// The sentence the lake sent with it.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Kept with the sentence of the error that carried it, for a set
    /// whose cases carry no sentence of their own.
    pub(super) fn says(&mut self, message: String) {
        self.message = message;
    }
}

/// Text a foreign library wrote about a failure (an operating system, a
/// database, TLS, an HTTP client, a JSON parser), kept only as one of its
/// constructors scrubs it: never a record's key or values, and never a
/// path on the host. Its constructors are called only where a gate
/// allows (`scripts/gates/detail-sources.sh`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Detail(String);

impl Detail {
    /// An operating system's failure: its kind and its own text, which
    /// name no path (a path is in the `io::Error`'s context, not here).
    pub fn io(e: &std::io::Error) -> Detail {
        let os = e.raw_os_error().map(|_| e.to_string());
        Detail(os.unwrap_or_else(|| e.kind().to_string()))
    }

    /// A JSON parser's failure: what kind it is and where, never the text
    /// it quotes, which may be a record's value.
    pub fn json(e: &serde_json::Error) -> Detail {
        let kind = match e.classify() {
            serde_json::error::Category::Io => "an I/O failure",
            serde_json::error::Category::Syntax => "not JSON",
            serde_json::error::Category::Data => "not of the expected shape",
            serde_json::error::Category::Eof => "cut short",
        };
        Detail(format!(
            "{kind}, at line {} column {}",
            e.line(),
            e.column()
        ))
    }

    /// A database's or a library's own words for a failure, as its caller
    /// scrubbed them: an error's code and primary message, never its
    /// detail lines.
    pub fn scrubbed(said: impl fmt::Display) -> Detail {
        Detail(said.to_string())
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Detail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A closed set of cases with a fallback for a code this build does not
/// know: its derived serde form is internally tagged by [`Closed::TAG`],
/// and the fallback is skipped by it.
pub trait Closed: Sized + Serialize + DeserializeOwned + fmt::Display {
    /// The member that names the case.
    const TAG: &'static str = "code";

    /// Whether a case carries its sentence as `message`. A set whose cases
    /// do not is said by the error that carries it.
    const SAYS: bool = true;

    /// The fallback, holding what the wire carried.
    fn unknown(unknown: Unknown) -> Self;

    /// The fallback, if this is it.
    fn as_unknown(&self) -> Option<&Unknown>;
}

/// The codes a derived, internally tagged serde form knows, as serde
/// itself lists them when it meets a code it does not know.
fn codes<T: DeserializeOwned>(tag: &str) -> &'static [&'static str] {
    #[derive(Debug)]
    struct Codes(&'static [&'static str]);
    impl fmt::Display for Codes {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{:?}", self.0)
        }
    }
    impl std::error::Error for Codes {}
    impl serde::de::Error for Codes {
        fn custom<M: fmt::Display>(_: M) -> Codes {
            Codes(&[])
        }
        fn unknown_variant(_: &str, expected: &'static [&'static str]) -> Codes {
            Codes(expected)
        }
    }
    // No code is the empty name.
    let probe = serde::de::value::MapDeserializer::<_, Codes>::new(std::iter::once((tag, "")));
    T::deserialize(probe).err().map_or(&[], |Codes(c)| c)
}

/// The serde form of a [`Closed`] value: its derived form plus `message`,
/// and an unknown code read as the fallback.
pub mod closed {
    use super::*;

    /// Write the case, its parameters and its sentence.
    pub fn serialize<T: Closed, S: Serializer>(v: &T, s: S) -> Result<S::Ok, S::Error> {
        let mut map = match v.as_unknown() {
            Some(u) => {
                let mut map = u.params.clone();
                map.insert(T::TAG.into(), Value::String(u.code.clone()));
                map
            }
            None => match serde_json::to_value(v).map_err(serde::ser::Error::custom)? {
                Value::Object(map) => map,
                other => return Err(serde::ser::Error::custom(format!("{other} is no case"))),
            },
        };
        if T::SAYS {
            map.insert("message".into(), Value::String(v.to_string()));
        }
        map.serialize(s)
    }

    /// Read a case. A code this build does not know is the fallback, and
    /// so is a known code whose parameters it cannot read (one a newer lake
    /// changed or added), so that the error around it still reads.
    pub fn deserialize<'de, T: Closed, D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
        let Value::Object(mut map) = Value::deserialize(d)? else {
            return Err(serde::de::Error::custom("a case is an object"));
        };
        let message = match T::SAYS.then(|| map.remove("message")).flatten() {
            Some(Value::String(m)) => m,
            _ => String::new(),
        };
        let code = match map.get(T::TAG) {
            Some(Value::String(c)) => c.clone(),
            _ => return Err(serde::de::Error::missing_field(T::TAG)),
        };
        if codes::<T>(T::TAG).contains(&code.as_str())
            && let Ok(known) = T::deserialize(Value::Object(map.clone()))
        {
            return Ok(known);
        }
        map.remove(T::TAG);
        let params = map;
        Ok(T::unknown(Unknown {
            code,
            params,
            message,
        }))
    }

    /// A case that may not be there, in the serde form of a [`Closed`]
    /// value when it is.
    pub mod option {
        use super::*;

        /// Write the case, if one.
        pub fn serialize<T: Closed, S: Serializer>(v: &Option<T>, s: S) -> Result<S::Ok, S::Error> {
            match v {
                Some(v) => super::serialize(v, s),
                None => s.serialize_none(),
            }
        }

        /// Read the case, if one.
        pub fn deserialize<'de, T: Closed, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<T>, D::Error> {
            match Option::<Value>::deserialize(d)? {
                None | Some(Value::Null) => Ok(None),
                Some(v) => super::deserialize(v)
                    .map(Some)
                    .map_err(serde::de::Error::custom),
            }
        }
    }

    /// A list of cases, each in the serde form of a [`Closed`] value.
    pub mod list {
        use super::*;

        /// Write each case.
        pub fn serialize<T: Closed, S: Serializer>(v: &[T], s: S) -> Result<S::Ok, S::Error> {
            let each = v
                .iter()
                .map(|c| super::serialize(c, serde_json::value::Serializer));
            let each = each.collect::<Result<Vec<_>, _>>();
            each.map_err(serde::ser::Error::custom)?.serialize(s)
        }

        /// Read each case.
        pub fn deserialize<'de, T: Closed, D: Deserializer<'de>>(d: D) -> Result<Vec<T>, D::Error> {
            let each = Vec::<Value>::deserialize(d)?
                .into_iter()
                .map(super::deserialize);
            each.collect::<Result<_, _>>()
                .map_err(serde::de::Error::custom)
        }
    }
}

impl<T: Closed> Closed for Box<T> {
    const TAG: &'static str = T::TAG;
    const SAYS: bool = T::SAYS;

    fn unknown(unknown: Unknown) -> Self {
        Box::new(T::unknown(unknown))
    }

    fn as_unknown(&self) -> Option<&Unknown> {
        T::as_unknown(self)
    }
}

/// A closed set of plain names, written as its name and said as its
/// sentence. A name this build does not know is kept, written back as it
/// came, and said as itself in backticks, a backtick or a backslash in it
/// escaped by a backslash, which no known name's sentence is.
macro_rules! named {
    (
        $(#[$meta:meta])*
        pub enum $set:ident {
            $( $(#[$doc:meta])* $case:ident = $name:literal => $sentence:literal, )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum $set {
            $( $(#[$doc])* $case, )*
            /// A name this build does not know.
            Unknown(String),
        }

        impl $set {
            /// Its name on the wire.
            pub fn name(&self) -> &str {
                match self {
                    $( $set::$case => $name, )*
                    $set::Unknown(n) => n,
                }
            }
        }

        impl fmt::Display for $set {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    $( $set::$case => f.write_str($sentence), )*
                    $set::Unknown(n) => {
                        let n = n.replace('\\', "\\\\").replace('`', "\\`");
                        write!(f, "`{n}`")
                    }
                }
            }
        }

        impl Param for $set {
            fn said(&self) -> Option<String> {
                Some(self.to_string())
            }
        }

        impl Serialize for $set {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.name())
            }
        }

        impl<'de> Deserialize<'de> for $set {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<$set, D::Error> {
                let name = String::deserialize(d)?;
                Ok(match name.as_str() {
                    $( $name => $set::$case, )*
                    _ => $set::Unknown(name),
                })
            }
        }
    };
}

pub(crate) use named;

catalog! {
    /// A seam the call needs and the host has not bound.
    pub enum Seam {
        /// A verb's executor, by the name the verb declares.
        Executor { name: String } => "the verb executor `{name}`",
        /// An embedding backend for a field's model: none is bound, or one of another model or length.
        Embedding {
            model: String,
            dimensions: u32,
            #[serde(rename = "type")]
            ty: TypeName,
            field: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            bound: Option<Bound>,
        } => "the embedding model `{model}` with {dimensions} dimensions, for `{ty}.{field}` [(the bound one is {bound})|(none is bound)]",
        /// A function backing of a kind.
        FunctionBacking { kind: String } => "the `{kind}` function backing (the host bound none of that kind)",
        /// A vector index, which this engine keeps none of.
        VectorIndex => "a vector index (this engine keeps none)",
        /// An engine, whose opener was not passed in.
        Engine { name: String } => "the engine `{name}` (no opener for it was passed in)",
        /// An operation of the host's that this door does not offer.
        Operation { op: String } => "the host operation `{op}`, which this door does not offer",
    }
}

/// An embedding backend bound for a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    /// Its model.
    pub model: String,
    /// Its length.
    pub dimensions: u32,
}

/// Said inside [`Seam::Embedding`]'s template, by a template of its own.
impl Param for Bound {
    fn said(&self) -> Option<String> {
        struct Said<'a>(&'a Bound);
        impl fmt::Display for Said<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let Bound { model, dimensions } = self.0;
                let params = [("model", model.said()), ("dimensions", dimensions.said())];
                render(f, "`{model}` with {dimensions}", &params)
            }
        }
        Some(Said(self).to_string())
    }
}

named! {
    /// What a write left undone where its lease did not hold.
    pub enum Act {
        /// Nothing was appended.
        Appended = "appended" => "nothing was appended",
        /// Nothing was imported.
        Imported = "imported" => "nothing was imported",
        /// Nothing was sealed.
        Sealed = "sealed" => "nothing was sealed",
    }
}

named! {
    /// A part of a lake's storage, or of what is read into one.
    pub enum Part {
        /// The log.
        Log = "log" => "the log",
        /// An index of the log's segment files.
        LogIndex = "log_index" => "the log index",
        /// A segment of the log.
        Segment = "segment" => "a segment of the log",
        /// A checkpoint in the log.
        Checkpoint = "checkpoint" => "a checkpoint",
        /// The file that records the writer lease.
        LeaseFile = "lease_file" => "the lease file",
        /// The projection.
        Projection = "projection" => "the projection",
        /// The place that holds a lake's name.
        Name = "name" => "the lake's name",
        /// An export's files.
        Export = "export" => "the export",
        /// A change feed being read.
        Feed = "feed" => "the feed",
        /// A file.
        File = "file" => "a file",
    }
}

named! {
    /// Why a vector gives a search no direction to rank along.
    pub enum Aimless {
        /// A component is not a finite number.
        NotFinite = "not_finite" => "has a component that is not a finite number",
        /// Every component is 0.
        Zero = "zero" => "has no direction: every component is 0",
    }
}

named! {
    /// What the storage was doing when it failed.
    pub enum Doing {
        /// Running a statement.
        Query = "query" => "running a statement on",
        /// Creating.
        Create = "create" => "creating",
        /// Opening.
        Open = "open" => "opening",
        /// Reading.
        Read = "read" => "reading",
        /// Writing.
        Write = "write" => "writing",
        /// Syncing.
        Sync = "sync" => "syncing",
        /// Renaming.
        Rename = "rename" => "renaming",
        /// Truncating.
        Truncate = "truncate" => "truncating",
        /// Locking.
        Lock = "lock" => "locking",
        /// Unlocking.
        Unlock = "unlock" => "unlocking",
        /// Measuring.
        Measure = "measure" => "measuring",
        /// Compacting.
        Compact = "compact" => "compacting",
    }
}

named! {
    /// What a failed append left in the log's files.
    pub enum Left {
        /// Nothing.
        Nothing = "nothing" => "it left nothing in the log's files",
        /// Its events, whose sync failed: every handle reads them, and the outcome is not known.
        Unsure = "unsure" => "its events are in the log's files and their sync failed: every handle reads them, and the outcome is not known",
        /// What it wrote may lie past the committed end: the next append or seal cuts it before it writes, or is refused.
        Uncut = "uncut" => "what it wrote may still lie past the committed end, and the next append or seal cuts it before it writes, or is refused",
    }
}

named! {
    /// How a call of a verb that reaches outside the lake, not handed to its executor, is sent again.
    pub enum Retry {
        /// Call the verb again.
        Call = "call" => "call the verb again",
        /// Call it again under another key or with none: its key answers what is recorded.
        OtherKey = "other_key" => "sending it again under its key will not run it: call the verb again under another key or with none",
        /// Propose the call again: the proposal is decided.
        Propose = "propose" => "the proposal is decided, so accepting it again will not run it: propose the call again",
    }
}

impl Retry {
    /// What sending the call again answers where its executor's answer is
    /// held: nothing for a call with no key, which runs anew.
    pub fn resent(self) -> Option<Resent> {
        match self {
            Retry::Call => None,
            Retry::OtherKey => Some(Resent::OtherKey),
            Retry::Propose => Some(Resent::Propose),
            Retry::Unknown(n) => Some(Resent::Unknown(n)),
        }
    }
}

named! {
    /// How a call of a verb that reaches outside the lake, whose executor's answer is held, is answered when it is sent again.
    pub enum Resent {
        /// Sent again under its key, the call is answered.
        OtherKey = "other_key" => "sent again under its key, the call answers it",
        /// Accepted again, the proposal is answered.
        Propose = "propose" => "accepted again, the proposal answers it",
    }
}

named! {
    /// A TLS setup that failed, on the side of the lake's own connection.
    pub enum Tls {
        /// The certificate authority file cannot be read.
        CaUnreadable = "ca_unreadable" => "the location's `tls.ca_file` cannot be read",
        /// The certificate authority file holds no certificate.
        CaEmpty = "ca_empty" => "the location's `tls.ca_file` holds no certificate",
        /// No certificate bundle is at a well-known path.
        NoBundle = "no_bundle" => "no certificate bundle was found at a well-known path; name one as the location's `tls.ca_file`, or set `tls.mode` to `disable` for a server you reach without TLS",
        /// TLS cannot be set up with what was given.
        Setup = "setup" => "the library refused the setup",
        /// The client key cannot be read.
        KeyUnreadable = "key_unreadable" => "the location's `tls.client_key` cannot be read",
        /// The client certificate file cannot be read.
        CertUnreadable = "cert_unreadable" => "the location's `tls.client_cert` cannot be read",
        /// The client certificate file holds no certificate.
        CertEmpty = "cert_empty" => "the location's `tls.client_cert` holds no certificate",
        /// The client certificate cannot be used.
        CertUnusable = "cert_unusable" => "the client certificate cannot be used",
    }
}

catalog! {
    #[derive(Eq)]
    /// A remote end a call reaches: an embedding backend, a function's backing, a REST door.
    pub enum Remote {
        /// The embedding backend of a model.
        Embedding { model: String } => "the embedding backend `{model}`",
        /// A verb's executor.
        Executor { name: String } => "the verb executor `{name}`",
        /// A function's backing.
        Function { function: String } => "function `{function}`",
        /// A lake's REST door.
        Door { lake: String } => "the REST door of lake `{lake}`",
    }
}

catalog! {
    /// What went wrong in an exchange with a remote end.
    pub enum Exchange {
        /// The request cannot be made as configured.
        CannotRequest { detail: Detail } => "cannot be requested: {detail}",
        /// Its host is not a name TLS can verify.
        NotVerifiable { detail: Detail } => "its host is not a name TLS can verify: {detail}",
        /// No connection was made.
        CannotConnect { detail: Detail } => "cannot be connected to: {detail}",
        /// TLS with it failed.
        TlsFailed { detail: Detail } => "TLS with it failed: {detail}",
        /// TLS cannot be set up: no usable certificate bundle.
        NoBundle => "TLS cannot be set up: no usable certificate bundle; the operator names one as `ca_file`",
        /// HTTP failed.
        Http { detail: Detail } => "HTTP failed: {detail}",
        /// The answer's body could not be read.
        Body { detail: Detail } => "its body could not be read: {detail}",
        /// The body was larger than the limit, in bytes.
        TooLarge { cap: u64 } => "answered more than {cap} bytes at once; raise `max_answer_bytes` if the lake's records are that large",
        /// It answered a status it was not asked for.
        Status { status: u16 } => "answered {status}",
        /// It answered nothing within the time allowed.
        TimedOut { secs: u64 } => "gave no answer within {secs} seconds",
        /// It answered with a body that is not the contract's, under a status if the body is its.
        NotTheContract {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            status: Option<u16>,
        } => "answered [{status} ]with a body that is not the contract's",
        /// It sent a line or a block that is not the contract's.
        LineNotTheContract => "sent a line that is not the contract's",
        /// It answered the feed without the position.
        NoPosition => "answered the feed without its position",
        /// It sent nothing, not even a keep-alive.
        Silent { secs: u64 } => "sent nothing for {secs} seconds, not even a keep-alive",
        /// It took nothing and answered nothing while records were being sent.
        Stalled { secs: u64 } => "took nothing and answered nothing for {secs} seconds while records were being sent",
        /// It sent nothing after the records ended.
        SilentAfter { secs: u64 } => "sent nothing for {secs} seconds after the records ended",
        /// It ended the stream.
        Ended => "ended its stream",
        /// It cut its answer off.
        CutOff { detail: Detail } => "cut its answer off: {detail}",
        /// The request cannot be sent its key.
        KeyNotSent => "cannot be sent its key",
        /// The request cannot be built.
        NotSent => "cannot be sent this request",
        /// It answered other vectors than texts asked.
        Vectors { answered: u64, asked: u64 } => "answered {answered} vectors for {asked} texts",
        /// It answered a vector with no direction to rank along.
        NoDirection { why: Aimless } => "answered a vector that {why}",
        /// It answered what is not JSON.
        NotJson { detail: Detail } => "answered what is not JSON: {detail}",
        /// It refused the connection.
        ConnectionRefused => "refused the connection",
        /// TLS with it failed, in the library's words if it gave any.
        TlsBroke {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            detail: Option<Detail>,
        } => "failed TLS[: {detail}]",
        /// It cannot be reached, in the operating system's words if it gave any.
        Unreachable {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            detail: Option<Detail>,
        } => "cannot be reached[: {detail}]",
        /// It closed the connection.
        Closed => "closed the connection",
        /// It gave no whole answer within the time allowed.
        NoWholeAnswer { secs: u64 } => "gave no whole answer within {secs} seconds",
        /// It answered a status that is not the contract's.
        UnexpectedStatus { status: u16 } => "answered {status}, which is not the contract's",
        /// It answered a redirect, which is not followed.
        Redirect { status: u16 } => "answered a redirect ({status}), which is not followed",
        /// It answered an index out of the texts' range.
        IndexOut { index: u64, texts: u64 } => "answered the index {index} for {texts} texts",
        /// It answered an index twice.
        IndexTwice { index: u64 } => "answered the index {index} twice",
        /// It answered a vector of another length than the one asked for.
        Dimensions { answered: u64, want: u64 } => "answered a vector of {answered} dimensions, not {want}",
        /// It answered a vector with a value that is not finite.
        NotFinite => "answered a vector with a value that is not finite",
        /// A command did not start.
        DidNotStart { command: String, detail: Detail } => "ran the command `{command}`, which did not start: {detail}",
        /// A command's output could not be read.
        OutputUnread { command: String } => "ran the command `{command}`, whose output could not be read",
        /// A command wrote more than the limit, in bytes.
        OutputTooLarge { command: String, cap: u64 } => "ran the command `{command}`, which wrote more than {cap} bytes",
        /// A command could not be waited for.
        NotWaited { command: String } => "ran the command `{command}`, which could not be waited for",
        /// A command took longer than allowed and was stopped.
        TookTooLong { command: String, secs: u64 } => "ran the command `{command}`, which took longer than {secs}s and was stopped",
        /// A command failed with an exit status, or (none) was stopped by a signal.
        Exited {
            command: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            exit_status: Option<i32>,
        } => "ran the command `{command}`, which [failed with exit status {exit_status}|was stopped by a signal]",
        /// It failed with an error of the contract.
        Failed { cause: Box<Error> } => "failed: {cause}",
    }
}

named! {
    /// What a format version added that an event carries, written in an older format.
    pub enum Added {
        /// It carries `rekeyed`.
        Rekeyed = "rekeyed" => "carries `rekeyed`",
        /// It declares a call of `date_bucket`.
        DateBucket = "date_bucket" => "declares a call of `date_bucket`",
        /// It carries `upstream`.
        Upstream = "upstream" => "carries `upstream`",
        /// It proposes a declaration or a migration.
        ProposesSchema = "proposes_schema" => "proposes a declaration or a migration",
        /// It carries `annotations` in a declaration.
        Annotations = "annotations" => "carries `annotations` in a declaration",
        /// It declares a derived relation.
        DerivedRelation = "derived_relation" => "declares a derived relation",
        /// It declares a trigger's `max_depth`.
        TriggerDepth = "trigger_depth" => "declares a trigger's `max_depth`",
        /// It declares a call of `retired`.
        Retired = "retired" => "declares a call of `retired`",
        /// It declares a view's parameters.
        ViewParams = "view_params" => "declares a view's parameters",
        /// It calls a verb with no key.
        NoKey = "no_key" => "calls a verb with no key",
        /// It declares a verb on its type.
        VerbOnType = "verb_on_type" => "declares a verb on its type",
        /// It declares a view that merges sources.
        MergedSources = "merged_sources" => "declares a view that merges sources",
        /// It proposes a value type's tightening.
        Tightening = "tightening" => "proposes a value type's tightening",
        /// It declares a call of `floor`.
        Floor = "floor" => "declares a call of `floor`",
        /// It declares a call of `date_bucket` by a width.
        DateBucketByWidth = "date_bucket_by_width" => "declares a call of `date_bucket` by a width",
        /// It declares a call of `in_set`.
        InSet = "in_set" => "declares a call of `in_set`",
        /// It renames a relation.
        RelationRename = "relation_rename" => "renames a relation",
        /// It writes a `view_kept.` setting.
        KeptSetting = "kept_setting" => "writes a `view_kept.` setting",
        /// It carries `made`.
        Made = "made" => "carries `made`",
        /// It proposes a migration whose report holds `restated`.
        Restated = "restated" => "proposes a migration whose report holds `restated`",
        /// It declares an `initial` value.
        Initial = "initial" => "declares an `initial` value",
        /// It declares `with_the_set`.
        WithTheSet = "with_the_set" => "declares `with_the_set`",
        /// It declares when a field is computed.
        ComputedAt = "computed_at" => "declares when a field is computed",
        /// It renames a value type's values.
        ValueRenames = "value_renames" => "renames a value type's values",
        /// It proposes a value type's migration that renames values.
        ProposesValueRenames = "proposes_value_renames" => "proposes a value type's migration that renames values",
        /// It proposes a write pinned to its record.
        Pinned = "pinned" => "proposes a write pinned to its record",
    }
}

catalog! {
    /// What is wrong with stored or exported events, a checkpoint, or the files that hold them.
    pub enum Flaw {
        /// A line is not JSON.
        NotJson { detail: Detail } => "a line is not JSON: {detail}",
        /// A line is not an event.
        NotAnEvent { detail: Detail } => "a line is not an event: {detail}",
        /// The bytes are not canonical.
        NotCanonical => "the bytes are not canonical",
        /// A member is not in its one written form.
        NotOneForm => "a member is not in its one written form",
        /// It is written in a format outside the formats read.
        FormatOutside { v: u64, lowest: u64, highest: u64 } => "written in log format {v}, and the formats read are {lowest} to {highest}",
        /// It is written in a format before that of the event before it.
        FormatBack { v: u64, before: u64 } => "written in format {v} after an event in format {before}",
        /// It carries what a later format added.
        FormatBelow { v: u64, carries: Added, need: u64 } => "a format {v} event {carries}, which format {need} added",
        /// `context` carries `cause`, which the envelope carries.
        CauseInContext => "`context` carries `cause`, which the envelope carries",
        /// The envelope carries the origin `initial`, a field's alone.
        InitialOrigin => "the envelope carries the origin `initial`, a field's alone",
        /// An event other than `record_ingested` carries `upstream`.
        UpstreamOn { kind: String } => "a `{kind}` event carries `upstream`, which only a `record_ingested` event carries",
        /// The hash does not match the event.
        HashMismatch => "the hash does not match the event",
        /// The engine's record of the event disagrees with its bytes.
        Disagrees => "the engine's record of the event disagrees with its bytes",
        /// Another position stands where one belongs, or none does.
        Position {
            expected: Position,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            found: Option<Position>,
        } => "[position {found} stands where |expected ]position {expected}[{?found} belongs]",
        /// It does not name the hash of the event before it.
        PrevHash => "it does not name the hash of the event before it",
        /// Its `known_at` is before that of the event before it.
        KnownAtBack { known_at: Timestamp, before: Timestamp } => "its known_at {known_at} is before the {before} of the event before it",
        /// A segment begins there, and its first event is no checkpoint.
        NoCheckpoint => "a segment begins there, and its first event is no checkpoint",
        /// It ends before a position it should hold.
        EndsBefore { position: Position } => "it ends before position {position}",
        /// It ends inside a set of declarations.
        EndsInsideSet => "it ends inside a set of declarations: the set's last declaration is missing",
        /// It holds no event at the position.
        NoEvent { position: Position } => "it holds no event at position {position}",
        /// A migration moves a key the lake does not hold.
        MovesUnheld {
            #[serde(rename = "type")]
            ty: TypeName,
            key: RecordKey,
        } => "a migration moves `{ty}` `{key}`, which the lake does not hold",
        /// A value type's renames reach other types than those that use it.
        RenamesOtherTypes { value_type: String } => "it renames values of `{value_type}` in other types than those that use it",
        /// It records under a seam that is the store's own.
        OwnSeam { seam: String } => "it records under `{seam}`: a seam that begins `sys:` is the store's own",
        /// It declares or writes a type whose name is the store's own.
        OwnType {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "it declares or writes the type `{ty}`: a name that begins `sys:` is the store's own, for what it keeps of a view",
        /// It writes a record of a view.
        ViewRecord {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "it writes a record of `{ty}`, a view, whose records are computed",
        /// A checkpoint does not follow the position before it.
        CheckpointFollows { through: Position } => "the checkpoint says it follows position {through}",
        /// A checkpoint's hash is not that of the event before it.
        CheckpointHash => "the checkpoint's hash is not the hash of the event before it",
        /// A checkpoint's segment list has a range out of place.
        CheckpointRange { first: Position, last: Position, next: u64 } => "the checkpoint's segment list has the range {first}..={last} where one starting at {next} belongs",
        /// A checkpoint's segment list does not end where it does.
        CheckpointEnd { through: Position } => "the checkpoint's segment list does not end at position {through}",
        /// A segment differs from what a checkpoint recorded.
        SegmentDiffers { first: Position, at: Position } => "the segment from position {first} differs from what the checkpoint at {at} recorded",
        /// A segment a checkpoint records is missing.
        SegmentMissing { first: Position, at: Position } => "the segment from position {first} is missing; the checkpoint at {at} records it",
        /// A migration cannot convert a record.
        CannotConvert {
            #[serde(rename = "type")]
            ty: TypeName,
            key: RecordKey,
        } => "a migration cannot convert `{ty}` `{key}`",
        /// The log changes a record before any event wrote it.
        ChangedUnwritten {
            #[serde(rename = "type")]
            ty: TypeName,
            key: RecordKey,
        } => "it changes `{ty}` `{key}` before any event wrote it",
        /// The engine handed records out of key order.
        OutOfKeyOrder {
            #[serde(rename = "type")]
            ty: TypeName,
            key: RecordKey,
            before: RecordKey,
        } => "the engine handed `{ty}` out of key order, `{key}` after `{before}`",
        /// A stored proposal holds no write.
        ProposalWithoutWrite { proposal: ProposalId } => "it holds proposal `{proposal}` without its write",
        /// The projection holds something it cannot read, named as its table names it.
        Unreadable { what: String, detail: Detail } => "it holds {what} it cannot read: {detail}",
        /// A system type has no schema hash.
        NoSchemaHash {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "the system type `{ty}` has no schema hash",
        /// The fold cannot order a type's computed fields.
        CannotOrder {
            #[serde(rename = "type")]
            ty: TypeName,
            cause: Box<Error>,
        } => "the fold cannot order the computed fields of `{ty}`: {cause}",
        /// The fold cannot compute a type's derived fields.
        CannotDerive {
            #[serde(rename = "type")]
            ty: TypeName,
            cause: Box<Error>,
        } => "the fold cannot compute the derived fields of `{ty}`: {cause}",
        /// A line of a kind it does not know.
        UnknownLine => "it holds a line it does not know",
        /// Its last commit line follows no event of its segment.
        CommitWithoutEvent => "its last commit line follows no event of its segment",
        /// A checkpoint is not the first event of a later file.
        CheckpointNotFirst => "a checkpoint is not the first event of a later file",
        /// A checkpoint does not name the position and hash before it.
        CheckpointPrev => "the checkpoint does not name the position and hash before it",
        /// A checkpoint does not list the files before it.
        CheckpointFiles => "the checkpoint does not list the files before it",
        /// A later file does not start with a checkpoint.
        LaterWithoutCheckpoint => "a later file does not start with a checkpoint",
        /// Another file follows than the one that belongs.
        NextFile { name: String, first: u64 } => "the next file is {name}, starting at {first}",
        /// A file is not named by a first position in 20 digits.
        FileName { name: String } => "the file {name} is not named by a first position in 20 digits",
        /// A file does not end in a line feed.
        NoLineFeed { name: String } => "the file {name} does not end in a line feed",
        /// A file holds no event.
        NoEventIn { name: String } => "the file {name} holds no event",
        /// There is no segment file.
        NoSegmentFile => "there is no segment file under log/",
        /// An event carries another fence than its writer's lease had.
        FenceMismatch { carried: u64, lease: u64 } => "an event carries fence {carried}, and its writer's lease had fence {lease}",
        /// The engine and the file digest a segment differently.
        DigestDiffers { engine: String, file: String } => "the engine digests the segment before it as `{engine}`, and the file's is `{file}`",
        /// The engine holds no event of the segment before to seal.
        NothingToSeal => "the engine holds no event of the segment before it to seal",
        /// A file ends before its committed events do.
        Truncated => "it ends before its committed events do",
    }
}

catalog! {
    /// Why the lake cannot answer now. Retrying may help; the sentence says what the operator can do.
    pub enum Situation {
        /// Another writer holds the lake's writer lease.
        LeaseHeld { holder: String, fence: u64 } => "the lake's writer lease is held by `{holder}` (fence {fence}); it must be released before another writer can take it",
        /// The lease's lock is held: by the writer its row names, by a session still ending, or by another schema whose name shares the lock.
        LeaseLocked {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            holder: Option<String>,
            fence: u64,
        } => "the lake's writer lease cannot be taken: its lock is held, and the lease row [names `{holder}` ](fence {fence})[{?holder}| names no holder]. Either that writer holds it, or a writer's session is still ending, or another schema's name hashes to the same lock; retry once the writer is gone, or give this lake another schema",
        /// The caller's lease is stale: another holds the lake's lease now, or no one does.
        LeaseStale {
            fence: u64,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            holder: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            current: Option<u64>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            undone: Option<Act>,
        } => "the lease with fence {fence} is stale: [`{holder}` holds the lake's writer lease[ with fence {current}]|[the lease row records fence {current} and no holder|no one holds the lake's writer lease]][; {undone}]",
        /// The caller's lease is not held by this handle.
        LeaseNotHeld {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            fence: Option<u64>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            undone: Option<Act>,
        } => "the lease[ with fence {fence}] is not held by this handle[; {undone}]",
        /// The lake is open read-only here; whether another process holds its writer lease.
        ReadOnly {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            lake: Option<String>,
            #[serde(default)]
            leased_elsewhere: bool,
        } => "[lake `{lake}`|this lake] is open read-only here[{?leased_elsewhere}; another process holds its writer lease|[{?lake}; open it for writing to take the lease]]",
        /// There is no lake where the location says, in the schema looked in where the engine keeps one per lake.
        NoLake {
            lake: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            schema: Option<String>,
        } => "there is no lake `{lake}` [in the schema `{schema}`|where its location says]; open it with `create` to make it",
        /// This server serves no lake of the name.
        NotServed { lake: String } => "this server serves no lake `{lake}`",
        /// A part every lake of this engine has is missing: something else made what is there.
        Unmade { part: Part } => "this engine did not create what is there: {part} is missing",
        /// The lake has no projection yet.
        NoProjection { lake: String } => "lake `{lake}` has no projection yet; open it for writing first",
        /// The lake could not be compacted while a reader holds a snapshot.
        Busy { lake: String } => "lake `{lake}` could not be compacted: a reader holds a snapshot of its projection; nothing a read answers has changed, so compact again when the reader is done",
        /// The projection is behind the log and could not catch up; nothing was appended.
        Behind {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            cause: Option<Box<Error>>,
        } => "the lake's projection is behind its log and could not catch up; nothing was appended[ ({cause})]",
        /// The write was decided while the projection was behind the log; nothing was appended.
        DecidedBehind => "this write was decided while the lake's projection was behind its log; nothing was appended",
        /// What was appended from a position is in the log and not folded yet.
        NotFolded { from: Position, cause: Box<Error> } => "what was appended is in the log from position {from} and is not folded yet; the next write folds it, and a write is sent again only under its idempotency key ({cause})",
        /// The lake's names moved while the write waited; send it again.
        Moved => "the lake's names moved while this write waited; send it again",
        /// A call of a verb that reaches outside the lake failed before its executor was handed it.
        Executor { retry: Retry, cause: Box<Error> } => "the verb's executor was not handed this call ({cause}); {retry}",
        /// A call's executor answered, recording its answer failed, and the answer is held and recorded before the next write; what sending it again answers, unless it has no key.
        AnswerHeld {
            answered: Value,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            retry: Option<Resent>,
            cause: Box<Error>,
        } => "the verb's executor answered this call, and recording its answer, {answered}, failed: {cause}; the answer is held and recorded before the next write[; {retry}]",
        /// A call's executor answered, and recording its answer failed.
        AnswerLost { answered: Value, cause: Box<Error> } => "the verb's executor answered this call with {answered}, and recording its answer failed: {cause}",
        /// The store's work stopped unexpectedly.
        Stopped { detail: Detail } => "the store's work stopped unexpectedly: {detail}",
        /// Stored or exported events, or the files that hold them, are damaged, at a position if one.
        Damaged {
            part: Part,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            position: Option<Position>,
            #[serde(with = "closed")]
            flaw: Flaw,
        } => "{part} is damaged[ at position {position}]: {flaw}",
        /// The storage failed.
        Storage { doing: Doing, part: Part, detail: Detail } => "the storage failed while {doing} {part}: {detail}",
        /// An append to the log failed; what it left, and why the cut after it failed where it did.
        AppendFailed {
            detail: Detail,
            left: Left,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            cut: Option<Box<Error>>,
        } => "appending to the log failed: {detail}[; {left}][ ({cut})]",
        /// A seal's step was taken and syncing its directory failed: the next segment is in the log's files, and the outcome is not known.
        SealUnsure { detail: Detail } => "syncing the log's directory failed: {detail}; the next segment is in the log's files and its name's sync failed: every handle reads it, and the outcome is not known",
        /// A segment was sealed through a position, and this handle then failed to read on.
        SealedNotRead { last: Position, cause: Box<Error> } => "the segment through position {last} is sealed and the next one is in the log's files, synced; this handle then failed to read on: {cause}",
        /// A compaction failed, and the projection is as it was.
        CompactFailed { lake: String, detail: Detail } => "lake `{lake}` could not be compacted, and its projection is as it was: {detail}",
        /// Every event was imported, and folding the projection failed.
        ImportNotFolded { cause: Box<Error> } => "every event was imported, and folding the projection failed: {cause}; a writer's open folds it again",
        /// An import stopped part way, at the position it reached.
        ImportStopped { reached: Position, lake: String, cause: Box<Error> } => "the import stopped at position {reached}: {cause}; the lake `{lake}` holds the export's events before that position, a true prefix that is not the exported lake, and refuses another import; remove it and import again",
        /// The PostgreSQL server is older than the engine needs.
        ServerTooOld { major: u32, minor: u32 } => "the server is PostgreSQL {major}.{minor}; the engine needs PostgreSQL 16 or newer, and nothing was created",
        /// A transaction-mode connection pooler seems to sit in front of the server.
        Pooler => "the connection does not keep its session; a transaction-mode connection pooler seems to sit in front of the server. The writer lease is a session lock, which such a pooler loses or hands to another client, so the lake was not opened for writing. Connect directly or through a session-mode pooler.",
        /// TLS cannot be set up for the lake's connection, with the library's words if it gave any.
        Tls {
            problem: Tls,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            detail: Option<Detail>,
        } => "TLS cannot be set up: {problem}[: {detail}]",
        /// The lake's names cannot all become tools.
        ToolNames {
            #[serde(with = "closed::list")]
            problems: Vec<NameProblem>,
        } => "the lake's names cannot all become tools[: {problems}]",
        /// An exchange with a remote end failed.
        Remote {
            #[serde(with = "closed")]
            remote: Remote,
            #[serde(with = "closed")]
            exchange: Exchange,
        } => "calling {remote}: {exchange}",
    }
}

#[cfg(test)]
mod tests;
