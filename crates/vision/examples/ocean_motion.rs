//! Verify the same Rust SEA-RAFT connector used by the UI on the editor's original demo generator.
use filmcraft_media::generators::{DemoScene, Generator, render};
use filmcraft_time::FrameRate;
use filmcraft_vision::SeaRaft;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let output = args.next().map(PathBuf::from).ok_or("Supply an output directory, optionally followed by first and last source-frame numbers")?;
    let selection = match (args.next(), args.next()) {
        (None, None) => vec![43i64, 44, 49, 67],
        (Some(first), Some(last)) => {
            let first: i64 = first.to_str().ok_or("Invalid first frame")?.parse()?;
            let last: i64 = last.to_str().ok_or("Invalid last frame")?.parse()?;
            if first < 0 || last <= first || last.saturating_sub(first) >= 300 || last > 430 {
                return Err("Select 2–300 frames within the Ocean Sunset demo (0–430)".into());
            }
            (first..=last).collect()
        }
        _ => return Err("Supply both first and last source-frame numbers".into()),
    };
    let consecutive = selection.len() > 4 || selection.get(1).zip(selection.get(2)).is_some_and(|(a, b)| *b == *a + 1);
    std::fs::create_dir_all(&output)?;
    let rate = FrameRate::FPS_23_976;
    let generator = Generator::Demo(DemoScene::OceanSunset);
    let (width, height) = (640u32, 360u32);
    let cancel = AtomicBool::new(false);
    let mut runtime = SeaRaft::start(&cancel)?;
    let first = *selection.first().ok_or("No source frames")?;
    let mut previous = render(&generator, width, height, rate.tick_of(first).seconds() as f32, first, rate);
    let mut previous_index = first;
    let mut pairs = Vec::new();
    for index in &selection {
        let tick = rate.tick_of(*index);
        let pixels = render(&generator, width, height, tick.seconds() as f32, *index, rate);
        image::save_buffer(output.join(format!("ocean-{index:04}.png")), &pixels, width, height, image::ColorType::Rgba8)?;
        if consecutive && *index == first {
            continue;
        }
        let started = std::time::Instant::now();
        let evidence = runtime.analyze(width as usize, height as usize, &previous, &pixels, &cancel)?;
        let elapsed = started.elapsed().as_secs_f64();
        println!("SEA-RAFT: {previous_index} → {index}: {} vectors, {:.3}s, {:.3}px mean motion", evidence.vectors.len(), elapsed, evidence.mean_motion_pixels);
        pairs.push(serde_json::json!({
            "fromFrame":previous_index, "toFrame":index,
            "fromSourceTicks":rate.tick_of(previous_index).0, "toSourceTicks":tick.0,
            "seconds":elapsed, "evidence":evidence,
        }));
        if consecutive {
            previous = pixels;
            previous_index = *index;
        }
    }
    let report = serde_json::json!({"schemaVersion":3,"analyzer":"filmcraft-vision/sea-raft-M","runtime":runtime.metadata,
        "fixture":"Ocean_Sunset.mp4","fps":{"numerator":24000,"denominator":1001},"sourceFrames":selection,
        "pairs":pairs,"vectorExport":"samples of dense SEA-RAFT flow on a 16-pixel grid"});
    std::fs::write(output.join("sea-raft.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("Exported SEA-RAFT results to {}", output.display());
    Ok(())
}
