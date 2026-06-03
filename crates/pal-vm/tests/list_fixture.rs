use pal_vm::PalListSystem;

#[test]
fn pal_list_push_pop_and_count_match_native_ordering() {
    let mut lists = PalListSystem::new();
    let list = lists.create();

    assert!(lists.push(list, 10, 1));
    assert!(lists.push(list, 20, 1));
    assert!(lists.push_last(list, 30, 2));
    assert_eq!(lists.len(list), 3);
    assert_eq!(lists.get_data_count(list, 1), 2);
    assert_eq!(lists.get_data(list, 1, 0), 20);
    assert_eq!(lists.get_data(list, 1, 1), 10);
    assert_eq!(lists.get_data(list, 2, 0), 30);

    assert_eq!(lists.pop(list, 1), 20);
    assert_eq!(lists.pop_first(list), 10);
    assert_eq!(lists.pop_first(list), 30);
    assert_eq!(lists.pop_first(list), 0);
}

#[test]
fn pal_list_delete_and_find_use_data_and_tag_like_pal() {
    let mut lists = PalListSystem::new();
    let list = lists.create();
    assert!(lists.push_last(list, 11, 7));
    assert!(lists.push_last(list, 12, 8));
    assert!(lists.push_last(list, 13, 7));

    let first = lists.find(list, 7).unwrap();
    assert_eq!(lists.find_next(list, first), Some(2));
    assert!(lists.delete_data(list, 12));
    assert_eq!(lists.get_data_count(list, 8), 0);
    assert!(!lists.delete_data(list, 99));

    assert!(lists.release(list));
    assert_eq!(lists.get_data_count(list, 7), 0);
}
