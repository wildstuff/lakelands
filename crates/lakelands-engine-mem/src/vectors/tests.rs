use super::*;

fn index() -> Vectors {
    let mut v = Vectors::default();
    let ty = TypeName::from("doc");
    v.put(&ty, &"east".into(), "emb", &[1.0, 0.0]);
    v.put(&ty, &"north".into(), "emb", &[0.0, 1.0]);
    v.put(&ty, &"north-east".into(), "emb", &[1.0, 1.0]);
    v.put(&ty, &"also-east".into(), "emb", &[3.0, 0.0]);
    v
}

#[test]
fn the_nearest_come_best_first_with_ties_by_key() {
    let hits = index()
        .nearest(&"doc".into(), "emb", &[2.0, 0.0], 3)
        .unwrap();
    let keys: Vec<&str> = hits.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["also-east", "east", "north-east"]);
    assert!((hits[0].1 - 1.0).abs() < 1e-12);
    assert!((hits[2].1 - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
}

#[test]
fn put_replaces_remove_forgets_and_count_counts() {
    let mut v = index();
    let ty = TypeName::from("doc");
    assert_eq!(v.count(&ty, "emb"), 4);
    v.put(&ty, &"east".into(), "emb", &[0.0, 1.0]);
    v.remove(&ty, &"north".into(), "emb");
    v.remove(&ty, &"absent".into(), "emb");
    assert_eq!(v.count(&ty, "emb"), 3);
    assert_eq!(v.count(&ty, "other"), 0);
    let best = v.nearest(&ty, "emb", &[0.0, 1.0], 1).unwrap();
    assert_eq!(best[0].0.as_str(), "east");
}

#[test]
fn a_zero_vector_scores_zero_and_a_wrong_length_is_refused() {
    let v = index();
    let ty = TypeName::from("doc");
    let hits = v.nearest(&ty, "emb", &[0.0, 0.0], 10).unwrap();
    assert!(hits.iter().all(|(_, s)| *s == 0.0));
    assert!(v.nearest(&ty, "emb", &[1.0], 10).is_err());
    assert!(v.nearest(&ty, "none", &[1.0], 10).unwrap().is_empty());
}
