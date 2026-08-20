use engine_core::{
    math2d::{Camera2D, CameraBounds, Viewport},
    transform::Vec2,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let viewport = Viewport::new(256.0, 256.0)?;
    let camera = Camera2D::new(Vec2::new(999.0, -999.0), 16.0)?
        .with_rotation(0.25)?
        .with_bounds(CameraBounds::new(
            Vec2::new(-8.0, -8.0),
            Vec2::new(8.0, 8.0),
        )?)?;
    let center = camera.screen_to_world(Vec2::new(128.0, 128.0), viewport)?;
    let corner = camera.screen_to_world(Vec2::zero(), viewport)?;
    let round_trip = camera.screen_to_world(
        camera.world_to_screen(Vec2::new(1.5, -2.0), viewport)?,
        viewport,
    )?;
    println!(
        "camera_demo_ok center={center:?} corner={corner:?} round_trip={round_trip:?} rotation={:.3} zoom={:.1}",
        camera.rotation, camera.zoom
    );
    Ok(())
}
