//! A breakdown document for people and language models: every number comes from saved tool results.
use crate::{camera_moves::Segment, look::Look, shots::Shot};
use serde::{Deserialize, Serialize};
use std::fmt::Write;

pub const MAX_SHOTS: usize = 10_000;
pub const MAX_OBJECTS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPresence {
    pub shot_index: usize,
    pub name: String,
    pub frames_present: usize,
    pub frames_total: usize,
    pub mean_centroid: Option<[f64; 2]>,
    pub mean_area_fraction: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextNote {
    pub shot_index: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefInput {
    pub title: String,
    pub frame_rate: f64,
    pub frame_count: usize,
    pub start_seconds: f64,
    pub source_size: [u32; 2],
    pub shots: Vec<Shot>,
    pub looks: Vec<Option<Look>>,
    pub camera: Option<Vec<Segment>>,
    pub objects: Vec<ObjectPresence>,
    pub notes: Vec<TextNote>,
    pub limitations: Vec<String>,
}

pub fn position_words(centroid: [f64; 2]) -> &'static str {
    let column = if centroid[0] < 1.0 / 3.0 {
        0
    } else if centroid[0] < 2.0 / 3.0 {
        1
    } else {
        2
    };
    let row = if centroid[1] < 1.0 / 3.0 {
        0
    } else if centroid[1] < 2.0 / 3.0 {
        1
    } else {
        2
    };
    match (row, column) {
        (0, 0) => "upper left",
        (0, 1) => "upper centre",
        (0, 2) => "upper right",
        (1, 0) => "middle left",
        (1, 1) => "centre",
        (1, 2) => "middle right",
        (2, 0) => "lower left",
        (2, 1) => "lower centre",
        _ => "lower right",
    }
}

fn seconds(start: f64, tick: i64, first_tick: i64, ticks_per_second: f64) -> f64 {
    start + (tick.saturating_sub(first_tick)) as f64 / ticks_per_second
}

fn clock(s: f64) -> String {
    let s = s.max(0.0);
    let minutes = (s / 60.0).floor();
    format!("{:02}:{:06.3}", minutes as u64, s - minutes * 60.0)
}

fn temperature(warm: f32) -> &'static str {
    if warm > 0.08 {
        "warm"
    } else if warm < -0.08 {
        "cool"
    } else {
        "neutral"
    }
}

fn saturation_words(s: f32) -> &'static str {
    if s < 0.15 {
        "desaturated"
    } else if s < 0.4 {
        "moderately saturated"
    } else {
        "highly saturated"
    }
}

fn camera_for(segments: &[Segment], shot: &Shot, first_frame: usize) -> Vec<String> {
    let mut out = Vec::new();
    for s in segments {
        let start = s.start_comparison.saturating_add(1).saturating_add(first_frame);
        let end = s.end_comparison.saturating_add(1).saturating_add(first_frame);
        if end < shot.start_frame.max(first_frame + 1) || start > shot.end_frame {
            continue;
        }
        let overlap = end.min(shot.end_frame).saturating_sub(start.max(shot.start_frame)).saturating_add(1);
        out.push(format!("{} ({overlap} frames)", s.movement.label()));
        if out.len() >= 12 {
            break;
        }
    }
    out
}

pub fn markdown(input: &BriefInput, ticks_per_second: i64) -> Result<String, String> {
    if input.shots.len() > MAX_SHOTS
        || input.looks.len() != input.shots.len()
        || input.objects.len() > MAX_OBJECTS
        || input.notes.len() > MAX_OBJECTS
        || !input.frame_rate.is_finite()
        || input.frame_rate <= 0.0
        || !input.start_seconds.is_finite()
        || ticks_per_second <= 0
        || input.title.chars().count() > 200
    {
        return Err("Brief input exceeds limits or has invalid timing".into());
    }
    for look in input.looks.iter().flatten() {
        look.validate()?;
    }
    let tps = ticks_per_second as f64;
    let first_tick = input.shots.first().map(|s| s.start_tick).unwrap_or(0);
    let mut md = String::new();
    let e = |_: std::fmt::Error| "Could not format brief".to_owned();
    writeln!(md, "# Breakdown: {}", input.title.replace(['\n', '\r'], " ")).map_err(e)?;
    writeln!(md).map_err(e)?;
    let duration = input.frame_count as f64 / input.frame_rate;
    writeln!(
        md,
        "{} frames at {:.3} fps ({:.2} s) · source {}×{} · {} shots · {} cuts",
        input.frame_count,
        input.frame_rate,
        duration,
        input.source_size[0],
        input.source_size[1],
        input.shots.len(),
        input.shots.len().saturating_sub(1)
    )
    .map_err(e)?;
    if !input.shots.is_empty() {
        let visible: Vec<&Shot> = input.shots.iter().filter(|s| !s.black).collect();
        if !visible.is_empty() {
            let mean = visible.iter().map(|s| s.frame_count as f64).sum::<f64>() / visible.len() as f64 / input.frame_rate;
            let shortest = visible.iter().map(|s| s.frame_count).min().unwrap_or(0) as f64 / input.frame_rate;
            let longest = visible.iter().map(|s| s.frame_count).max().unwrap_or(0) as f64 / input.frame_rate;
            writeln!(md, "Average shot {mean:.2} s · shortest {shortest:.2} s · longest {longest:.2} s · {} black shots", input.shots.len() - visible.len())
                .map_err(e)?;
        }
    }
    if let Some(camera) = input.camera.as_ref() {
        let mut shares: Vec<(String, usize)> = Vec::new();
        for s in camera {
            match shares.iter_mut().find(|(label, _)| *label == s.movement.label()) {
                Some(entry) => entry.1 += s.comparisons,
                None => shares.push((s.movement.label().to_owned(), s.comparisons)),
            }
        }
        shares.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let words: Vec<String> = shares.iter().take(6).map(|(l, n)| format!("{l} {n}")).collect();
        writeln!(md, "Camera (comparisons): {}", words.join(" · ")).map_err(e)?;
    }
    writeln!(md).map_err(e)?;
    writeln!(md, "## Shot list").map_err(e)?;
    writeln!(md).map_err(e)?;
    writeln!(md, "| # | Start | End | Frames | Boundary | Key | Palette | Camera |").map_err(e)?;
    writeln!(md, "|---|---|---|---|---|---|---|---|").map_err(e)?;
    for (i, shot) in input.shots.iter().enumerate() {
        let look = input.looks.get(i).and_then(|l| l.as_ref());
        let palette: Vec<String> = look.map(|l| l.palette.iter().take(4).map(|c| c.hex.clone()).collect()).unwrap_or_default();
        let camera = input.camera.as_ref().map(|c| camera_for(c, shot, 0).join(", ")).unwrap_or_default();
        writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            i + 1,
            clock(seconds(input.start_seconds, shot.start_tick, first_tick, tps)),
            clock(seconds(input.start_seconds, shot.end_tick, first_tick, tps)),
            shot.frame_count,
            format!("{:?}", shot.boundary).to_lowercase(),
            if shot.black { "black".to_owned() } else { look.map(|l| l.key.clone()).unwrap_or_else(|| "?".into()) },
            palette.join(" "),
            camera
        )
        .map_err(e)?;
    }
    writeln!(md).map_err(e)?;
    for (i, shot) in input.shots.iter().enumerate() {
        let look = input.looks.get(i).and_then(|l| l.as_ref());
        let start = seconds(input.start_seconds, shot.start_tick, first_tick, tps);
        let end = seconds(input.start_seconds, shot.end_tick, first_tick, tps);
        writeln!(
            md,
            "## Shot {} · {} → {} ({} frames, {:.2} s)",
            i + 1,
            clock(start),
            clock(end),
            shot.frame_count,
            shot.frame_count as f64 / input.frame_rate
        )
        .map_err(e)?;
        writeln!(md).map_err(e)?;
        if shot.black {
            writeln!(md, "- Black frames (fade, gap or slate). No look analysis.").map_err(e)?;
            writeln!(md).map_err(e)?;
            continue;
        }
        writeln!(
            md,
            "- Boundary: {}{}",
            format!("{:?}", shot.boundary).to_lowercase(),
            shot.boundary_score.map(|s| format!(" (change score {s:.2})")).unwrap_or_default()
        )
        .map_err(e)?;
        if shot.frame_count > 1 {
            let effective = input.frame_rate * shot.unique_frames as f64 / shot.frame_count as f64;
            if shot.hold_fraction >= 0.3 {
                writeln!(
                    md,
                    "- Cadence: {} unique of {} frames (held {:.0}%): animated at about {:.1} fps, i.e. on {:.0}s",
                    shot.unique_frames,
                    shot.frame_count,
                    shot.hold_fraction * 100.0,
                    effective,
                    (input.frame_rate / effective.max(0.001)).round()
                )
                .map_err(e)?;
            } else {
                writeln!(md, "- Cadence: every frame changes ({} unique of {})", shot.unique_frames, shot.frame_count).map_err(e)?;
            }
        }
        if let Some(l) = look {
            let palette: Vec<String> = l.palette.iter().map(|c| format!("{} {:.0}%", c.hex, c.share * 100.0)).collect();
            writeln!(
                md,
                "- Tone: {} (mean luma {:.2}, contrast {:.2}, {:.0}% near black, {:.0}% near white)",
                l.key,
                l.mean_luma,
                l.luma_spread,
                l.dark_fraction * 100.0,
                l.bright_fraction * 100.0
            )
            .map_err(e)?;
            writeln!(
                md,
                "- Colour: {} · {} (saturation {:.2}, warm bias {:+.2}) · {} distinct colours",
                temperature(l.warm_bias),
                saturation_words(l.mean_saturation),
                l.mean_saturation,
                l.warm_bias,
                l.unique_colors
            )
            .map_err(e)?;
            writeln!(md, "- Palette: {}", palette.join(", ")).map_err(e)?;
            if let Some(p) = l.pixel_pitch {
                let source = (p as f64 * f64::from(input.source_size[0]) / l.analyzed_size[0].max(1) as f64).round();
                writeln!(md, "- Pixel grid: blocks of about {p} analysis px ({source:.0} source px): pixel-art style").map_err(e)?;
            }
            if l.letterbox.iter().any(|v| *v > 0.01) {
                writeln!(
                    md,
                    "- Black bars: top {:.0}% · bottom {:.0}% · left {:.0}% · right {:.0}%",
                    l.letterbox[0] * 100.0,
                    l.letterbox[1] * 100.0,
                    l.letterbox[2] * 100.0,
                    l.letterbox[3] * 100.0
                )
                .map_err(e)?;
            }
        }
        if let Some(camera) = input.camera.as_ref() {
            let moves = camera_for(camera, shot, 0);
            if !moves.is_empty() {
                writeln!(md, "- Camera: {}", moves.join(", ")).map_err(e)?;
            }
        }
        for o in input.objects.iter().filter(|o| o.shot_index == i).take(64) {
            let position = o.mean_centroid.map(position_words).unwrap_or("position unknown");
            let area = o.mean_area_fraction.map(|a| format!(", about {:.0}% of the frame", a * 100.0)).unwrap_or_default();
            writeln!(md, "- Subject “{}”: {} of {} frames, {position}{area}", o.name.replace(['\n', '\r', '|'], " "), o.frames_present, o.frames_total)
                .map_err(e)?;
        }
        for n in input.notes.iter().filter(|n| n.shot_index == i).take(64) {
            writeln!(md, "- Note: {}", n.text.replace(['\n', '\r'], " ")).map_err(e)?;
        }
        let mut prompt: Vec<String> = Vec::new();
        if let Some(l) = look {
            if l.pixel_pitch.is_some() {
                prompt.push("pixel art".into());
            }
            prompt.push(format!("{} lighting", l.key));
            prompt.push(format!("{} {} palette", temperature(l.warm_bias), saturation_words(l.mean_saturation)));
            let hexes: Vec<String> = l.palette.iter().take(4).map(|c| c.hex.clone()).collect();
            if !hexes.is_empty() {
                prompt.push(format!("colours {}", hexes.join(" ")));
            }
        }
        for o in input.objects.iter().filter(|o| o.shot_index == i).take(8) {
            prompt.push(format!("{} at {}", o.name.replace(['\n', '\r', '|'], " "), o.mean_centroid.map(position_words).unwrap_or("unknown position")));
        }
        if let Some(camera) = input.camera.as_ref() {
            let moves = camera_for(camera, shot, 0);
            if let Some(first) = moves.first() {
                prompt.push(format!("camera {}", first.split(" (").next().unwrap_or(first)));
            }
        }
        if shot.hold_fraction >= 0.3 && shot.frame_count > 1 {
            prompt.push(format!("animated on {:.0}s", (shot.frame_count as f64 / shot.unique_frames.max(1) as f64).round()));
        }
        prompt.push(format!("{:.2} s", shot.frame_count as f64 / input.frame_rate));
        writeln!(md).map_err(e)?;
        writeln!(md, "```prompt").map_err(e)?;
        writeln!(md, "{}", prompt.join(", ")).map_err(e)?;
        writeln!(md, "```").map_err(e)?;
        writeln!(md).map_err(e)?;
    }
    if !input.limitations.is_empty() {
        writeln!(md, "## Limitations").map_err(e)?;
        writeln!(md).map_err(e)?;
        for l in input.limitations.iter().take(64) {
            writeln!(md, "- {}", l.replace(['\n', '\r'], " ")).map_err(e)?;
        }
    }
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_moves::Movement;
    use crate::look::PaletteColor;
    use crate::shots::Boundary;
    fn shot(index: usize, start: usize, end: usize, black: bool) -> Shot {
        Shot {
            index,
            start_frame: start,
            end_frame: end,
            start_tick: start as i64 * 100,
            end_tick: end as i64 * 100,
            frame_count: end - start + 1,
            keyframe: (start + end) / 2,
            boundary: if index == 0 { Boundary::RangeStart } else { Boundary::Cut },
            boundary_score: (index > 0).then_some(0.7),
            black,
            mean_luma: 0.2,
            mean_saturation: 0.3,
            warm_bias: -0.2,
            duplicate_frames: if black { 0 } else { 8 },
            unique_frames: if black { end - start + 1 } else { (end - start + 1).saturating_sub(8) },
            hold_fraction: if black { 0.0 } else { 8.0 / (end - start).max(1) as f32 },
        }
    }
    fn look() -> Look {
        Look {
            analyzed_size: [320, 180],
            mean_luma: 0.2,
            luma_spread: 0.1,
            dark_fraction: 0.4,
            bright_fraction: 0.01,
            mean_saturation: 0.3,
            warm_bias: -0.2,
            key: "low-key".into(),
            unique_colors: 12,
            palette: vec![
                PaletteColor { rgb: [10, 20, 40], hex: "#0a1428".into(), share: 0.7 },
                PaletteColor { rgb: [200, 150, 40], hex: "#c89628".into(), share: 0.3 },
            ],
            pixel_pitch: Some(2),
            letterbox: [0.0; 4],
        }
    }
    #[test]
    fn markdown_lists_shots_prompts_and_subjects() {
        let input = BriefInput {
            title: "Caveman".into(),
            frame_rate: 30.0,
            frame_count: 24,
            start_seconds: 0.0,
            source_size: [640, 360],
            shots: vec![shot(0, 0, 11, false), shot(1, 12, 14, true), shot(2, 15, 23, false)],
            looks: vec![Some(look()), None, Some(look())],
            camera: Some(vec![Segment {
                movement: Movement::PanLeft,
                start_comparison: 0,
                end_comparison: 5,
                comparisons: 6,
                mean_dx_pixels: Some(2.0),
                mean_dy_pixels: Some(0.0),
                mean_scale: Some(1.0),
                mean_rotation_degrees: Some(0.0),
            }]),
            objects: vec![ObjectPresence {
                shot_index: 2,
                name: "Caveman".into(),
                frames_present: 9,
                frames_total: 9,
                mean_centroid: Some([0.8, 0.7]),
                mean_area_fraction: Some(0.12),
            }],
            notes: vec![TextNote { shot_index: 0, text: "2:47 AM clock".into() }],
            limitations: vec!["Heuristics".into()],
        };
        let md = markdown(&input, 10_000).unwrap();
        assert!(md.contains("# Breakdown: Caveman"));
        assert!(md.contains("3 shots · 2 cuts"));
        assert!(md.contains("| 1 | 00:00.000 | 00:00.110 | 12 | rangestart | low-key | #0a1428 #c89628 | pan left (6 frames) |"));
        assert!(md.contains("Black frames"));
        assert!(md.contains("Subject “Caveman”: 9 of 9 frames, lower right, about 12% of the frame"));
        assert!(md.contains("Pixel grid: blocks of about 2 analysis px (4 source px)"));
        assert!(md.contains(
            "```prompt\npixel art, low-key lighting, cool moderately saturated palette, colours #0a1428 #c89628, camera pan left, animated on 3s, 0.40 s\n```"
        ));
        assert!(md.contains("Note: 2:47 AM clock"));
        assert!(md.contains("- Heuristics"));
        assert_eq!(position_words([0.1, 0.1]), "upper left");
        assert_eq!(position_words([0.5, 0.5]), "centre");
    }
    #[test]
    fn invalid_inputs_are_rejected() {
        let mut input = BriefInput {
            title: "x".into(),
            frame_rate: 30.0,
            frame_count: 1,
            start_seconds: 0.0,
            source_size: [1, 1],
            shots: vec![shot(0, 0, 0, false)],
            looks: vec![],
            camera: None,
            objects: vec![],
            notes: vec![],
            limitations: vec![],
        };
        assert!(markdown(&input, 100).is_err(), "looks must align with shots");
        input.looks = vec![None];
        assert!(markdown(&input, 0).is_err());
        input.frame_rate = f64::NAN;
        assert!(markdown(&input, 100).is_err());
        input.frame_rate = 30.0;
        assert!(markdown(&input, 100).unwrap().contains("Shot 1"));
    }
}
