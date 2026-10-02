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
}
