//! Why a lake cannot answer now, and which seam a call needs: closed sets
//! of cases with their parameters as data. A case's sentence is its
//! `Display` and is built nowhere else; a host renders the case in its
//! users' language, and the sentence is the fallback and what is logged
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

use super::Error;
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
        map.insert("message".into(), Value::String(v.to_string()));
        map.serialize(s)
    }

    /// Read a case; a code this build does not know is the fallback.
    pub fn deserialize<'de, T: Closed, D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
        let Value::Object(mut map) = Value::deserialize(d)? else {
            return Err(serde::de::Error::custom("a case is an object"));
        };
        let message = match map.remove("message") {
            Some(Value::String(m)) => m,
            _ => String::new(),
        };
        let code = match map.get(T::TAG) {
            Some(Value::String(c)) => c.clone(),
            _ => return Err(serde::de::Error::missing_field(T::TAG)),
        };
        if codes::<T>(T::TAG).contains(&code.as_str()) {
            return T::deserialize(Value::Object(map)).map_err(serde::de::Error::custom);
        }
        map.remove(T::TAG);
        let params = map;
        Ok(T::unknown(Unknown {
            code,
            params,
            message,
        }))
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

    fn unknown(unknown: Unknown) -> Self {
        Box::new(T::unknown(unknown))
    }

    fn as_unknown(&self) -> Option<&Unknown> {
        T::as_unknown(self)
    }
}

macro_rules! closed {
    ($ty:ident) => {
        impl Closed for $ty {
            fn unknown(unknown: Unknown) -> Self {
                $ty::Unknown(unknown)
            }

            fn as_unknown(&self) -> Option<&Unknown> {
                match self {
                    $ty::Unknown(u) => Some(u),
                    _ => None,
                }
            }
        }
    };
}

/// A closed set of plain names, written as its name. A name this build
/// does not know is kept, written back as it came, and said as itself in
/// backticks, which no known name's sentence is.
macro_rules! named {
    (
        $(#[$meta:meta])*
        pub enum $set:ident {
            $( $(#[$doc:meta])* $case:ident = $name:literal, )*
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

/// A seam the call needs and the host has not bound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Seam {
    /// A verb's executor, by the name the verb declares.
    Executor {
        /// Its name.
        name: String,
    },
    /// An embedding backend for a field's model: none is bound, or one of
    /// another model or length.
    Embedding {
        /// The model the field is declared with.
        model: String,
        /// Its length.
        dimensions: u32,
        /// The type of the field.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The field.
        field: String,
        /// The backend bound instead, if one is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bound: Option<Bound>,
    },
    /// A function backing of a kind.
    FunctionBacking {
        /// The kind.
        kind: String,
    },
    /// A vector index, which this engine keeps none of.
    VectorIndex,
    /// An engine, whose opener was not passed in.
    Engine {
        /// Its name.
        name: String,
    },
    /// An operation of the host's that this door does not offer.
    Operation {
        /// The operation.
        op: String,
    },
    /// A seam this build does not know.
    #[serde(skip)]
    Unknown(Unknown),
}

closed!(Seam);

impl fmt::Display for Seam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Seam::Executor { name } => write!(f, "the verb executor `{name}`"),
            Seam::Embedding {
                model,
                dimensions,
                ty,
                field,
                bound,
            } => {
                write!(
                    f,
                    "the embedding model `{model}` with {dimensions} dimensions, for `{ty}.{field}`"
                )?;
                match bound {
                    None => write!(f, " (none is bound)"),
                    Some(Bound { model, dimensions }) => {
                        write!(f, " (the bound one is `{model}` with {dimensions})")
                    }
                }
            }
            Seam::FunctionBacking { kind } => write!(
                f,
                "the `{kind}` function backing (the host bound none of that kind)"
            ),
            Seam::VectorIndex => write!(f, "a vector index (this engine keeps none)"),
            Seam::Engine { name } => {
                write!(f, "the engine `{name}` (no opener for it was passed in)")
            }
            Seam::Operation { op } => {
                write!(
                    f,
                    "the host operation `{op}`, which this door does not offer"
                )
            }
            Seam::Unknown(u) => f.write_str(&u.message),
        }
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

named! {
    /// What a write left undone where its lease did not hold.
    pub enum Act {
        /// Nothing was appended.
        Appended = "appended",
        /// Nothing was imported.
        Imported = "imported",
        /// Nothing was sealed.
        Sealed = "sealed",
    }
}

named! {
    /// A part of a lake's storage, or of what is read into one.
    pub enum Part {
        /// The log.
        Log = "log",
        /// An index of the log's segment files.
        LogIndex = "log_index",
        /// A segment of the log.
        Segment = "segment",
        /// A checkpoint in the log.
        Checkpoint = "checkpoint",
        /// The file that records the writer lease.
        LeaseFile = "lease_file",
        /// The projection.
        Projection = "projection",
        /// The place that holds a lake's name.
        Name = "name",
        /// An export's files.
        Export = "export",
        /// A change feed being read.
        Feed = "feed",
        /// A file.
        File = "file",
    }
}

named! {
    /// Why a vector gives a search no direction to rank along.
    pub enum Aimless {
        /// A component is not a finite number.
        NotFinite = "not_finite",
        /// Every component is 0.
        Zero = "zero",
    }
}

impl fmt::Display for Aimless {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Aimless::NotFinite => "has a component that is not a finite number",
            Aimless::Zero => "has no direction: every component is 0",
            Aimless::Unknown(n) => return write!(f, "`{n}`"),
        })
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Part::Log => "the log",
            Part::LogIndex => "the log index",
            Part::Segment => "a segment of the log",
            Part::Checkpoint => "a checkpoint",
            Part::LeaseFile => "the lease file",
            Part::Projection => "the projection",
            Part::Name => "the lake's name",
            Part::Export => "the export",
            Part::Feed => "the feed",
            Part::File => "a file",
            Part::Unknown(n) => return write!(f, "`{n}`"),
        })
    }
}

named! {
    /// What the storage was doing when it failed.
    pub enum Doing {
        /// Running a statement.
        Query = "query",
        /// Creating.
        Create = "create",
        /// Opening.
        Open = "open",
        /// Reading.
        Read = "read",
        /// Writing.
        Write = "write",
        /// Syncing.
        Sync = "sync",
        /// Renaming.
        Rename = "rename",
        /// Truncating.
        Truncate = "truncate",
        /// Locking.
        Lock = "lock",
        /// Unlocking.
        Unlock = "unlock",
        /// Measuring.
        Measure = "measure",
        /// Compacting.
        Compact = "compact",
    }
}

impl fmt::Display for Doing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Doing::Query => "running a statement on",
            Doing::Create => "creating",
            Doing::Open => "opening",
            Doing::Read => "reading",
            Doing::Write => "writing",
            Doing::Sync => "syncing",
            Doing::Rename => "renaming",
            Doing::Truncate => "truncating",
            Doing::Lock => "locking",
            Doing::Unlock => "unlocking",
            Doing::Measure => "measuring",
            Doing::Compact => "compacting",
            Doing::Unknown(n) => return write!(f, "`{n}`"),
        })
    }
}

named! {
    /// What a failed append left in the log's files.
    pub enum Left {
        /// Nothing.
        Nothing = "nothing",
        /// Its events, whose sync failed: every handle reads them, and the
        /// outcome is not known.
        Unsure = "unsure",
        /// What it wrote may lie past the committed end: the next append or
        /// seal cuts it before it writes, or is refused.
        Uncut = "uncut",
    }
}

named! {
    /// How a call of a verb that reaches outside the lake is sent again.
    pub enum Retry {
        /// Call the verb again.
        Call = "call",
        /// Call it again under another key or with none: its key answers
        /// what is recorded.
        OtherKey = "other_key",
        /// Propose the call again: the proposal is decided.
        Propose = "propose",
    }
}

named! {
    /// A TLS setup that failed, on the side of the lake's own connection.
    pub enum Tls {
        /// The certificate authority file cannot be read.
        CaUnreadable = "ca_unreadable",
        /// The certificate authority file holds no certificate.
        CaEmpty = "ca_empty",
        /// No certificate bundle is at a well-known path.
        NoBundle = "no_bundle",
        /// TLS cannot be set up with what was given.
        Setup = "setup",
        /// The client key cannot be read.
        KeyUnreadable = "key_unreadable",
        /// The client certificate file cannot be read.
        CertUnreadable = "cert_unreadable",
        /// The client certificate file holds no certificate.
        CertEmpty = "cert_empty",
        /// The client certificate cannot be used.
        CertUnusable = "cert_unusable",
    }
}

impl fmt::Display for Tls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Tls::CaUnreadable => "the location's `tls.ca_file` cannot be read",
            Tls::CaEmpty => "the location's `tls.ca_file` holds no certificate",
            Tls::NoBundle => {
                "no certificate bundle was found at a well-known path; name one as the \
                 location's `tls.ca_file`, or set `tls.mode` to `disable` for a server \
                 you reach without TLS"
            }
            Tls::Setup => "the library refused the setup",
            Tls::KeyUnreadable => "the location's `tls.client_key` cannot be read",
            Tls::CertUnreadable => "the location's `tls.client_cert` cannot be read",
            Tls::CertEmpty => "the location's `tls.client_cert` holds no certificate",
            Tls::CertUnusable => "the client certificate cannot be used",
            Tls::Unknown(n) => return write!(f, "`{n}`"),
        })
    }
}

/// A remote end a call reaches: an embedding backend, a function's
/// backing, a REST door.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Remote {
    /// The embedding backend of a model.
    Embedding {
        /// The model.
        model: String,
    },
    /// A verb's executor.
    Executor {
        /// Its name.
        name: String,
    },
    /// A function's backing.
    Function {
        /// The function.
        function: String,
    },
    /// A lake's REST door.
    Door {
        /// The lake.
        lake: String,
    },
    /// A remote end this build does not know.
    #[serde(skip)]
    Unknown(Unknown),
}

closed!(Remote);

impl fmt::Display for Remote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Remote::Embedding { model } => write!(f, "the embedding backend `{model}`"),
            Remote::Executor { name } => write!(f, "the verb executor `{name}`"),
            Remote::Function { function } => write!(f, "function `{function}`"),
            Remote::Door { lake } => write!(f, "the REST door of lake `{lake}`"),
            Remote::Unknown(u) => f.write_str(&u.message),
        }
    }
}

/// What went wrong in an exchange with a remote end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Exchange {
    /// The request cannot be made as configured.
    CannotRequest {
        /// The library's words.
        detail: Detail,
    },
    /// The host is not a name TLS can verify.
    NotVerifiable {
        /// The host.
        host: String,
        /// The library's words.
        detail: Detail,
    },
    /// No connection was made.
    CannotConnect {
        /// The host.
        host: String,
        /// The port.
        port: u16,
        /// The library's words.
        detail: Detail,
    },
    /// TLS with the host failed.
    TlsFailed {
        /// The host.
        host: String,
        /// The port.
        port: u16,
        /// The library's words.
        detail: Detail,
    },
    /// TLS cannot be set up: no usable certificate bundle.
    NoBundle,
    /// HTTP failed.
    Http {
        /// The library's words.
        detail: Detail,
    },
    /// The answer's body could not be read.
    Body {
        /// The library's words.
        detail: Detail,
    },
    /// The body was larger than the limit.
    TooLarge {
        /// The limit, in bytes.
        cap: u64,
    },
    /// It answered a status it was not asked for.
    Status {
        /// The status.
        status: u16,
    },
    /// It answered nothing within the time allowed.
    TimedOut {
        /// The seconds allowed.
        secs: u64,
    },
    /// It answered with a body that is not the contract's.
    NotTheContract {
        /// The status of the answer, if the body is its.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
    },
    /// It sent a line or a block that is not the contract's.
    LineNotTheContract,
    /// It answered the feed without the position.
    NoPosition,
    /// It sent nothing, not even a keep-alive.
    Silent {
        /// The seconds it was silent.
        secs: u64,
    },
    /// It took nothing and answered nothing while records were being sent.
    Stalled {
        /// The seconds it stalled.
        secs: u64,
    },
    /// It sent nothing after the records ended.
    SilentAfter {
        /// The seconds it was silent.
        secs: u64,
    },
    /// It ended the stream.
    Ended,
    /// It cut its answer off.
    CutOff {
        /// The library's words.
        detail: Detail,
    },
    /// The request cannot be sent its key.
    KeyNotSent,
    /// The request cannot be built.
    NotSent,
    /// It answered other vectors than texts asked.
    Vectors {
        /// The vectors answered.
        answered: u64,
        /// The texts asked.
        asked: u64,
    },
    /// It answered a vector with no direction to rank along.
    NoDirection {
        /// Why it has none.
        why: Aimless,
    },
    /// It answered what is not JSON.
    NotJson {
        /// The parser's words.
        detail: Detail,
    },
    /// It refused the connection.
    ConnectionRefused,
    /// TLS with it failed.
    TlsBroke {
        /// The library's words, if it gave any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Detail>,
    },
    /// It cannot be reached.
    Unreachable {
        /// The operating system's words, if it gave any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Detail>,
    },
    /// It closed the connection.
    Closed,
    /// It gave no whole answer within the time allowed.
    NoWholeAnswer {
        /// The seconds allowed.
        secs: u64,
    },
    /// It answered a status that is not the contract's.
    UnexpectedStatus {
        /// The status.
        status: u16,
    },
    /// It answered a redirect, which is not followed.
    Redirect {
        /// The status.
        status: u16,
    },
    /// It answered an index out of the texts' range.
    IndexOut {
        /// The index.
        index: u64,
        /// The texts asked.
        texts: u64,
    },
    /// It answered an index twice.
    IndexTwice {
        /// The index.
        index: u64,
    },
    /// It answered a vector of another length.
    Dimensions {
        /// The length answered.
        answered: u64,
        /// The length asked for.
        want: u64,
    },
    /// It answered a vector with a value that is not finite.
    NotFinite,
    /// A command did not start.
    DidNotStart {
        /// The command.
        command: String,
        /// The operating system's words.
        detail: Detail,
    },
    /// A command's output could not be read.
    OutputUnread {
        /// The command.
        command: String,
    },
    /// A command wrote more than the limit.
    OutputTooLarge {
        /// The command.
        command: String,
        /// The limit, in bytes.
        cap: u64,
    },
    /// A command could not be waited for.
    NotWaited {
        /// The command.
        command: String,
    },
    /// A command took longer than allowed and was stopped.
    TookTooLong {
        /// The command.
        command: String,
        /// The seconds allowed.
        secs: u64,
    },
    /// A command failed with an exit status, or was stopped by a signal.
    Exited {
        /// The command.
        command: String,
        /// Its exit status; none where a signal stopped it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_status: Option<i32>,
    },
    /// It failed with an error of the contract.
    Failed {
        /// The error.
        cause: Box<Error>,
    },
    /// An exchange this build does not know.
    #[serde(skip)]
    Unknown(Unknown),
}

closed!(Exchange);

impl fmt::Display for Exchange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Exchange::CannotRequest { detail } => write!(f, "cannot be requested: {detail}"),
            Exchange::NotVerifiable { host, detail } => {
                write!(f, "`{host}` is not a name TLS can verify: {detail}")
            }
            Exchange::CannotConnect { host, port, detail } => {
                write!(f, "cannot connect to {host}:{port}: {detail}")
            }
            Exchange::TlsFailed { host, port, detail } => {
                write!(f, "TLS with {host}:{port} failed: {detail}")
            }
            Exchange::NoBundle => write!(
                f,
                "TLS cannot be set up: no usable certificate bundle; the operator names one as \
                 `ca_file`"
            ),
            Exchange::Http { detail } => write!(f, "HTTP failed: {detail}"),
            Exchange::Body { detail } => write!(f, "its body could not be read: {detail}"),
            Exchange::TooLarge { cap } => write!(
                f,
                "answered more than {cap} bytes at once; raise `max_answer_bytes` if the lake's \
                 records are that large"
            ),
            Exchange::Status { status } => write!(f, "answered {status}"),
            Exchange::TimedOut { secs } => write!(f, "gave no answer within {secs} seconds"),
            Exchange::NotTheContract { status: Some(s) } => {
                write!(f, "answered {s} with a body that is not the contract's")
            }
            Exchange::NotTheContract { status: None } => {
                write!(f, "answered with a body that is not the contract's")
            }
            Exchange::LineNotTheContract => write!(f, "sent a line that is not the contract's"),
            Exchange::NoPosition => write!(f, "answered the feed without its position"),
            Exchange::Silent { secs } => {
                write!(f, "sent nothing for {secs} seconds, not even a keep-alive")
            }
            Exchange::Stalled { secs } => write!(
                f,
                "took nothing and answered nothing for {secs} seconds while records were being sent"
            ),
            Exchange::SilentAfter { secs } => {
                write!(f, "sent nothing for {secs} seconds after the records ended")
            }
            Exchange::Ended => write!(f, "ended its stream"),
            Exchange::CutOff { detail } => write!(f, "cut its answer off: {detail}"),
            Exchange::KeyNotSent => write!(f, "cannot be sent its key"),
            Exchange::NotSent => write!(f, "cannot be sent this request"),
            Exchange::NoDirection { why } => write!(f, "answered a vector that {why}"),
            Exchange::Vectors { answered, asked } => {
                write!(f, "answered {answered} vectors for {asked} texts")
            }
            Exchange::NotJson { detail } => write!(f, "answered what is not JSON: {detail}"),
            Exchange::ConnectionRefused => write!(f, "refused the connection"),
            Exchange::TlsBroke { detail: Some(d) } => write!(f, "failed TLS: {d}"),
            Exchange::TlsBroke { detail: None } => write!(f, "failed TLS"),
            Exchange::Unreachable { detail: Some(d) } => write!(f, "cannot be reached: {d}"),
            Exchange::Unreachable { detail: None } => write!(f, "cannot be reached"),
            Exchange::Closed => write!(f, "closed the connection"),
            Exchange::NoWholeAnswer { secs } => {
                write!(f, "gave no whole answer within {secs} seconds")
            }
            Exchange::UnexpectedStatus { status } => {
                write!(f, "answered {status}, which is not the contract's")
            }
            Exchange::Redirect { status } => {
                write!(f, "answered a redirect ({status}), which is not followed")
            }
            Exchange::IndexOut { index, texts } => {
                write!(f, "answered the index {index} for {texts} texts")
            }
            Exchange::IndexTwice { index } => write!(f, "answered the index {index} twice"),
            Exchange::Dimensions { answered, want } => {
                write!(f, "answered a vector of {answered} dimensions, not {want}")
            }
            Exchange::NotFinite => write!(f, "answered a vector with a value that is not finite"),
            Exchange::DidNotStart { command, detail } => {
                write!(
                    f,
                    "ran the command `{command}`, which did not start: {detail}"
                )
            }
            Exchange::OutputUnread { command } => {
                write!(
                    f,
                    "ran the command `{command}`, whose output could not be read"
                )
            }
            Exchange::OutputTooLarge { command, cap } => {
                write!(
                    f,
                    "ran the command `{command}`, which wrote more than {cap} bytes"
                )
            }
            Exchange::NotWaited { command } => {
                write!(
                    f,
                    "ran the command `{command}`, which could not be waited for"
                )
            }
            Exchange::TookTooLong { command, secs } => write!(
                f,
                "ran the command `{command}`, which took longer than {secs}s and was stopped"
            ),
            Exchange::Exited {
                command,
                exit_status: Some(code),
            } => write!(
                f,
                "ran the command `{command}`, which failed with exit status {code}"
            ),
            Exchange::Exited {
                command,
                exit_status: None,
            } => write!(
                f,
                "ran the command `{command}`, which was stopped by a signal"
            ),
            Exchange::Failed { cause } => write!(f, "failed: {cause}"),
            Exchange::Unknown(u) => f.write_str(&u.message),
        }
    }
}

named! {
    /// What a format version added that an event carries, written in an
    /// older format.
    pub enum Added {
        /// It carries `rekeyed`.
        Rekeyed = "rekeyed",
        /// It declares a call of `date_bucket`.
        DateBucket = "date_bucket",
        /// It carries `upstream`.
        Upstream = "upstream",
        /// It proposes a declaration or a migration.
        ProposesSchema = "proposes_schema",
        /// It carries `annotations` in a declaration.
        Annotations = "annotations",
        /// It declares a derived relation.
        DerivedRelation = "derived_relation",
        /// It declares a trigger's `max_depth`.
        TriggerDepth = "trigger_depth",
        /// It declares a call of `retired`.
        Retired = "retired",
        /// It declares a view's parameters.
        ViewParams = "view_params",
        /// It calls a verb with no key.
        NoKey = "no_key",
        /// It declares a verb on its type.
        VerbOnType = "verb_on_type",
        /// It declares a view that merges sources.
        MergedSources = "merged_sources",
        /// It proposes a value type's tightening.
        Tightening = "tightening",
        /// It declares a call of `floor`.
        Floor = "floor",
        /// It declares a call of `date_bucket` by a width.
        DateBucketByWidth = "date_bucket_by_width",
        /// It declares a call of `in_set`.
        InSet = "in_set",
        /// It renames a relation.
        RelationRename = "relation_rename",
        /// It writes a `view_kept.` setting.
        KeptSetting = "kept_setting",
        /// It carries `made`.
        Made = "made",
        /// It proposes a migration whose report holds `restated`.
        Restated = "restated",
        /// It declares an `initial` value.
        Initial = "initial",
        /// It declares `with_the_set`.
        WithTheSet = "with_the_set",
        /// It declares when a field is computed.
        ComputedAt = "computed_at",
        /// It renames a value type's values.
        ValueRenames = "value_renames",
        /// It proposes a value type's migration that renames values.
        ProposesValueRenames = "proposes_value_renames",
    }
}

impl fmt::Display for Added {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Added::Rekeyed => f.write_str("carries `rekeyed`"),
            Added::DateBucket => f.write_str("declares a call of `date_bucket`"),
            Added::Upstream => f.write_str("carries `upstream`"),
            Added::ProposesSchema => f.write_str("proposes a declaration or a migration"),
            Added::Annotations => f.write_str("carries `annotations` in a declaration"),
            Added::DerivedRelation => f.write_str("declares a derived relation"),
            Added::TriggerDepth => f.write_str("declares a trigger's `max_depth`"),
            Added::Retired => f.write_str("declares a call of `retired`"),
            Added::ViewParams => f.write_str("declares a view's parameters"),
            Added::NoKey => f.write_str("calls a verb with no key"),
            Added::VerbOnType => f.write_str("declares a verb on its type"),
            Added::MergedSources => f.write_str("declares a view that merges sources"),
            Added::Tightening => f.write_str("proposes a value type's tightening"),
            Added::Floor => f.write_str("declares a call of `floor`"),
            Added::DateBucketByWidth => f.write_str("declares a call of `date_bucket` by a width"),
            Added::InSet => f.write_str("declares a call of `in_set`"),
            Added::RelationRename => f.write_str("renames a relation"),
            Added::KeptSetting => f.write_str("writes a `view_kept.` setting"),
            Added::Made => f.write_str("carries `made`"),
            Added::Restated => f.write_str("proposes a migration whose report holds `restated`"),
            Added::Initial => f.write_str("declares an `initial` value"),
            Added::WithTheSet => f.write_str("declares `with_the_set`"),
            Added::ComputedAt => f.write_str("declares when a field is computed"),
            Added::ValueRenames => f.write_str("renames a value type's values"),
            Added::ProposesValueRenames => {
                f.write_str("proposes a value type's migration that renames values")
            }
            Added::Unknown(n) => write!(f, "`{n}`"),
        }
    }
}

/// What is wrong with stored or exported events, a checkpoint, or the
/// files that hold them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Flaw {
    /// A line is not JSON.
    NotJson {
        /// The parser's words.
        detail: Detail,
    },
    /// A line is not an event.
    NotAnEvent {
        /// The parser's words.
        detail: Detail,
    },
    /// The bytes are not canonical.
    NotCanonical,
    /// A member is not in its one written form.
    NotOneForm,
    /// It is written in a format outside the formats read.
    FormatOutside {
        /// Its format.
        v: u64,
        /// The lowest format read.
        lowest: u64,
        /// The highest format read.
        highest: u64,
    },
    /// It is written in a format before that of the event before it.
    FormatBack {
        /// Its format.
        v: u64,
        /// The format of the event before it.
        before: u64,
    },
    /// It carries what a later format added.
    FormatBelow {
        /// Its format.
        v: u64,
        /// What it carries.
        carries: Added,
        /// The format that added it.
        need: u64,
    },
    /// `context` carries `cause`, which the envelope carries.
    CauseInContext,
    /// The envelope carries the origin `initial`, a field's alone.
    InitialOrigin,
    /// An event other than `record_ingested` carries `upstream`.
    UpstreamOn {
        /// The event's kind.
        kind: String,
    },
    /// The hash does not match the event.
    HashMismatch,
    /// The engine's record of the event disagrees with its bytes.
    Disagrees,
    /// Another position stands where one belongs.
    Position {
        /// The position that belongs there.
        expected: Position,
        /// The position found, if one was.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        found: Option<Position>,
    },
    /// It does not name the hash of the event before it.
    PrevHash,
    /// Its `known_at` is before that of the event before it.
    KnownAtBack {
        /// Its `known_at`.
        known_at: Timestamp,
        /// The one before it.
        before: Timestamp,
    },
    /// A segment begins there, and its first event is no checkpoint.
    NoCheckpoint,
    /// It ends before a position it should hold.
    EndsBefore {
        /// The position.
        position: Position,
    },
    /// It ends inside a set of declarations.
    EndsInsideSet,
    /// It holds no event at the position.
    NoEvent {
        /// The position.
        position: Position,
    },
    /// A migration moves a key the lake does not hold.
    MovesUnheld {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The key.
        key: RecordKey,
    },
    /// A value type's renames reach other types than those that use it.
    RenamesOtherTypes {
        /// The value type.
        value_type: String,
    },
    /// It records under a seam that is the store's own.
    OwnSeam {
        /// The seam.
        seam: String,
    },
    /// It declares or writes a type whose name is the store's own.
    OwnType {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
    },
    /// It writes a record of a view.
    ViewRecord {
        /// The view.
        #[serde(rename = "type")]
        ty: TypeName,
    },
    /// A checkpoint does not follow the position before it.
    CheckpointFollows {
        /// The position it says it follows.
        through: Position,
    },
    /// A checkpoint's hash is not that of the event before it.
    CheckpointHash,
    /// A checkpoint's segment list has a range out of place.
    CheckpointRange {
        /// The range's first position.
        first: Position,
        /// Its last.
        last: Position,
        /// Where one belongs.
        next: u64,
    },
    /// A checkpoint's segment list does not end where it does.
    CheckpointEnd {
        /// Where it should end.
        through: Position,
    },
    /// A segment differs from what a checkpoint recorded.
    SegmentDiffers {
        /// The segment's first position.
        first: Position,
        /// The checkpoint's position.
        at: Position,
    },
    /// A segment a checkpoint records is missing.
    SegmentMissing {
        /// The segment's first position.
        first: Position,
        /// The checkpoint's position.
        at: Position,
    },
    /// A migration cannot convert a record.
    CannotConvert {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The key.
        key: RecordKey,
    },
    /// The log changes a record before any event wrote it.
    ChangedUnwritten {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The key.
        key: RecordKey,
    },
    /// The engine handed records out of key order.
    OutOfKeyOrder {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The key handed.
        key: RecordKey,
        /// The key before it.
        before: RecordKey,
    },
    /// A stored proposal holds no write.
    ProposalWithoutWrite {
        /// The proposal.
        proposal: ProposalId,
    },
    /// The projection holds something it cannot read.
    Unreadable {
        /// What it holds, as its table names it.
        what: String,
        /// The parser's words.
        detail: Detail,
    },
    /// A system type has no schema hash.
    NoSchemaHash {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
    },
    /// The fold cannot order a type's computed fields.
    CannotOrder {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// Why.
        cause: Box<Error>,
    },
    /// The fold cannot compute a type's derived fields.
    CannotDerive {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// Why.
        cause: Box<Error>,
    },
    /// A line of a kind it does not know.
    UnknownLine,
    /// Its last commit line follows no event of its segment.
    CommitWithoutEvent,
    /// A checkpoint is not the first event of a later file.
    CheckpointNotFirst,
    /// A checkpoint does not name the position and hash before it.
    CheckpointPrev,
    /// A checkpoint does not list the files before it.
    CheckpointFiles,
    /// A later file does not start with a checkpoint.
    LaterWithoutCheckpoint,
    /// Another file follows than the one that belongs.
    NextFile {
        /// The file's name.
        name: String,
        /// The position it starts at.
        first: u64,
    },
    /// A file is not named by a first position in 20 digits.
    FileName {
        /// The name.
        name: String,
    },
    /// A file does not end in a line feed.
    NoLineFeed {
        /// The name.
        name: String,
    },
    /// A file holds no event.
    NoEventIn {
        /// The name.
        name: String,
    },
    /// There is no segment file.
    NoSegmentFile,
    /// An event carries another fence than its writer's lease had.
    FenceMismatch {
        /// The fence it carries.
        carried: u64,
        /// The lease's.
        lease: u64,
    },
    /// The engine and the file digest a segment differently.
    DigestDiffers {
        /// The engine's digest.
        engine: String,
        /// The file's.
        file: String,
    },
    /// The engine holds no event of the segment before to seal.
    NothingToSeal,
    /// A file ends before its committed events do.
    Truncated,
    /// A flaw this build does not know.
    #[serde(skip)]
    Unknown(Unknown),
}

closed!(Flaw);

impl fmt::Display for Flaw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Flaw::NotJson { detail } => write!(f, "a line is not JSON: {detail}"),
            Flaw::NotAnEvent { detail } => write!(f, "a line is not an event: {detail}"),
            Flaw::NotCanonical => write!(f, "the bytes are not canonical"),
            Flaw::NotOneForm => write!(f, "a member is not in its one written form"),
            Flaw::FormatOutside { v, lowest, highest } => write!(
                f,
                "written in log format {v}, and the formats read are {lowest} to {highest}"
            ),
            Flaw::FormatBack { v, before } => {
                write!(f, "written in format {v} after an event in format {before}")
            }
            Flaw::FormatBelow { v, carries, need } => {
                write!(f, "a format {v} event {carries}, which format {need} added")
            }
            Flaw::CauseInContext => {
                write!(f, "`context` carries `cause`, which the envelope carries")
            }
            Flaw::InitialOrigin => {
                write!(
                    f,
                    "the envelope carries the origin `initial`, a field's alone"
                )
            }
            Flaw::UpstreamOn { kind } => write!(
                f,
                "a `{kind}` event carries `upstream`, which only a `record_ingested` event carries"
            ),
            Flaw::HashMismatch => write!(f, "the hash does not match the event"),
            Flaw::Disagrees => write!(
                f,
                "the engine's record of the event disagrees with its bytes"
            ),
            Flaw::Position {
                expected,
                found: Some(found),
            } => write!(
                f,
                "position {found} stands where position {expected} belongs"
            ),
            Flaw::Position {
                expected,
                found: None,
            } => write!(f, "expected position {expected}"),
            Flaw::PrevHash => write!(f, "it does not name the hash of the event before it"),
            Flaw::KnownAtBack { known_at, before } => write!(
                f,
                "its known_at {known_at} is before the {before} of the event before it"
            ),
            Flaw::NoCheckpoint => write!(
                f,
                "a segment begins there, and its first event is no checkpoint"
            ),
            Flaw::EndsBefore { position } => write!(f, "it ends before position {position}"),
            Flaw::EndsInsideSet => write!(
                f,
                "it ends inside a set of declarations: the set's last declaration is missing"
            ),
            Flaw::NoEvent { position } => write!(f, "it holds no event at position {position}"),
            Flaw::MovesUnheld { ty, key } => write!(
                f,
                "a migration moves `{ty}` `{key}`, which the lake does not hold"
            ),
            Flaw::RenamesOtherTypes { value_type } => write!(
                f,
                "it renames values of `{value_type}` in other types than those that use it"
            ),
            Flaw::OwnSeam { seam } => write!(
                f,
                "it records under `{seam}`: a seam that begins `sys:` is the store's own"
            ),
            Flaw::OwnType { ty } => write!(
                f,
                "it declares or writes the type `{ty}`: a name that begins `sys:` is the store's \
                 own, for what it keeps of a view"
            ),
            Flaw::ViewRecord { ty } => {
                write!(
                    f,
                    "it writes a record of `{ty}`, a view, whose records are computed"
                )
            }
            Flaw::CheckpointFollows { through } => {
                write!(f, "the checkpoint says it follows position {through}")
            }
            Flaw::CheckpointHash => write!(
                f,
                "the checkpoint's hash is not the hash of the event before it"
            ),
            Flaw::CheckpointRange { first, last, next } => write!(
                f,
                "the checkpoint's segment list has the range {first}..={last} where one starting \
                 at {next} belongs"
            ),
            Flaw::CheckpointEnd { through } => write!(
                f,
                "the checkpoint's segment list does not end at position {through}"
            ),
            Flaw::SegmentDiffers { first, at } => write!(
                f,
                "the segment from position {first} differs from what the checkpoint at {at} \
                 recorded"
            ),
            Flaw::SegmentMissing { first, at } => write!(
                f,
                "the segment from position {first} is missing; the checkpoint at {at} records it"
            ),
            Flaw::CannotConvert { ty, key } => {
                write!(f, "a migration cannot convert `{ty}` `{key}`")
            }
            Flaw::ChangedUnwritten { ty, key } => {
                write!(f, "it changes `{ty}` `{key}` before any event wrote it")
            }
            Flaw::OutOfKeyOrder { ty, key, before } => write!(
                f,
                "the engine handed `{ty}` out of key order, `{key}` after `{before}`"
            ),
            Flaw::ProposalWithoutWrite { proposal } => {
                write!(f, "it holds proposal `{proposal}` without its write")
            }
            Flaw::Unreadable { what, detail } => {
                write!(f, "it holds {what} it cannot read: {detail}")
            }
            Flaw::NoSchemaHash { ty } => write!(f, "the system type `{ty}` has no schema hash"),
            Flaw::CannotOrder { ty, cause } => write!(
                f,
                "the fold cannot order the computed fields of `{ty}`: {cause}"
            ),
            Flaw::CannotDerive { ty, cause } => write!(
                f,
                "the fold cannot compute the derived fields of `{ty}`: {cause}"
            ),
            Flaw::UnknownLine => write!(f, "it holds a line it does not know"),
            Flaw::CommitWithoutEvent => {
                write!(f, "its last commit line follows no event of its segment")
            }
            Flaw::CheckpointNotFirst => {
                write!(f, "a checkpoint is not the first event of a later file")
            }
            Flaw::CheckpointPrev => write!(
                f,
                "the checkpoint does not name the position and hash before it"
            ),
            Flaw::CheckpointFiles => write!(f, "the checkpoint does not list the files before it"),
            Flaw::LaterWithoutCheckpoint => {
                write!(f, "a later file does not start with a checkpoint")
            }
            Flaw::NextFile { name, first } => {
                write!(f, "the next file is {name}, starting at {first}")
            }
            Flaw::FileName { name } => {
                write!(f, "{name} is not named by a first position in 20 digits")
            }
            Flaw::NoLineFeed { name } => write!(f, "{name} does not end in a line feed"),
            Flaw::NoEventIn { name } => write!(f, "{name} holds no event"),
            Flaw::NoSegmentFile => write!(f, "there is no segment file under log/"),
            Flaw::FenceMismatch { carried, lease } => write!(
                f,
                "an event carries fence {carried}, and its writer's lease had fence {lease}"
            ),
            Flaw::DigestDiffers { engine, file } => write!(
                f,
                "the engine digests the segment before it as `{engine}`, and the file's is \
                 `{file}`"
            ),
            Flaw::NothingToSeal => {
                write!(
                    f,
                    "the engine holds no event of the segment before it to seal"
                )
            }
            Flaw::Truncated => write!(f, "it ends before its committed events do"),
            Flaw::Unknown(u) => f.write_str(&u.message),
        }
    }
}

/// Why the lake cannot answer now. Retrying may help; the sentence says
/// what the operator can do.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Situation {
    /// Another writer holds the lake's writer lease.
    LeaseHeld {
        /// Who holds it.
        holder: String,
        /// Its fence.
        fence: u64,
    },
    /// The lease's lock is held: by the writer its row names, by a session
    /// still ending, or by another schema whose name shares the lock.
    LeaseLocked {
        /// The holder the lease row names, if it names one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        holder: Option<String>,
        /// The fence the row records.
        fence: u64,
    },
    /// The caller's lease is stale: another holds the lake's lease now, or
    /// no one does.
    LeaseStale {
        /// The stale lease's fence.
        fence: u64,
        /// Who holds the lease now, if anyone is named.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        holder: Option<String>,
        /// The fence recorded now, if one is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<u64>,
        /// What was not done.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undone: Option<Act>,
    },
    /// The caller's lease is not held by this handle.
    LeaseNotHeld {
        /// The lease's fence, if the caller named one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fence: Option<u64>,
        /// What was not done.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undone: Option<Act>,
    },
    /// The lake is open read-only here.
    ReadOnly {
        /// The lake, if the engine names it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lake: Option<String>,
        /// Whether another process holds its writer lease.
        #[serde(default)]
        leased_elsewhere: bool,
    },
    /// There is no lake where the location says.
    NoLake {
        /// The lake.
        lake: String,
        /// The schema looked in, where the engine keeps one per lake.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<String>,
    },
    /// This server serves no lake of the name.
    NotServed {
        /// The lake.
        lake: String,
    },
    /// A part every lake of this engine has is missing: something else
    /// made what is there.
    Unmade {
        /// The part.
        part: Part,
    },
    /// The lake has no projection yet.
    NoProjection {
        /// The lake.
        lake: String,
    },
    /// The lake could not be compacted while a reader holds a snapshot.
    Busy {
        /// The lake.
        lake: String,
    },
    /// The projection is behind the log and could not catch up; nothing
    /// was appended.
    Behind {
        /// Why it could not.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<Box<Error>>,
    },
    /// The write was decided while the projection was behind the log;
    /// nothing was appended.
    DecidedBehind,
    /// What was appended is in the log and not folded yet.
    NotFolded {
        /// The first position appended.
        from: Position,
        /// Why the fold failed.
        cause: Box<Error>,
    },
    /// The lake's names moved while the write waited; send it again.
    Moved,
    /// A call of a verb that reaches outside the lake failed around its
    /// executor's hand-out.
    Executor {
        /// What the executor answered, where it was handed the call and
        /// recording its answer failed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answered: Option<Value>,
        /// Whether its answer is held and recorded before the next write.
        #[serde(default)]
        held: bool,
        /// How the call is sent again.
        retry: Retry,
        /// What failed.
        cause: Box<Error>,
    },
    /// The store's work stopped unexpectedly.
    Stopped {
        /// The runtime's words.
        detail: Detail,
    },
    /// Stored or exported events, or the files that hold them, are
    /// damaged.
    Damaged {
        /// What is damaged.
        part: Part,
        /// Where, if at a position.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<Position>,
        /// What is wrong.
        #[serde(with = "closed")]
        flaw: Flaw,
    },
    /// The storage failed.
    Storage {
        /// What it was doing.
        doing: Doing,
        /// What it was doing it to.
        part: Part,
        /// The storage's words.
        detail: Detail,
    },
    /// An append to the log failed.
    AppendFailed {
        /// The storage's words.
        detail: Detail,
        /// What it left.
        left: Left,
        /// Why the cut after it failed, where it did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cut: Option<Box<Error>>,
    },
    /// A seal's step was taken and syncing its directory failed: the next
    /// segment is in the log's files, and the outcome is not known.
    SealUnsure {
        /// The storage's words.
        detail: Detail,
    },
    /// A segment was sealed, and this handle then failed to read on.
    SealedNotRead {
        /// The sealed segment's last position.
        last: Position,
        /// Why.
        cause: Box<Error>,
    },
    /// A compaction failed, and the projection is as it was.
    CompactFailed {
        /// The lake.
        lake: String,
        /// The storage's words.
        detail: Detail,
    },
    /// Every event was imported, and folding the projection failed.
    ImportNotFolded {
        /// Why.
        cause: Box<Error>,
    },
    /// An import stopped part way.
    ImportStopped {
        /// The position it reached.
        reached: Position,
        /// The lake.
        lake: String,
        /// Why.
        cause: Box<Error>,
    },
    /// The PostgreSQL server is older than the engine needs.
    ServerTooOld {
        /// Its major version.
        major: u32,
        /// Its minor version.
        minor: u32,
    },
    /// A transaction-mode connection pooler seems to sit in front of the
    /// server.
    Pooler,
    /// TLS cannot be set up for the lake's connection.
    Tls {
        /// What failed.
        problem: Tls,
        /// The library's words, if it gave any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<Detail>,
    },
    /// The lake's names cannot all become tools.
    ToolNames {
        /// The problems.
        #[serde(with = "closed::list")]
        problems: Vec<NameProblem>,
    },
    /// An exchange with a remote end failed.
    Remote {
        /// The remote end.
        #[serde(with = "closed")]
        remote: Remote,
        /// What failed.
        #[serde(with = "closed")]
        exchange: Exchange,
    },
    /// A situation this build does not know.
    #[serde(skip)]
    Unknown(Unknown),
}

closed!(Situation);

impl fmt::Display for Situation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let undone = |f: &mut fmt::Formatter<'_>, undone: &Option<Act>| match undone {
            Some(Act::Appended) => write!(f, "; nothing was appended"),
            Some(Act::Imported) => write!(f, "; nothing was imported"),
            Some(Act::Sealed) => write!(f, "; nothing was sealed"),
            Some(Act::Unknown(n)) => write!(f, "; `{n}`"),
            None => Ok(()),
        };
        match self {
            Situation::LeaseHeld { holder, fence } => write!(
                f,
                "the lake's writer lease is held by `{holder}` (fence {fence}); it must be \
                 released before another writer can take it"
            ),
            Situation::LeaseLocked { holder, fence } => {
                write!(
                    f,
                    "the lake's writer lease cannot be taken: its lock is held, and "
                )?;
                match holder {
                    Some(h) => write!(f, "the lease row names `{h}` (fence {fence})")?,
                    None => write!(f, "the lease row (fence {fence}) names no holder")?,
                }
                write!(
                    f,
                    ". Either that writer holds it, or a writer's session is still ending, or \
                     another schema's name hashes to the same lock; retry once the writer is \
                     gone, or give this lake another schema"
                )
            }
            Situation::LeaseStale {
                fence,
                holder,
                current,
                undone: u,
            } => {
                write!(f, "the lease with fence {fence} is stale: ")?;
                match (holder, current) {
                    (Some(h), Some(c)) => {
                        write!(f, "`{h}` holds the lake's writer lease with fence {c}")?
                    }
                    (Some(h), None) => write!(f, "`{h}` holds the lake's writer lease")?,
                    (None, Some(c)) => write!(f, "the lease row records fence {c} and no holder")?,
                    (None, None) => write!(f, "no one holds the lake's writer lease")?,
                }
                undone(f, u)
            }
            Situation::LeaseNotHeld { fence, undone: u } => {
                match fence {
                    Some(fence) => write!(f, "the lease with fence {fence}")?,
                    None => write!(f, "the lease")?,
                }
                write!(f, " is not held by this handle")?;
                undone(f, u)
            }
            Situation::ReadOnly {
                lake,
                leased_elsewhere,
            } => {
                match lake {
                    Some(l) => write!(f, "lake `{l}` is open read-only here")?,
                    None => write!(f, "this lake is open read-only here")?,
                }
                if *leased_elsewhere {
                    write!(f, "; another process holds its writer lease")
                } else if lake.is_some() {
                    write!(f, "; open it for writing to take the lease")
                } else {
                    Ok(())
                }
            }
            Situation::NoLake {
                lake,
                schema: Some(s),
            } => write!(
                f,
                "there is no lake `{lake}` in the schema `{s}`; open it with `create` to make it"
            ),
            Situation::NoLake { lake, schema: None } => write!(
                f,
                "there is no lake `{lake}` where its location says; open it with `create` to \
                 make it"
            ),
            Situation::NotServed { lake } => write!(f, "this server serves no lake `{lake}`"),
            Situation::Unmade { part } => {
                write!(f, "{part} is missing; it was not created by this engine")
            }
            Situation::NoProjection { lake } => write!(
                f,
                "lake `{lake}` has no projection yet; open it for writing first"
            ),
            Situation::Busy { lake } => write!(
                f,
                "lake `{lake}` could not be compacted: a reader holds a snapshot of its \
                 projection; nothing a read answers has changed, so compact again when the \
                 reader is done"
            ),
            Situation::Behind { cause } => {
                write!(
                    f,
                    "the lake's projection is behind its log and could not catch up; nothing was \
                     appended"
                )?;
                match cause {
                    Some(c) => write!(f, " ({})", c.said()),
                    None => Ok(()),
                }
            }
            Situation::DecidedBehind => write!(
                f,
                "this write was decided while the lake's projection was behind its log; nothing \
                 was appended"
            ),
            Situation::NotFolded { from, cause } => write!(
                f,
                "what was appended is in the log from position {from} and is not folded yet; the \
                 next write folds it, and a write is sent again only under its idempotency key \
                 ({})",
                cause.said()
            ),
            Situation::Moved => write!(
                f,
                "the lake's names moved while this write waited; send it again"
            ),
            Situation::Executor {
                answered,
                held,
                retry,
                cause,
            } => {
                match answered {
                    Some(a) => write!(
                        f,
                        "the verb's executor answered this call with {a}, and recording its \
                         answer failed: {}",
                        cause.said()
                    )?,
                    None => write!(
                        f,
                        "{}; the verb's executor was not handed this call",
                        cause.said()
                    )?,
                }
                if answered.is_some() {
                    if *held {
                        write!(f, "; the answer is held and recorded before the next write")?;
                        match retry {
                            Retry::OtherKey => {
                                write!(f, "; sent again under its key, the call answers it")?
                            }
                            Retry::Propose => {
                                write!(f, "; accepted again, the proposal answers it")?
                            }
                            Retry::Unknown(n) => write!(f, "; `{n}`")?,
                            Retry::Call => {}
                        }
                    }
                    return Ok(());
                }
                match retry {
                    Retry::Call => write!(f, ": call the verb again"),
                    Retry::Unknown(n) => write!(f, ": `{n}`"),
                    Retry::OtherKey => write!(
                        f,
                        ", and sending it again under its key will not run it: call the verb \
                         again under another key or with none"
                    ),
                    Retry::Propose => write!(
                        f,
                        ", and the proposal is decided, so accepting it again will not run it: \
                         propose the call again"
                    ),
                }
            }
            Situation::Stopped { detail } => {
                write!(f, "the store's work stopped unexpectedly: {detail}")
            }
            Situation::Damaged {
                part,
                position,
                flaw,
            } => {
                write!(f, "{part} is damaged")?;
                if let Some(p) = position {
                    write!(f, " at position {p}")?;
                }
                write!(f, ": {flaw}")
            }
            Situation::Storage {
                doing,
                part,
                detail,
            } => write!(f, "{doing} {part} failed: {detail}"),
            Situation::AppendFailed { detail, left, cut } => {
                write!(f, "appending to the log failed: {detail}")?;
                match (left, cut) {
                    (Left::Unsure, _) => write!(
                        f,
                        "; its events are in the log's files and their sync failed: every \
                         handle reads them, and the outcome is not known"
                    ),
                    (Left::Uncut, Some(c)) => write!(
                        f,
                        "; what it wrote may still lie past the committed end, and the next \
                         append or seal cuts it before it writes, or is refused ({})",
                        c.said()
                    ),
                    (Left::Uncut, None) => write!(
                        f,
                        "; what it wrote may still lie past the committed end, and the next \
                         append or seal cuts it before it writes, or is refused"
                    ),
                    (Left::Unknown(n), _) => write!(f, "; `{n}`"),
                    (Left::Nothing, _) => Ok(()),
                }
            }
            Situation::SealUnsure { detail } => write!(
                f,
                "syncing the log's directory failed: {detail}; the next segment is in the log's \
                 files and its name's sync failed: every handle reads it, and the outcome is not \
                 known"
            ),
            Situation::SealedNotRead { last, cause } => write!(
                f,
                "the segment through position {last} is sealed and the next one is in the log's \
                 files, synced; this handle then failed to read on: {}",
                cause.said()
            ),
            Situation::CompactFailed { lake, detail } => write!(
                f,
                "lake `{lake}` could not be compacted, and its projection is as it was: {detail}"
            ),
            Situation::ImportNotFolded { cause } => write!(
                f,
                "every event was imported, and folding the projection failed: {}; a writer's \
                 open folds it again",
                cause.said()
            ),
            Situation::ImportStopped {
                reached,
                lake,
                cause,
            } => write!(
                f,
                "the import stopped at position {reached}: {}; the lake `{lake}` holds the \
                 export's events before that position, a true prefix that is not the exported \
                 lake, and refuses another import; remove it and import again",
                cause.said()
            ),
            Situation::ServerTooOld { major, minor } => write!(
                f,
                "the server is PostgreSQL {major}.{minor}; the engine needs PostgreSQL 16 or \
                 newer, and nothing was created"
            ),
            Situation::Pooler => write!(
                f,
                "the connection does not keep its session; a transaction-mode connection pooler \
                 seems to sit in front of the server. The writer lease is a session lock, which \
                 such a pooler loses or hands to another client, so the lake was not opened for \
                 writing. Connect directly or through a session-mode pooler."
            ),
            Situation::Tls { problem, detail } => {
                write!(f, "TLS cannot be set up: {problem}")?;
                match detail {
                    Some(d) => write!(f, ": {d}"),
                    None => Ok(()),
                }
            }
            Situation::ToolNames { problems } => {
                write!(f, "the lake's names cannot all become tools")?;
                for (i, p) in problems.iter().enumerate() {
                    let sep = if i == 0 { ": " } else { "; " };
                    write!(f, "{sep}{p}")?;
                }
                Ok(())
            }
            Situation::Remote { remote, exchange } => write!(f, "{remote}: {exchange}"),
            Situation::Unknown(u) => f.write_str(&u.message),
        }
    }
}

#[cfg(test)]
mod tests;
