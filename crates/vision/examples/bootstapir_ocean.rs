//! Verify the Rust BootsTAPIR connector on known Ocean Sunset motion, without an editor build.
use filmcraft_media::generators::{DemoScene, Generator, render};
use filmcraft_time::FrameRate;
use filmcraft_vision::point_tracking::{Query, grid, infer};
use std::sync::atomic::AtomicBool;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args().nth(1).ok_or("Supply output JSON path")?;
    let rate = FrameRate::FPS_23_976;
    let first = 43i64;
    let last = 67i64;
    let t = rate.tick_of(first).seconds();
    let cx = 0.5 + 0.02 * (t * 0.05).sin();
    let cy = 0.42 + t * 0.006;
    let points = [[cx - 0.03, cy], [cx + 0.03, cy], [cx, cy - 0.05], [cx, cy + 0.05]];
    let queries = if std::env::args().nth(2).as_deref() == Some("grid") {
        grid(0, 4, 25)?
    } else {
        points.into_iter().map(|position| Query { frame_index: 0, position }).collect()
    };
    let frames = (first..=last).map(|i| render(&Generator::Demo(DemoScene::OceanSunset), 320, 180, rate.tick_of(i).seconds() as f32, i, rate)).collect();
    let started = std::time::Instant::now();
    let result = infer(320, 180, frames, queries, &AtomicBool::new(false))?;
    for (i, track) in result.tracks.iter().enumerate() {
        if let Some((a, b)) = track.observations.first().zip(track.observations.last()) {
            println!(
                "Point {} · Δx {:.3}px · Δy {:.3}px at 640x360 · visible {}",
                i + 1,
                (b.position[0] - a.position[0]) * 640.0,
                (b.position[1] - a.position[1]) * 360.0,
                b.visible
            );
        }
    }
    println!(
        "Expected generator Δy {:.3}px · elapsed {:.2}s · runtime {}",
        (rate.tick_of(last).seconds() - t) * 0.006 * 360.0,
        started.elapsed().as_secs_f64(),
        result.runtime
    );
    std::fs::write(output, serde_json::to_vec(&result)?)?;
    Ok(())
}
