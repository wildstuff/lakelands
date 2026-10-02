use super::*;

#[test]
fn a_field_grant_beats_the_type_default_which_beats_the_lake_default() {
    let invoice = TypeName::from("invoice");
    let mut g = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    g.types.insert(
        invoice.clone(),
        TypeGrants {
            default: Some(Access::Masked),
            fields: [(FieldName::from("verdict"), Access::Write)].into(),
        },
    );
    assert_eq!(g.access(&invoice, &"verdict".into()), Access::Write);
    assert_eq!(g.access(&invoice, &"amount".into()), Access::Masked);
    assert_eq!(g.access(&"partner".into(), &"name".into()), Access::Read);
}

#[test]
fn reading_everything_needs_every_grant_to_read_and_restricted() {
    assert!(Grants::owner().reads_everything());
    let reader = Grants {
        default: Access::Read,
        restricted: true,
        ..Grants::default()
    };
    assert!(
        reader.reads_everything(),
        "read is enough; write is not needed"
    );
    let narrowed = |default: Option<Access>, field: Option<Access>| {
        let mut g = reader.clone();
        g.types.insert(
            "invoice".into(),
            TypeGrants {
                default,
                fields: field.map(|a| ("amount".into(), a)).into_iter().collect(),
            },
        );
        g
    };
    assert!(narrowed(Some(Access::Write), Some(Access::Read)).reads_everything());
    assert!(!narrowed(Some(Access::Masked), None).reads_everything());
    assert!(!narrowed(None, Some(Access::Masked)).reads_everything());
    assert!(!narrowed(None, Some(Access::None)).reads_everything());
    assert!(
        !Grants {
            restricted: false,
            ..reader.clone()
        }
        .reads_everything(),
        "withheld records are part of the log"
    );
    assert!(
        !Grants {
            default: Access::Masked,
            ..reader
        }
        .reads_everything()
    );
    assert!(!Grants::default().reads_everything());
}

#[test]
fn nothing_is_granted_by_default() {
    let g = Grants::default();
    assert_eq!(g.access(&"invoice".into(), &"amount".into()), Access::None);
    assert!(!g.review && !g.schema && !g.ingest && !g.restricted);
}
