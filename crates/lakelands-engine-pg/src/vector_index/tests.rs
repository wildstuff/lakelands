use super::*;

#[test]
fn a_unit_vector_has_length_one_and_zeros_have_none() {
    assert_eq!(unit_text(&[3.0, 4.0]).as_deref(), Some("[0.6,0.8]"));
    assert_eq!(unit_text(&[0.0, 0.0]), None);
}

#[test]
fn a_reader_reads_its_class_graph_and_none_stale_or_unbuilt() {
    let graph = |class: &str, generation: Option<i64>, stale: Option<i64>| Graph {
        field: "e".into(),
        class: class.into(),
        dims: 2,
        generation,
        stale,
        next: None,
    };
    let (a, b) = (graph("a", Some(1), None), graph("b", Some(2), None));
    let name = |c: &str, g: i64| Some(bare("t", "e", c, g));
    assert_eq!(pick(&[a.clone(), b.clone()], "t", true, 2), name("a", 1));
    assert_eq!(pick(&[a.clone(), b.clone()], "t", false, 2), name("b", 2));
    assert_eq!(pick(std::slice::from_ref(&b), "t", true, 2), name("b", 2));
    assert_eq!(pick(&[a.clone(), b.clone()], "t", true, 3), None);
    // A held record's reader does not fall back to B while A is built.
    let building = graph("a", None, None);
    assert_eq!(pick(&[building, b.clone()], "t", true, 2), None);
    assert_eq!(
        pick(&[a, graph("b", Some(2), Some(9))], "t", false, 2),
        None
    );
}
