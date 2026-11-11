//! Reading a contract type from a JSON value, strictly: a struct is read
//! from an object and never from a sequence.
//!
//! Serde reads a struct's members from a sequence by position as well as
//! from an object by name. A positional form names nothing, so a member
//! added or moved changes what it means, silently. [`Reader`] refuses it,
//! and every value it reads — a map's member, a sequence's element, an
//! externally tagged variant's content — is read through it, so the
//! refusal holds however deep the struct sits.
//!
//! Three places serde buffers a value before reading it cannot be seen
//! from here: the subtree of a `flatten` member, the variants of an
//! `untagged` enum, and the content of a tagged enum's variant. A type
//! that reads one of those is read by hand, through [`read`] on the
//! members it splits out, so its own refusal is this one.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, DeserializeOwned, DeserializeSeed, Visitor};
use serde::forward_to_deserialize_any;
use serde_json::{Map, Value};

/// A value that will not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// A sequence where a struct, or a tagged enum, is asked for.
    Positional {
        /// The members the form takes, each in backticks, joined by `, `.
        takes: String,
        /// Where the sequence sat, when a buffered read below the root
        /// said it; `None` where the reader's own path says where.
        at: Option<String>,
    },
    /// A member the form does not take, and the members it takes.
    NoMember {
        /// The member it does not take.
        member: String,
        /// The members it takes.
        takes: &'static [&'static str],
    },
    /// A member the form takes and was not given.
    Missing(&'static str),
    /// A name that is not one of the variants a tagged form takes.
    Variant {
        /// The name it was given.
        name: String,
        /// The names it takes.
        takes: &'static [&'static str],
    },
    /// Anything else, in serde's words.
    Other(String),
}

/// How every positional refusal opens; what follows it is the form's
/// members, each in backticks.
const POSITIONAL: &str =
    "its members are named, not given by position: write it as an object that takes ";

impl Fault {
    /// The refusal of a sequence where `fields` are asked for by name.
    fn positional(fields: &[&str]) -> Fault {
        Fault::Positional {
            takes: listed(fields),
            at: None,
        }
    }
}

/// The refusal of a sequence where an object that names `tag` is asked
/// for: a tagged enum, whose members the tag chooses.
pub fn refused(tag: &str) -> String {
    format!("{POSITIONAL}`{tag}`")
}

/// The refusal of a sequence where an object that names `fields` is
/// asked for: for a type that reads itself by hand, and so meets the
/// sequence in a visitor of its own.
pub fn positional(fields: &[&str]) -> String {
    Fault::positional(fields).to_string()
}

/// Reads a struct's members as they are written, its time axes apart:
/// `wire` reads the members the struct names, and `build` puts them and
/// the axes together.
///
/// The members are taken one at a time, as the text writes them, so a
/// member's refusal keeps the place it was met at; none is buffered
/// first. A sequence where the struct is asked for is refused in this
/// module's words, whether the reader refuses it before the visitor or it
/// is met here.
pub fn struct_read<'de, D, W, T>(
    d: D,
    name: &'static str,
    names: &'static [&'static str],
    build: impl FnOnce(W, crate::read::ReadAt) -> T,
) -> Result<T, D::Error>
where
    D: de::Deserializer<'de>,
    W: DeserializeOwned,
{
    /// The members a struct takes, less its time axes, which are read
    /// apart once the ones beside them are read.
    struct Members<M> {
        map: M,
        axes: Map<String, Value>,
    }

    impl<'de, M: de::MapAccess<'de>> de::MapAccess<'de> for &mut Members<M> {
        type Error = M::Error;

        fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
        where
            K: de::DeserializeSeed<'de>,
        {
            loop {
                let Some(key) = self.map.next_key_seed(PhantomData::<String>)? else {
                    return Ok(None);
                };
                if key == "as_of" || key == "valid_at" {
                    let value = self.map.next_value_seed(PhantomData::<Value>)?;
                    self.axes.insert(key, value);
                    continue;
                }
                return seed
                    .deserialize(de::value::StringDeserializer::<M::Error>::new(key))
                    .map(Some);
            }
        }

        fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
        where
            V: de::DeserializeSeed<'de>,
        {
            self.map.next_value_seed(seed)
        }
    }

    struct Visit<W, T, F> {
        build: F,
        names: &'static [&'static str],
        _taken: PhantomData<fn() -> (W, T)>,
    }

    impl<'de, W: DeserializeOwned, T, F: FnOnce(W, crate::read::ReadAt) -> T> Visitor<'de>
        for Visit<W, T, F>
    {
        type Value = T;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("an object that names the members it takes")
        }

        fn visit_map<M: de::MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            let mut members = Members {
                map,
                axes: Map::new(),
            };
            let wire = serde::Deserialize::deserialize(de::value::MapAccessDeserializer::new(
                &mut members,
            ))?;
            let at = axes(&mut members.axes)?;
            Ok((self.build)(wire, at))
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, _seq: A) -> Result<T, A::Error> {
            Err(de::Error::custom(positional(self.names)))
        }
    }

    d.deserialize_struct(
        name,
        names,
        Visit {
            build,
            names,
            _taken: PhantomData,
        },
    )
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::Positional { takes, at } => match at {
                Some(at) => write!(f, "at `{at}`: {POSITIONAL}{takes}"),
                None => write!(f, "{POSITIONAL}{takes}"),
            },
            Fault::NoMember { member, takes } => {
                write!(f, "unknown field `{member}`, {}", expected(takes))
            }
            Fault::Missing(member) => write!(f, "missing field `{member}`"),
            Fault::Variant { name, takes } => {
                write!(f, "unknown variant `{name}`, {}", expected(takes))
            }
            Fault::Other(said) => f.write_str(said),
        }
    }
}

impl std::error::Error for Fault {}

impl de::Error for Fault {
    /// The structure a member a struct does not take is said with is kept,
    /// so a door that reads again for its words finds it there.
    fn unknown_field(member: &str, takes: &'static [&'static str]) -> Fault {
        Fault::NoMember {
            member: member.to_owned(),
            takes,
        }
    }

    fn missing_field(member: &'static str) -> Fault {
        Fault::Missing(member)
    }

    fn unknown_variant(name: &str, takes: &'static [&'static str]) -> Fault {
        Fault::Variant {
            name: name.to_owned(),
            takes,
        }
    }

    fn custom<T: fmt::Display>(said: T) -> Fault {
        // A nested reader says its refusal through here; the sentence is
        // read back so that a positional refusal stays one, and where a
        // buffered read below the root said the sequence sat.
        let said = said.to_string();
        let (at, rest) = match said.strip_prefix("at `").and_then(|r| r.split_once("`: ")) {
            Some((at, rest)) => (Some(at.to_owned()), rest),
            None => (None, said.as_str()),
        };
        match rest.strip_prefix(POSITIONAL) {
            Some(takes) => Fault::Positional {
                takes: takes.to_owned(),
                at,
            },
            None => Fault::Other(said),
        }
    }
}

/// A JSON value read the strict way.
pub struct Reader(Value);

/// Reads `T` from `value`, saying where a member that does not fit sits.
pub fn read<T: DeserializeOwned>(value: Value) -> Result<T, serde_path_to_error::Error<Fault>> {
    serde_path_to_error::deserialize(Reader(value))
}

/// Reads `T` from `value`; the name an entry point answers to at a door.
pub fn from_value<T: DeserializeOwned>(
    value: Value,
) -> Result<T, serde_path_to_error::Error<Fault>> {
    read(value)
}

/// Reads each of a list's members strictly, in [`read`]'s words.
pub fn read_slice<T: DeserializeOwned>(
    value: Value,
) -> Result<Vec<T>, serde_path_to_error::Error<Fault>> {
    read(value)
}

/// Reads `T` from JSON `text`: a form written by position is refused in
/// [`Fault::Positional`]'s words, and every other misfit is said in serde's
/// own, met in the order the text writes the members.
///
/// The text is read once as a value, which sorts a map's keys; a refusal
/// that is not a sequence's is read again as the text is written, so the
/// file's first misfit is the one named.
pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    from_value::<T>(value).map_err(|refused| match refused.inner() {
        Fault::Positional { .. } => refused.to_string(),
        _ => serde_json::from_str::<T>(text)
            .err()
            .map_or_else(|| refused.to_string(), |said| said.to_string()),
    })
}

/// Reads `T` from `value`, in another reader's words: a member a struct
/// does not take, or one it needs, is said as that reader says it, so a
/// door that reads again for its words sees the same member; anything
/// else is said in this reader's words, where it stopped.
pub fn value<T: DeserializeOwned, E: de::Error>(value: Value) -> Result<T, E> {
    read(value).map_err(|e| {
        let path = e.path().to_string();
        // A member a struct does not take is refused while its name is
        // read, so the path is that member's own name rather than where
        // the struct sits; the reader that asked for the struct says
        // where, as it says it for every member.
        match e.inner() {
            Fault::NoMember { member, takes } => E::unknown_field(member, takes),
            Fault::Missing(member) => E::missing_field(member),
            Fault::Variant { name, takes } => E::unknown_variant(name, takes),
            inner if path == "." => E::custom(inner.to_string()),
            inner => E::custom(format!("at `{path}`: {inner}")),
        }
    })
}

/// Reads `T` from a tagged enum's remaining members, in another reader's
/// words.
pub fn at<T: DeserializeOwned, E: de::Error>(map: Map<String, Value>) -> Result<T, E> {
    value(Value::Object(map))
}

/// A tagged value's content, read as `T`: a kind that takes one and was
/// given none is missing `value`.
pub fn taken<T: DeserializeOwned, E: de::Error>(content: Option<Value>) -> Result<T, E> {
    match content {
        Some(value) => self::value(value),
        None => Err(E::missing_field("value")),
    }
}

/// A tagged value's members: refused when it is a sequence, which names
/// no tag.
pub fn object<E: de::Error>(value: Value, tag: &str) -> Result<Map<String, Value>, E> {
    match value {
        Value::Object(map) => Ok(map),
        Value::Array(_) => Err(E::custom(refused(tag))),
        other => Err(E::custom(format!(
            "an object that takes `{tag}`, not {}",
            kind(&other)
        ))),
    }
}

/// A tagged value's code, taken out of its members.
pub fn code<E: de::Error>(map: &mut Map<String, Value>, tag: &str) -> Result<String, E> {
    match map.remove(tag) {
        Some(Value::String(code)) => Ok(code),
        Some(other) => Err(E::custom(format!(
            "`{tag}` is {}, not a name",
            kind(&other)
        ))),
        None => Err(E::custom(format!("an object that takes `{tag}` names one"))),
    }
}

/// A read's time axes, split out of its members and read as a [`ReadAt`]:
/// the members a flattened struct holds beside the ones around it.
///
/// [`ReadAt`]: crate::read::ReadAt
pub fn axes<E: de::Error>(members: &mut Map<String, Value>) -> Result<crate::read::ReadAt, E> {
    let mut at = Map::new();
    for axis in ["as_of", "valid_at"] {
        if let Some(value) = members.remove(axis) {
            at.insert(axis.to_owned(), value);
        }
    }
    value(Value::Object(at))
}

/// One variant of an untagged enum: `T` read from `value`. When no variant
/// fits, serde's own sentence says so; a positional refusal is kept, since
/// it says which form was written.
pub fn variant<T: DeserializeOwned, E: de::Error>(value: Value, name: &str) -> Result<T, E> {
    read(value).map_err(|e| match e.inner() {
        Fault::Positional { .. } => E::custom(e.inner().to_string()),
        _ => E::custom(format!(
            "data did not match any variant of untagged enum {name}"
        )),
    })
}

/// The members a sentence names, each in backticks, joined by `, `.
fn listed(names: &[&str]) -> String {
    (names.iter())
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What serde says a form takes, after a member or a name it does not:
/// each in backticks, joined by `, `.
fn expected(takes: &[&str]) -> String {
    match takes {
        [] => "there are no fields".into(),
        [one] => format!("expected `{one}`"),
        many => format!("expected one of {}", listed(many)),
    }
}

/// The name of a value's form, for a sentence that says what was found.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a sequence",
        Value::Object(_) => "an object",
    }
}

impl<'de> de::Deserializer<'de> for Reader {
    type Error = Fault;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.0 {
            Value::Null => visitor.visit_unit(),
            Value::Bool(b) => visitor.visit_bool(b),
            Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
                (Some(i), ..) => visitor.visit_i64(i),
                (_, Some(u), _) => visitor.visit_u64(u),
                (.., Some(x)) => visitor.visit_f64(x),
                _ => Err(de::Error::custom("a number out of range")),
            },
            Value::String(s) => visitor.visit_string(s),
            Value::Array(a) => visitor.visit_seq(Items(a.into_iter())),
            Value::Object(o) => visitor.visit_map(Members::new(o)),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.0 {
            Value::Null => visitor.visit_none(),
            other => visitor.visit_some(Reader(other)),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Fault> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Fault> {
        if matches!(self.0, Value::Array(_)) {
            return Err(Fault::positional(fields));
        }
        self.deserialize_any(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Fault> {
        match self.0 {
            Value::String(name) => visitor.visit_enum(Unit(name)),
            Value::Object(o) if o.len() == 1 => {
                let (name, value) = (o.into_iter().next()).expect("one member");
                visitor.visit_enum(Variant(name, Reader(value)))
            }
            other => Err(de::Error::custom(format!(
                "expected a variant's name or an object of one member, found {}",
                kind(&other)
            ))),
        }
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.0 {
            Value::Object(o) => visitor.visit_map(Members::new(o)),
            other => Err(de::Error::custom(format!(
                "expected an object, found {}",
                kind(&other)
            ))),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.0 {
            Value::Array(a) => visitor.visit_seq(Items(a.into_iter())),
            other => Err(de::Error::custom(format!(
                "expected a sequence, found {}",
                kind(&other)
            ))),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.0 {
            Value::Null => visitor.visit_unit(),
            other => Err(de::Error::custom(format!(
                "expected null, found {}",
                kind(&other)
            ))),
        }
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit_struct tuple tuple_struct identifier ignored_any
    }
}

/// A map's members, each read strictly.
struct Members {
    members: std::vec::IntoIter<(String, Value)>,
    value: Option<Value>,
}

impl Members {
    fn new(map: Map<String, Value>) -> Members {
        Members {
            members: map.into_iter().collect::<Vec<_>>().into_iter(),
            value: None,
        }
    }
}

impl<'de> de::MapAccess<'de> for Members {
    type Error = Fault;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Fault> {
        match self.members.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(de::value::StrDeserializer::<Fault>::new(&key))
                    .map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Fault> {
        let value = self
            .value
            .take()
            .ok_or_else(|| de::Error::custom("a value with no key"))?;
        seed.deserialize(Reader(value))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.members.len())
    }
}

/// A sequence's elements, each read strictly.
struct Items(std::vec::IntoIter<Value>);

impl<'de> de::SeqAccess<'de> for Items {
    type Error = Fault;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Fault> {
        match self.0.next() {
            Some(value) => seed.deserialize(Reader(value)).map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

/// A variant named alone: a string's value.
struct Unit(String);

impl<'de> de::EnumAccess<'de> for Unit {
    type Error = Fault;
    type Variant = NoValue;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, NoValue), Fault> {
        let made = seed.deserialize(de::value::StrDeserializer::<Fault>::new(&self.0))?;
        Ok((made, NoValue))
    }
}

/// A variant that takes no content.
struct NoValue;

impl<'de> de::VariantAccess<'de> for NoValue {
    type Error = Fault;

    fn unit_variant(self) -> Result<(), Fault> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, _seed: T) -> Result<T::Value, Fault> {
        Err(de::Error::custom("this variant takes no value"))
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, _visitor: V) -> Result<V::Value, Fault> {
        Err(de::Error::custom("this variant takes no value"))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Fault> {
        Err(de::Error::custom("this variant takes no value"))
    }
}

/// A variant with its content, read strictly.
struct Variant(String, Reader);

impl<'de> de::EnumAccess<'de> for Variant {
    type Error = Fault;
    type Variant = Content;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Content), Fault> {
        let made = seed.deserialize(de::value::StrDeserializer::<Fault>::new(&self.0))?;
        Ok((made, Content(self.1)))
    }
}

/// A variant's content, read strictly.
struct Content(Reader);

impl<'de> de::VariantAccess<'de> for Content {
    type Error = Fault;

    fn unit_variant(self) -> Result<(), Fault> {
        match self.0.0 {
            Value::Null => Ok(()),
            other => Err(de::Error::custom(format!(
                "expected null, found {}",
                kind(&other)
            ))),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Fault> {
        seed.deserialize(self.0)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Fault> {
        de::Deserializer::deserialize_tuple(self.0, len, visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Fault> {
        de::Deserializer::deserialize_struct(self.0, "", fields, visitor)
    }
}

/// Reads a tagged value's members: the tag, then the rest.
pub fn members<'de, E, D>(d: D, tag: &str) -> Result<(String, Map<String, Value>), E>
where
    E: de::Error,
    D: de::Deserializer<'de>,
{
    let value = serde::Deserialize::deserialize(d).map_err(E::custom)?;
    let mut map = object::<E>(value, tag)?;
    let code = code(&mut map, tag)?;
    Ok((code, map))
}

/// Reads a variant's name, serde's own message for one that is not a
/// case of `T`.
pub fn name<T: DeserializeOwned, E: de::Error>(code: &str) -> Result<T, E> {
    T::deserialize(de::value::StrDeserializer::<E>::new(code))
}

/// Declares a tagged enum and reads it strictly: the tag is taken out of
/// the value's members, the variant is chosen by it, and the variant's
/// members are read from the rest of them, so a sequence where the enum
/// is asked for is refused naming its tag.
///
/// `deny` copies `deny_unknown_fields` onto each variant's reader;
/// `permit` leaves its members open.
#[macro_export]
macro_rules! tagged_enum {
    (
        tag = $tag:literal, names = $names:literal, $mode:ident;
        $(#[$outer:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$inner:meta])*
                $variant:ident $( { $($fields:tt)* } )? $( ( $($tuple:tt)* ) )?
            ),* $(,)?
        }
    ) => {
        $(#[$outer])*
        $vis enum $name {
            $(
                $(#[$inner])*
                $variant $( { $($fields)* } )? $( ( $($tuple)* ) )?
            ),*
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D>(d: D) -> ::core::result::Result<$name, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let (code, map) = $crate::strict::members::<D::Error, D>(d, $tag)?;
                #[derive(::serde::Deserialize)]
                #[serde(rename_all = $names)]
                #[allow(dead_code)]
                enum Codes {
                    $(
                        $(#[$inner])*
                        $variant
                    ),*
                }
                let code = $crate::strict::name::<Codes, D::Error>(&code)?;
                match code {
                    $(
                        Codes::$variant => $crate::tagged_variant!(
                            [$mode] $name, $variant $( { $($fields)* } )? $( ( $($tuple)* ) )? , map, D::Error
                        ),
                    )*
                }
            }
        }
    };
}

/// One variant of a [`tagged_enum!`]: its members read from the rest of
/// the tagged value, in a struct that names them.
#[macro_export]
macro_rules! tagged_variant {
    ([deny] $name:ident, $variant:ident { $($(#[$fmeta:meta])* $field:ident: $fty:ty),* $(,)? }, $map:ident, $e:ty) => {{
        #[derive(::serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            $($(#[$fmeta])* $field: $fty),*
        }
        let fields: Fields = $crate::strict::at::<Fields, $e>($map)?;
        Ok($name::$variant { $($field: fields.$field),* })
    }};
    ([permit] $name:ident, $variant:ident { $($(#[$fmeta:meta])* $field:ident: $fty:ty),* $(,)? }, $map:ident, $e:ty) => {{
        #[derive(::serde::Deserialize)]
        struct Fields {
            $($(#[$fmeta])* $field: $fty),*
        }
        let fields: Fields = $crate::strict::at::<Fields, $e>($map)?;
        Ok($name::$variant { $($field: fields.$field),* })
    }};
    ([$mode:ident] $name:ident, $variant:ident ( $ty:ty ), $map:ident, $e:ty) => {{
        let fields: $ty = $crate::strict::at::<$ty, $e>($map)?;
        Ok($name::$variant(fields))
    }};
    ([deny] $name:ident, $variant:ident, $map:ident, $e:ty) => {{
        #[derive(::serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {}
        let _fields: Fields = $crate::strict::at::<Fields, $e>($map)?;
        Ok($name::$variant)
    }};
    ([permit] $name:ident, $variant:ident, $map:ident, $e:ty) => {{
        #[derive(::serde::Deserialize)]
        struct Fields {}
        let _fields: Fields = $crate::strict::at::<Fields, $e>($map)?;
        Ok($name::$variant)
    }};
}
