use super::*;

#[test]
fn type_table_is_in_declaration_order() {
    for (index, (ty, _, _)) in TYPES.iter().enumerate() {
        assert_eq!(*ty as usize, index);
    }
}
