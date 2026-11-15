use super::*;

#[test]
fn char_indices_are_byte_offsets_for_ascii_names() {
    assert_eq!(char_indices("UserViewModel", &[0, 4, 8]), vec![0, 4, 8]);
}

#[test]
fn char_indices_skip_multibyte_characters() {
    assert_eq!(char_indices("ÄbC", &[2, 3]), vec![1, 2]);
}
