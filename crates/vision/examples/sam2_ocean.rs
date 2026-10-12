//! Local SAM 2.1 smoke test on the generated Ocean Sunset; no editor build.
use filmcraft_media::generators::{DemoScene, Generator, render};
use filmcraft_time::FrameRate;
use filmcraft_vision::{
    execution::Device,
    sam2::{self, Object, Prompt},
};
use std::sync::atomic::AtomicBool;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args().nth(1).ok_or("Supply output JSON")?;
    let device: Device = serde_json::from_value(serde_json::Value::String(std::env::args().nth(2).unwrap_or("auto".into())))?;
    let count = std::env::args().nth(3).map(|s| s.parse::<usize>()).transpose()?.unwrap_or(3);
    if !(1..=65).contains(&count) {
        return Err("Choose 1–65 test frames".into());
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let dir = std::env::temp_dir().join(format!("filmcraft-sam-test-{stamp}"));
    std::fs::create_dir(&dir)?;
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let temp = Temp(dir);
    let rate = FrameRate::FPS_23_976;
    let reference = if count > 32 { 0 } else { count / 2 };
    let t = rate.tick_of(43 + reference as i64).seconds();
    let cx = 0.5 + 0.02 * (t * 0.05).sin();
    let cy = 0.42 + t * 0.006;
    for i in 0..count {
        let frame = 43 + i as i64;
        let raw = render(&Generator::Demo(DemoScene::OceanSunset), 320, 180, rate.tick_of(frame).seconds() as f32, frame, rate);
        std::fs::write(temp.0.join(format!("{i}.rgba")), raw)?;
    }
    let object = Object {
        name: "Sun".into(),
        frame_index: reference,
        points: vec![
            Prompt { position: [cx, cy], foreground: true },
            Prompt { position: [cx, cy - 0.12], foreground: false },
            Prompt { position: [cx, cy + 0.15], foreground: false },
        ],
    };
    let started = std::time::Instant::now();
    let result = sam2::infer(320, 180, temp.0.clone(), count, vec![object], &AtomicBool::new(false), device)?;
    for track in &result.tracks {
        for (i, mask) in track.masks.iter().enumerate() {
            if let Some(mask) = mask {
                let (area, center, bounds) = mask.measurements(result.mask_size)?;
                println!("Frame {i}: area {area} centroid {center:?} bounds {bounds:?}");
            }
        }
    }
    println!("Elapsed {:.2}s · {}", started.elapsed().as_secs_f64(), result.runtime);
    std::fs::write(output, serde_json::to_vec(&result)?)?;
    Ok(())
}
