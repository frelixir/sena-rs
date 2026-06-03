use pal_vm::{PalMemoryHandle, PalMemorySystem};

#[test]
fn pal_memory_alloc_zero_flag_and_free_track_allocation_count() {
    let mut memory = PalMemorySystem::new();
    assert!(memory.alloc(0, 1).is_none());
    let zeroed = memory.alloc(4, 1).unwrap();
    assert_eq!(memory.get(zeroed).unwrap(), &[0, 0, 0, 0]);
    let uninitialized = memory.alloc(3, 0).unwrap();
    assert_eq!(memory.get(uninitialized).unwrap(), &[0xCD, 0xCD, 0xCD]);
    assert_eq!(memory.allocation_count(), 2);
    assert_eq!(memory.allocated_bytes(), 7);

    memory.get_mut(zeroed).unwrap()[2] = 9;
    assert_eq!(memory.get(zeroed).unwrap()[2], 9);
    assert!(memory.free(zeroed));
    assert!(!memory.free(zeroed));
    assert!(!memory.free(PalMemoryHandle(999)));
    assert_eq!(memory.allocation_count(), 1);
}
