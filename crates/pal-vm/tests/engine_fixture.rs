use pal_vm::{Engine, EngineConfig};

#[test]
fn engine_new_starts_without_diagnostic_visuals() {
    let engine = Engine::new(EngineConfig::default()).expect("engine should boot without root");

    assert_eq!(engine.sprites().len(), 0);
    assert_eq!(engine.sprites().surface_count(), 0);
    assert_eq!(engine.sprites().render_node_count(), 0);
    assert_eq!(engine.sprites().commands().len(), 0);
    assert_eq!(engine.task_system().active_task_count(), 0);
}

#[test]
fn pal_debug_does_not_create_runtime_objects() {
    std::env::set_var("PAL_DEBUG", "1");
    let engine = Engine::new(EngineConfig::default()).expect("engine should boot without root");
    std::env::remove_var("PAL_DEBUG");

    assert_eq!(engine.sprites().len(), 0);
    assert_eq!(engine.sprites().surface_count(), 0);
    assert_eq!(engine.sprites().render_node_count(), 0);
    assert_eq!(engine.sprites().commands().len(), 0);
    assert_eq!(engine.task_system().active_task_count(), 0);
}
