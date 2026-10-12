//! Group an exported Local motion result without inference or an editor build.
use filmcraft_vision::{
    local_motion::LocalMotion,
    motion_regions::{Regions, Settings, group},
};
use serde_json::{Value, json};
use std::io::Read;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("Supply Local motion JSON path")?;
    let output = args.next().ok_or("Supply grouped-output JSON path")?;
    let file = std::fs::File::open(input)?;
    if file.metadata()?.len() > 100_000_000 {
        return Err("Input exceeds 100 MB".into());
    }
    let value: Value = serde_json::from_reader(file.take(100_000_001))?;
    if value.get("tool").and_then(Value::as_str) != Some("localMotion") {
        return Err("Expected a Local motion tool export".into());
    }
    let local = value.get("result").ok_or("Missing local result")?;
    let pairs: Vec<LocalMotion> = serde_json::from_value(local.get("pairs").cloned().ok_or("Missing residual pairs")?)?;
    if pairs.is_empty() || pairs.len() > 299 {
        return Err("Invalid pair count".into());
    }
    let settings = Settings::default();
    let grouped = pairs.iter().map(|p| group(p, settings)).collect::<Result<Vec<_>, _>>()?;
    let available = grouped.iter().filter(|p| matches!(p, Regions::Estimated { .. })).count();
    let populated = grouped.iter().filter(|p| matches!(p,Regions::Estimated {regions,..} if !regions.is_empty())).count();
    let count: usize = grouped
        .iter()
        .map(|p| match p {
            Regions::Estimated { regions, .. } => regions.len(),
            _ => 0,
        })
        .sum();
    std::fs::write(
        output,
        serde_json::to_vec(&json!({"schemaVersion":1,"analyzer":"filmcraft-vision/residual-regions-v1","settings":settings,"pairs":grouped}))?,
    )?;
    println!("{} comparisons · {} available · {} with groups · {} total frame-local groups", pairs.len(), available, populated, count);
    Ok(())
}
