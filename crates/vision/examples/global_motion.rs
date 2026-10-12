//! Evaluate global image motion against an existing SEA-RAFT JSON export; no model/editor build.
use filmcraft_vision::{
    MotionPair,
    global_motion::{GlobalMotion, estimate},
};
use serde::Deserialize;
#[derive(Deserialize)]
struct Export {
    pairs: Vec<MotionPair>,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("Supply a SEA-RAFT JSON export")?;
    let file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    if size > 100_000_000 {
        return Err("Export is too large for this benchmark".into());
    }
    let report: Export = serde_json::from_reader(file)?;
    if report.pairs.len() > 299 {
        return Err("Too many frame comparisons".into());
    }
    let mut results = Vec::new();
    let (mut insufficient, mut max_displacement, mut max_rotation, mut max_scale_delta) = (0usize, 0.0f64, 0.0f64, 0.0f64);
    for pair in &report.pairs {
        let result = estimate(pair, 0.5)?;
        match &result {
            GlobalMotion::Estimated { fit } => {
                max_displacement = max_displacement.max(fit.center_displacement_pixels[0].hypot(fit.center_displacement_pixels[1]));
                max_rotation = max_rotation.max(fit.rotation_degrees.abs());
                max_scale_delta = max_scale_delta.max((fit.scale - 1.0).abs());
            }
            GlobalMotion::InsufficientEvidence { .. } => insufficient += 1,
        }
        results.push(result);
    }
    println!(
        "{} comparisons; {} insufficient; max center displacement {:.5}px; max rotation {:.5}°; max scale deviation {:.7}",
        results.len(),
        insufficient,
        max_displacement,
        max_rotation,
        max_scale_delta
    );
    let output = std::env::args_os().nth(2).ok_or("Supply an output JSON path")?;
    std::fs::write(output, serde_json::to_vec_pretty(&results)?)?;
    Ok(())
}
