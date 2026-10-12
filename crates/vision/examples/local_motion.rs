//! Verify residuals using a combined SEA-RAFT/global-motion export without launching the editor.
use filmcraft_vision::{
    MotionPair,
    global_motion::GlobalMotion,
    local_motion::{LocalMotion, estimate},
};
use serde_json::{Value, json};
use std::io::Read;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("Supply combined-export JSON path")?;
    let output = args.next().ok_or("Supply residual-output JSON path")?;
    let file = std::fs::File::open(input)?;
    if file.metadata()?.len() > 100_000_000 {
        return Err("Input exceeds 100 MB".into());
    }
    let value: Value = serde_json::from_reader(file.take(100_000_001))?;
    let results = value.get("results").and_then(Value::as_array).ok_or("Expected combined tool export")?;
    let flow = results.iter().find(|r| r.get("tool").and_then(Value::as_str) == Some("opticalFlow")).and_then(|r| r.get("result")).ok_or("Missing flow")?;
    let global =
        results.iter().find(|r| r.get("tool").and_then(Value::as_str) == Some("globalMotion")).and_then(|r| r.get("result")).ok_or("Missing global motion")?;
    if flow.get("captures") != global.get("captures") {
        return Err("Input ranges differ".into());
    }
    let pairs: Vec<MotionPair> = serde_json::from_value(flow.get("pairs").cloned().ok_or("Missing flow pairs")?)?;
    let fits: Vec<GlobalMotion> = serde_json::from_value(global.get("pairs").cloned().ok_or("Missing global pairs")?)?;
    if pairs.is_empty() || pairs.len() > 299 || pairs.len() != fits.len() {
        return Err("Invalid pair counts".into());
    }
    let residuals = pairs.iter().zip(&fits).map(|(pair, fit)| estimate(pair, fit)).collect::<Result<Vec<_>, _>>()?;
    let available = residuals.iter().filter(|p| matches!(p, LocalMotion::Estimated { .. })).count();
    let maximum = residuals
        .iter()
        .filter_map(|p| match p {
            LocalMotion::Estimated { max_residual_pixels, .. } => Some(*max_residual_pixels),
            _ => None,
        })
        .fold(0.0, f64::max);
    std::fs::write(output, serde_json::to_vec(&json!({"schemaVersion":1,"analyzer":"filmcraft-vision/local-residual-motion-v1","pairs":residuals}))?)?;
    println!("{} comparisons · {} available · maximum residual {:.4} analysis pixels", pairs.len(), available, maximum);
    Ok(())
}
