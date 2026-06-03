use std::fs;
use std::path::PathBuf;

use pal_vm::MSpriteSystem;

#[test]
#[ignore = "large local movie fixture"]
fn msprite_decodes_first_wmv_frame_from_fixture() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testcase/movie/opening.wmv");
    let bytes = fs::read(&fixture).expect("opening.wmv fixture");
    let mut system = MSpriteSystem::new();
    let loaded = system
        .load_wmv("opening.wmv", bytes)
        .expect("decode first WMV frame");
    assert!(loaded.width > 0);
    assert!(loaded.height > 0);
    assert_eq!(
        loaded.rgba.len(),
        loaded.width as usize * loaded.height as usize * 4
    );
    assert!(system.check(loaded.handle));
}
