//! Real-model smoke test across an overlapping chunk boundary without compiling the editor.
use filmcraft_media::generators::{DemoScene, Generator, render};
use filmcraft_time::FrameRate;
use filmcraft_vision::{
    execution::Device,
    point_tracking::{self, Query},
    point_tracking_chunks::track_with,
};
use std::sync::atomic::AtomicBool;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args().nth(1).ok_or("Supply output JSON path")?;
    let rate = FrameRate::FPS_23_976;
    let first = 43i64;
    let count = 31usize;
    let t = rate.tick_of(first).seconds();
    let cx = 0.5 + 0.02 * (t * 0.05).sin();
    let cy = 0.42 + t * 0.006;
    let queries = vec![Query { frame_index: 0, position: [cx, cy - 0.05] }, Query { frame_index: 0, position: [cx, cy + 0.05] }];
    let cancel = AtomicBool::new(false);
    let result = track_with(
        count,
        queries,
        &cancel,
        16,
        |frame| {
            let source = first + frame as i64;
            Ok((320, 180, render(&Generator::Demo(DemoScene::OceanSunset), 320, 180, rate.tick_of(source).seconds() as f32, source, rate)))
        },
        |frames, w, h, q| point_tracking::infer_with_device(w, h, frames, q, &cancel, Device::Cpu),
        |done, total| println!("Processed {done}/{total}"),
    )?;
    println!("{} frames · {} chunks", result.frame_count, result.chunks.len());
    for (i, track) in result.tracks.iter().enumerate() {
        if let Some((a, b)) = track.observations.first().and_then(Option::as_ref).zip(track.observations.last().and_then(Option::as_ref)) {
            println!("Point {} · downward Δy {:.3}px · visible {}", i + 1, (b.position[1] - a.position[1]) * 360.0, b.visible);
        }
    }
    println!("Expected downward Δy {:.3}px", (rate.tick_of(first + count as i64 - 1).seconds() - t) * 0.006 * 360.0);
    std::fs::write(output, serde_json::to_vec(&result)?)?;
    Ok(())
}
