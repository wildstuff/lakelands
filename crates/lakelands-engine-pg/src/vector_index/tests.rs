use super::*;

#[test]
fn a_unit_vector_has_length_one_and_zeros_have_none() {
    assert_eq!(unit_text(&[3.0, 4.0]).as_deref(), Some("[0.6,0.8]"));
    assert_eq!(unit_text(&[0.0, 0.0]), None);
}

#[test]
fn the_search_asks_the_index_for_at_least_200_and_at_most_1000() {
    assert!(search_settings(1).contains("ef_search = 200;"));
    assert!(search_settings(300).contains("ef_search = 300;"));
    assert!(search_settings(u32::MAX).contains("ef_search = 1000;"));
    assert!(search_settings(1).contains("max_scan_tuples = 20000;"));
}

#[test]
fn a_tie_at_the_boundary_is_pulled_deeper_and_past_the_ceiling_scanned() {
    let ranked = |scores: &[f64]| -> Vec<(RecordKey, f64)> {
        let keyed = |(i, s): (usize, &f64)| (format!("k{i}").as_str().into(), *s);
        scores.iter().enumerate().map(keyed).collect()
    };
    // The third scores as the second: the boundary of two cuts a tie,
    // in whatever order the index handed them out.
    assert!(cuts_a_tie(&ranked(&[0.9, 0.5, 0.5]), 2));
    assert!(cuts_a_tie(&ranked(&[0.5, 0.9, 0.5, 0.5]), 2));
    // It scores below, no more came than `k`, or nothing is asked.
    assert!(!cuts_a_tie(&ranked(&[0.4, 0.9, 0.5]), 2));
    assert!(!cuts_a_tie(&ranked(&[0.5, 0.5]), 2));
    assert!(!cuts_a_tie(&ranked(&[0.5]), 0));
    assert!(!cuts_a_tie(&[], 1));
    assert_eq!([deeper(11), deeper(3000)], [Some(22), Some(4096)]);
    assert_eq!([deeper(4096), deeper(u32::MAX)], [None, None]);
}
