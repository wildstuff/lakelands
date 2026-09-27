//! Link records, as the mock keeps them: a relation declared without
//! `via` is carried by records of its link type `sys.link.<type>.<relation>`,
//! keyed by the pair (`docs/decisions/035-a-link-is-a-record.md`). The mock
//! derives the link type itself, from the page, as it keeps its own key
//! rule: it shares no code with the store.

use lakelands_api::id::{RecordKey, RelationName, TypeName};
use lakelands_api::ontology::{FieldDecl, FieldType, RelationDecl, StorageKind, TypeDecl};

pub(crate) const FROM: &str = "from";
pub(crate) const TO: &str = "to";

pub(crate) fn name(t: &TypeName, r: &RelationName) -> TypeName {
    TypeName::from(format!("sys.link.{t}.{r}"))
}

/// The type and relation a link type's name spells.
pub(crate) fn parts(t: &TypeName) -> Option<(TypeName, RelationName)> {
    let (ty, r) = t.as_str().strip_prefix("sys.link.")?.rsplit_once('.')?;
    Some((TypeName::from(ty), RelationName::from(r)))
}

pub(crate) fn is_link_type(t: &TypeName) -> bool {
    t.as_str().starts_with("sys.link.")
}

fn end(field: &str, target: &TypeName, r: &RelationDecl) -> FieldDecl {
    FieldDecl {
        name: field.into(),
        ty: FieldType {
            target: Some(target.clone()),
            ..FieldType::kind(StorageKind::Reference)
        },
        required: true,
        authority: r.authority,
        derived: None,
        function: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    }
}

/// The link type of relation `r` of `t`.
pub(crate) fn link_type(t: &TypeName, r: &RelationDecl) -> TypeDecl {
    let mut fields = vec![end(FROM, t, r), end(TO, &r.target, r)];
    fields.extend(r.fields.iter().cloned());
    TypeDecl {
        name: name(t, &r.name),
        key: vec![FROM.into(), TO.into()],
        fields,
        relations: vec![],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: Some(format!(
            "The links of relation `{}` of `{t}` to `{}`.",
            r.name, r.target
        )),
        label: None,
    }
}

/// The link types a type's relations call for.
pub(crate) fn of(t: &TypeDecl) -> Vec<TypeDecl> {
    t.relations
        .iter()
        .filter(|r| r.via.is_none())
        .map(|r| link_type(&t.name, r))
        .collect()
}

/// The key of the link from `from` to `to`, by the key rule.
pub(crate) fn key(from: &RecordKey, to: &RecordKey) -> RecordKey {
    let c = |k: &RecordKey| k.as_str().replace('%', "%25").replace(':', "%3A");
    RecordKey::from(format!("{}:{}", c(from), c(to)))
}
