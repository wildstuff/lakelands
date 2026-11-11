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
        let takes = (fields.iter())
            .map(|field| format!("`{field}`"))
            .collect::<Vec<_>>()
            .join(", ");
        Fault::Positional { takes }
    }
}

/// The refusal of a sequence where an object that names `tag` is asked
/// for: a tagged enum, whose members the tag chooses.
pub fn refused(tag: &str) -> String {
    format!("{POSITIONAL}`{tag}`")
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fault::Positional { takes } => write!(f, "{POSITIONAL}{takes}"),
            Fault::Other(said) => f.write_str(said),
        }
    }
}

impl std::error::Error for Fault {}

impl de::Error for Fault {
    fn custom<T: fmt::Display>(said: T) -> Fault {
        // A nested reader says its refusal through here; the sentence is
        // read back so that a positional refusal stays one.
        let said = said.to_string();
        match said.strip_prefix(POSITIONAL) {
            Some(takes) => Fault::Positional {
                takes: takes.to_string(),
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

/// Reads a list of `T` from `value`, each element strictly.
pub fn read_slice<T: DeserializeOwned>(
    value: Value,
) -> Result<Vec<T>, serde_path_to_error::Error<Fault>> {
    read(value)
}

/// Reads `T` from `value`, in another reader's words.
pub fn value<T: DeserializeOwned, E: de::Error>(value: Value) -> Result<T, E> {
    read(value).map_err(|e| match e.path().to_string().as_str() {
        "." => E::custom(e.inner().to_string()),
        path => E::custom(format!("at `{path}`: {}", e.inner())),
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
        None => Err(E::custom(format!(
            "an object that takes `{tag}` names one"
        ))),
    }
}

/// A struct's members, taken from `d`: an object's, or the refusal of a
/// sequence, naming the members it would have given by position.
pub fn fields<'de, E, D>(d: D, names: &[&str]) -> Result<Map<String, Value>, E>
where
    E: de::Error,
    D: de::Deserializer<'de>,
{
    match serde::Deserialize::deserialize(d).map_err(E::custom)? {
        Value::Object(map) => Ok(map),
        Value::Array(_) => Err(E::custom(Fault::positional(names))),
        other => Err(E::custom(format!(
            "an object that takes {}, not {}",
            listed(names),
            kind(&other)
        ))),
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
        Fault::Other(_) => E::custom(format!(
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
                seed.deserialize(de::value::StrDeserializer::<Fault>::new(&key)).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Fault> {
        let value = self.value.take().ok_or_else(|| de::Error::custom("a value with no key"))?;
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

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, NoValue), Fault> {
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

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, Fault> {
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

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Content), Fault> {
        let made = seed.deserialize(de::value::StrDeserializer::<Fault>::new(&self.0))?;
        Ok((made, Content(self.1)))
    }
}

/// A variant's content, read strictly.
struct Content(Reader);

impl<'de> de::VariantAccess<'de> for Content {
    type Error = Fault;

    fn unit_variant(self) -> Result<(), Fault> {
        match self.0 .0 {
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

    fn tuple_variant<V: Visitor<'de>>(
        self,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, Fault> {
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
    ([deny] $name:ident, $variant:ident ( $ty:ty ), $map:ident, $e:ty) => {{
        let fields: $ty = $crate::strict::at::<$ty, $e>($map)?;
        Ok($name::$variant(fields))
    }};
    ([permit] $name:ident, $variant:ident ( $ty:ty ), $map:ident, $e:ty) => {{
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
