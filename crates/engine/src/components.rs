//! Reverse-engineering components applied to clips (the timeline's RE lane): `components.add`
//! writes a component's keyframes and its record in one undo step, `components.remove` takes
//! both away again, `components.list` reports them with their change marks.
use filmcraft_project::{
    AppliedComponent, ClipId, ComponentId, ComponentKind, ComponentMode, ComponentSample, ComponentSource, ItemKind, Label, ParamValue, ProjectLut, Track,
    TrackId, TrackKind, find_effect,
};
use filmcraft_render::look_transfer::{self, ColorStats};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, clip_p, f64_p, has_seq, keyframe_request, str_p, u64_p, write_keyframes};
use crate::{EngineError, Result, Session};

const LOOK_LUT_SIZE: usize = 17;
const LOOK_MEASURE_SIDE: f32 = 320.0;

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(id: &'static str, label: &'static str, params: &'static str, enabled: Enabled, run: Run, journal: bool) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled, run, journal }
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        spec(
            "components.add",
            "Apply Component",
            r#"{"clip":id,"kind":"camera2d","name":str,"mode":"follow"|"stabilize","start":ticks,"duration":ticks,"effect":"motion"?,"keyframes":{"<param>":[{"time":ticks,"value":…},…]}?,"set":{"<param>":value}?,"replace":bool?,"source":{"sequence":str,"width":n,"height":n,"frameRate":f,"startFrame":n,"endFrame":n,"file":str?}?,"samples":[{"tick":n,"dx":f,"dy":f,"scale":f,"rotation":f,"residual":f,"state":str?},…]?} — the keyframes and the component record in one undo step"#,
            has_seq,
            add,
            true,
        ),
        spec("components.remove", "Remove Component", r#"{"id":n}"#, has_seq, remove, true),
        spec(
            "components.setEnabled",
            "Bypass or Enable Components",
            r#"{"id":n?,"enabled":bool} — one component, or every component of the sequence without `id`; bypassing switches the component's effect off on its clip so the picture shows without it"#,
            has_seq,
            set_enabled,
            true,
        ),
        spec(
            "components.applyLookRange",
            "Apply Look to Range",
            r#"{"start":ticks,"end":ticks,"color":{"mean":[L,a,b],"std":[L,a,b],"weight":n},"name":str?,"summary":str?,"strength":0–1=1,"source":{…}?} — measures what the sequence shows in the range, bakes the colour transfer onto it as a LUT, and puts it on a new adjustment layer clip over the range (a free video track, else a new one) with a look component that follows the clip; one undo step"#,
            has_seq,
            apply_look_range,
            true,
        ),
        spec("components.list", "List Components", r#"{}"#, always, list, false),
    ]
}

fn apply_look_range(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "components.applyLookRange";
    let start = Tick(p.get("start").and_then(Value::as_i64).ok_or_else(|| bad(cmd, "need integer `start` ticks"))?);
    let end = Tick(p.get("end").and_then(Value::as_i64).ok_or_else(|| bad(cmd, "need integer `end` ticks"))?);
    let target: ColorStats = serde_json::from_value(p.get("color").cloned().ok_or_else(|| bad(cmd, "need `color` statistics"))?)
        .map_err(|e| bad(cmd, format!("bad `color`: {e}")))?;
    let strength = f64_p(p, "strength").unwrap_or(1.0);
    if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
        return Err(bad(cmd, "`strength` must be 0–1"));
    }
    let name: String = str_p(p, "name").filter(|n| !n.trim().is_empty()).unwrap_or("Look").chars().take(200).collect();
    let summary: String = str_p(p, "summary").unwrap_or("").chars().take(4096).collect();
    let source: ComponentSource = match p.get("source") {
        None | Some(Value::Null) => ComponentSource::default(),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| bad(cmd, format!("bad `source`: {e}")))?,
    };
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let (width, height, rate) = {
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        (q.settings.width, q.settings.height, q.settings.frame_rate)
    };
    if start < Tick::ZERO || end <= start || end.0.checked_sub(start.0).is_none() {
        return Err(bad(cmd, "the range must lie inside supported bounds and end after it starts"));
    }
    let dur = rate.snap_nearest(end - start).max(rate.frame_duration());
    let scale = (LOOK_MEASURE_SIDE / width.max(height).max(1) as f32).min(1.0);
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let mut parts = Vec::new();
    for fraction in [0.15f64, 0.5, 0.85] {
        let t = Tick(start.0.saturating_add((dur.0 as f64 * fraction) as i64));
        if let Some(image) = filmcraft_render::render_item(&s.project, seq_id, t, scale, &provider)
            && let Some(stats) = look_transfer::stats(&image)
        {
            parts.push(stats);
        }
    }
    let from = look_transfer::merge(&parts).ok_or_else(|| bad(cmd, "nothing renders in that range to transfer the look onto"))?;
    let lut = look_transfer::transfer_lut(&from, &target, LOOK_LUT_SIZE, strength as f32, &format!("FilmCraft {name}")).map_err(|e| bad(cmd, e))?;
    let text = lut.to_cube();
    let def = find_effect("lumetri").ok_or_else(|| bad(cmd, "Lumetri Color is not available"))?;
    let (clip, component, track, lut_ref) = s.edit("Apply Look to Range", |pr, st| {
        let lut_id = format!("lut{}", pr.alloc_id());
        let lut_ref = format!("lib:{lut_id}");
        pr.luts.push(ProjectLut {
            id: lut_id,
            name: format!("{name} · adjustment layer"),
            source_path: None,
            format: "cube".into(),
            text: text.as_str().into(),
        });
        let item = pr.add_item(&name, Label::Lavender, ItemKind::AdjustmentLayer { width, height, rate, duration: dur }, None);
        let mut ti = pr.make_track_item(item, TrackKind::Video, start, TimeRange::new(Tick::ZERO, dur), rate).ok_or_else(|| bad(cmd, "bad item"))?;
        let mut lumetri = def.instance();
        let mut previous = std::collections::BTreeMap::new();
        for (pid, value) in
            [("creative_on", ParamValue::Bool(true)), ("look_lut", ParamValue::Text(lut_ref.clone())), ("look_intensity", ParamValue::Float(100.0))]
        {
            if let Some(prm) = lumetri.param_mut(pid) {
                previous.insert(pid.to_string(), prm.value.clone());
                prm.value = value;
            }
        }
        ti.effects.push(lumetri);
        let clip = ti.id;
        let component_id = ComponentId(pr.alloc_id());
        let new_track = TrackId(pr.alloc_id());
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let range = TimeRange::new(start, dur);
        let free = |tr: &Track| !tr.locked && !tr.items.iter().any(|i| i.range().overlaps(&range));
        let top = q.video_tracks.iter().rposition(|tr| tr.items.iter().any(|i| i.range().overlaps(&range))).map_or(0, |k| k + 1);
        let idx = match (top..q.video_tracks.len()).find(|&i| q.video_tracks.get(i).is_some_and(free)) {
            Some(i) => i,
            None => {
                let n = q.video_tracks.len() + 1;
                if n > crate::sequence_tools::MAX_TRACKS {
                    return Err(bad(cmd, format!("a sequence has at most {} video tracks", crate::sequence_tools::MAX_TRACKS)));
                }
                q.video_tracks.push(Track::new(new_track, TrackKind::Video, format!("Video {n}")));
                q.video_tracks.len() - 1
            }
        };
        let track = q.video_tracks.get_mut(idx).ok_or_else(|| bad(cmd, "no video track"))?;
        track.items.push(ti);
        track.sort();
        let track_id = track.id;
        let component = AppliedComponent {
            id: component_id,
            kind: ComponentKind::Look,
            name: name.clone(),
            mode: ComponentMode::Follow,
            clip,
            start,
            duration: dur,
            effect: "lumetri".into(),
            params: vec!["creative_on".into(), "look_lut".into(), "look_intensity".into()],
            source: source.clone(),
            samples: Vec::new(),
            summary: summary.clone(),
            enabled: true,
            previous,
            follows_clip: true,
        };
        component.check().map_err(|e| bad(cmd, e))?;
        q.components.push(component);
        q.check().map_err(EngineError::Other)?;
        st.selection = vec![clip];
        Ok((clip, component_id, track_id, lut_ref))
    })?;
    Ok(json!({"clip": clip.0, "component": component.0, "track": track.0, "lut": lut_ref, "start": start.0, "duration": dur.0}))
}

fn set_enabled(s: &mut Session, p: &Value) -> Result<Value> {
    let enabled = p.get("enabled").and_then(Value::as_bool).ok_or_else(|| bad("components.setEnabled", "need boolean `enabled`"))?;
    let id = u64_p(p, "id").map(ComponentId);
    let label = if enabled { "Enable Component" } else { "Bypass Component" };
    let changed = s.edit_sequence(label, |q, _, _| {
        if let Some(id) = id
            && !q.components.iter().any(|c| c.id == id)
        {
            return Err(bad("components.setEnabled", "no such component"));
        }
        let targets: Vec<(ClipId, String)> = q
            .components
            .iter_mut()
            .filter(|c| id.is_none_or(|id| c.id == id))
            .map(|c| {
                c.enabled = enabled;
                (c.clip, c.effect.clone())
            })
            .collect();
        let mut changed = 0usize;
        for (clip, effect) in &targets {
            if let Some((_, it)) = q.find_item_mut(*clip)
                && let Some(e) = it.effects.iter_mut().find(|e| &e.effect == effect)
            {
                e.enabled = enabled;
                changed = changed.saturating_add(1);
            }
        }
        Ok(changed)
    })?;
    Ok(json!({"enabled": enabled, "effects": changed}))
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = clip_p(p, "clip").ok_or_else(|| bad("components.add", "need `clip`"))?;
    let kind = match str_p(p, "kind").unwrap_or("camera2d") {
        "camera2d" => ComponentKind::Camera2d,
        "look" => ComponentKind::Look,
        other => return Err(bad("components.add", format!("unknown component kind `{other}`"))),
    };
    let mode = match str_p(p, "mode").unwrap_or("follow") {
        "follow" => ComponentMode::Follow,
        "stabilize" => ComponentMode::Stabilize,
        other => return Err(bad("components.add", format!("`mode` must be follow or stabilize, not `{other}`"))),
    };
    let name = str_p(p, "name").unwrap_or("Component").to_string();
    let start = Tick(p.get("start").and_then(Value::as_i64).ok_or_else(|| bad("components.add", "need integer `start` ticks"))?);
    let duration = Tick(p.get("duration").and_then(Value::as_i64).ok_or_else(|| bad("components.add", "need integer `duration` ticks"))?);
    let request = keyframe_request(p, "keyframes", "components.add")?;
    let effect = match &request.effect {
        Value::String(e) => e.clone(),
        _ => return Err(bad("components.add", "`effect` must be an effect id such as motion")),
    };
    let source: ComponentSource = match p.get("source") {
        None | Some(Value::Null) => ComponentSource::default(),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| bad("components.add", format!("bad `source`: {e}")))?,
    };
    let samples: Vec<ComponentSample> = match p.get("samples") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| bad("components.add", format!("bad `samples`: {e}")))?,
    };
    let params: Vec<String> = request.params.keys().cloned().collect();
    let summary = str_p(p, "summary").unwrap_or("").chars().take(4096).collect::<String>();
    let (id, written) = s.edit_sequence("Apply Component", |q, ctx, _| {
        let (track, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        let previous: std::collections::BTreeMap<String, filmcraft_project::ParamValue> = it
            .effects
            .iter()
            .find(|e| e.effect == effect)
            .map(|e| {
                request
                    .statics
                    .keys()
                    .map(|k| (k.clone(), e.param(k).map(|p| p.value.clone()).or_else(|| e.def().and_then(|d| d.param(k)).map(|d| d.default.clone()))))
                    .filter_map(|(k, v)| v.map(|v| (k, v)))
                    .collect()
            })
            .unwrap_or_default();
        let written = write_keyframes(it, &request, "components.add")?;
        let id = ComponentId(ctx.alloc());
        let component = AppliedComponent {
            id,
            kind,
            name: name.clone(),
            mode,
            clip,
            start,
            duration,
            effect: effect.clone(),
            params: params.clone(),
            source: source.clone(),
            summary: summary.clone(),
            samples: samples.clone(),
            enabled: true,
            previous,
            follows_clip: false,
        };
        component.check().map_err(|e| bad("components.add", e))?;
        if q.track(track).is_none_or(|t| t.kind != TrackKind::Video) {
            return Err(bad("components.add", "components apply to video clips"));
        }
        q.components.push(component);
        Ok((id, written))
    })?;
    Ok(json!({"id": id.0, "keyframes": written}))
}

/// After a razor, a component that follows its clip continues on the new right-hand piece: the
/// piece keeps the effect, so it keeps its record on the lane too.
pub(crate) fn follow_split(q: &mut filmcraft_project::Sequence, new_ids: &[ClipId], ctx: &mut filmcraft_edit::EditCtx) {
    for new_id in new_ids {
        let Some((track_id, right)) = q.find_item(*new_id).map(|(t, i)| (t, i.clone())) else { continue };
        let Some(left) = q.track(track_id).and_then(|t| t.items.iter().find(|i| i.end() == right.start && i.item == right.item && i.id != right.id)) else {
            continue;
        };
        let left_id = left.id;
        let copies: Vec<AppliedComponent> = q
            .components
            .iter()
            .filter(|c| c.follows_clip && c.clip == left_id)
            .map(|c| AppliedComponent { id: ComponentId(0), clip: *new_id, start: right.start, duration: right.duration, ..c.clone() })
            .collect();
        for mut c in copies {
            c.id = ComponentId(ctx.alloc());
            q.components.push(c);
        }
    }
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let id = ComponentId(u64_p(p, "id").ok_or_else(|| bad("components.remove", "need `id`"))?);
    let layer_clip: Option<ClipId> = s.active_sequence().and_then(|q| {
        let c = q.components.iter().find(|c| c.id == id)?;
        let (_, it) = q.find_item(c.clip)?;
        let adjustment = matches!(s.project.item(it.item)?.kind, ItemKind::AdjustmentLayer { .. });
        (c.follows_clip && adjustment).then_some(c.clip)
    });
    let removed = s.edit_sequence("Remove Component", |q, _, st| {
        let index = q.components.iter().position(|c| c.id == id).ok_or_else(|| bad("components.remove", "no such component"))?;
        let component = q.components.remove(index);
        if let Some(clip) = layer_clip {
            for track in q.video_tracks.iter_mut() {
                track.items.retain(|i| i.id != clip);
                track.transitions.retain(|t| t.from != Some(clip) && t.to != Some(clip));
            }
            st.selection.retain(|c| *c != clip);
            return Ok(0usize);
        }
        let mut removed = 0usize;
        if let Some((_, it)) = q.find_item_mut(component.clip) {
            let last = (it.end() - Tick(1)).max(it.start);
            let from = it.source_time_at(component.start.clamp(it.start, last));
            let to = it.source_time_at((component.end() - Tick(1)).clamp(it.start, last));
            let (lo, hi) = (from.min(to), from.max(to));
            if let Some(e) = it.effects.iter_mut().find(|e| e.effect == component.effect) {
                for pid in &component.params {
                    if let Some(prm) = e.param_mut(pid) {
                        let before = prm.keyframes.len();
                        let resting = prm.value_at(lo);
                        prm.keyframes.retain(|k| k.time < lo || k.time > hi);
                        removed = removed.saturating_add(before.saturating_sub(prm.keyframes.len()));
                        if prm.keyframes.is_empty() {
                            prm.value = resting;
                        }
                    }
                }
                for (pid, value) in &component.previous {
                    if let Some(prm) = e.param_mut(pid)
                        && prm.keyframes.is_empty()
                    {
                        prm.value = value.clone();
                    }
                }
            }
        }
        Ok(removed)
    })?;
    Ok(json!({"id": id.0, "keyframes": removed}))
}

fn list(s: &mut Session, _p: &Value) -> Result<Value> {
    let q = s.active_sequence().ok_or(crate::EngineError::NoSequence)?;
    let rows: Vec<Value> = q
        .components
        .iter()
        .map(|c| {
            let present = q.find_item(c.clip).is_some();
            let (lo, hi) = c.scale_range();
            let (start, duration) = c.span(q);
            json!({
                "id": c.id.0,
                "kind": c.kind,
                "name": c.name,
                "mode": c.mode,
                "clip": c.clip.0,
                "clipPresent": present,
                "start": start.0,
                "duration": duration.0,
                "followsClip": c.follows_clip,
                "effect": c.effect,
                "enabled": c.enabled,
                "params": c.params,
                "source": c.source,
                "frames": c.samples.len(),
                "summary": c.summary,
                "maxDisplacementPx": c.max_displacement(),
                "scaleRange": [lo, hi],
                "changes": c.changes().iter().map(|(t, k)| json!({"time": t.0, "kind": k})).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({"components": rows}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Session {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s
    }

    #[test]
    fn add_writes_keyframes_and_record_in_one_step_and_remove_takes_both_away() {
        let mut s = demo();
        let a = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let fd = s.active_sequence().unwrap().settings.frame_rate.frame_duration();
        let keys: Vec<Value> = (0..4).map(|k| json!({"time": (a.start + Tick(fd.0 * k)).0, "value": 100.0 + k as f64})).collect();
        let samples: Vec<Value> = (0..4)
            .map(|k| json!({"tick": fd.0 * k, "dx": k as f64, "dy": 0.0, "scale": 1.0, "rotation": 0.0, "residual": 0.0, "state": if k == 0 { Value::Null } else { json!("moving") }}))
            .collect();
        let before = s.history.undo.len();
        let r = s
            .execute(
                "components.add",
                json!({"clip": a.id.0, "kind": "camera2d", "name": "pan", "mode": "follow", "start": a.start.0, "duration": fd.0 * 4, "keyframes": {"scale": keys}, "samples": samples, "source": {"sequence": "Main Edit", "width": 1920, "height": 1080, "frameRate": 23.976, "startFrame": 0, "endFrame": 3}}),
            )
            .unwrap();
        assert_eq!(r["keyframes"], json!(4));
        let id = r["id"].as_u64().unwrap();
        assert_eq!(s.history.undo.len(), before + 1);
        let q = s.active_sequence().unwrap().clone();
        assert_eq!(q.components.len(), 1);
        assert_eq!(q.components[0].params, vec!["scale".to_string()]);
        assert_eq!(q.find_item(a.id).unwrap().1.effect("motion").unwrap().param("scale").unwrap().keyframes.len(), 4);
        let listed = s.execute("components.list", json!({})).unwrap();
        assert_eq!(listed["components"][0]["frames"], json!(4));
        assert_eq!(listed["components"][0]["changes"][0]["kind"], json!("motionStarts"));
        assert_eq!(listed["components"][0]["maxDisplacementPx"], json!(3.0));
        let saved = serde_json::to_string(&*s.project).unwrap();
        let reloaded: filmcraft_project::Project = serde_json::from_str(&saved).unwrap();
        assert_eq!(reloaded.sequence(s.state.active_sequence.unwrap()).unwrap().components, q.components, "components survive save and load");
        let r = s.execute("components.remove", json!({"id": id})).unwrap();
        assert_eq!(r["keyframes"], json!(4));
        let q = s.active_sequence().unwrap();
        assert!(q.components.is_empty());
        let scale = q.find_item(a.id).unwrap().1.effect("motion").unwrap().param("scale").unwrap().clone();
        assert!(scale.keyframes.is_empty());
        assert_eq!(scale.value.as_f64().unwrap(), 100.0, "the parameter keeps the value it had where the component started");
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.active_sequence().unwrap().components.len(), 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.active_sequence().unwrap().components.is_empty());
        assert!(s.execute("components.remove", json!({"id": 99_999})).is_err());
        assert!(s.execute("components.add", json!({"clip": a.id.0, "kind": "lens", "start": 0, "duration": 1})).is_err());
        assert!(s.execute("components.add", json!({"clip": a.id.0, "start": a.start.0, "duration": 0})).is_err());
        assert!(
            s.execute(
                "components.add",
                json!({"clip": a.id.0, "start": a.start.0, "duration": fd.0, "keyframes": {"scale": [{"time": (a.end() + Tick(1)).0, "value": 1.0}]}})
            )
            .is_err()
        );
        assert!(s.active_sequence().unwrap().components.is_empty(), "refused calls leave nothing behind");
    }

    #[test]
    fn removing_a_component_restores_the_static_values_it_overwrote() {
        let mut s = demo();
        let a = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let fd = s.active_sequence().unwrap().settings.frame_rate.frame_duration();
        s.execute("effects.apply", json!({"clips": [a.id.0], "effect": "lumetri"})).unwrap();
        s.execute("effects.setParam", json!({"clip": a.id.0, "effect": "lumetri", "param": "saturation", "value": 80.0})).unwrap();
        let r = s
            .execute(
                "components.add",
                json!({"clip": a.id.0, "kind": "look", "name": "look", "start": a.start.0, "duration": fd.0 * 2, "effect": "lumetri", "keyframes": {}, "set": {"saturation": 150.0, "look_lut": "lib:none", "creative_on": true}}),
            )
            .unwrap();
        let id = r["id"].as_u64().unwrap();
        let lumetri = |s: &Session| s.active_sequence().unwrap().find_item(a.id).unwrap().1.effect("lumetri").unwrap().clone();
        assert_eq!(lumetri(&s).param("saturation").unwrap().value.as_f64(), Some(150.0));
        assert_eq!(lumetri(&s).param("look_lut").unwrap().value, filmcraft_project::ParamValue::Text("lib:none".into()));
        s.execute("components.remove", json!({"id": id})).unwrap();
        let e = lumetri(&s);
        assert_eq!(e.param("saturation").unwrap().value.as_f64(), Some(80.0), "the value from before the component comes back");
        assert_eq!(e.param("look_lut").unwrap().value, filmcraft_project::ParamValue::Text(String::new()), "the LUT slot is empty again");
    }

    #[test]
    fn bypassing_a_component_switches_its_effect_off_and_back_on() {
        let mut s = demo();
        let a = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let fd = s.active_sequence().unwrap().settings.frame_rate.frame_duration();
        let r = s.execute("components.add", json!({"clip": a.id.0, "name": "pan", "start": a.start.0, "duration": fd.0 * 2})).unwrap();
        let id = r["id"].as_u64().unwrap();
        assert!(s.active_sequence().unwrap().find_item(a.id).unwrap().1.effect("motion").unwrap().enabled);
        let r = s.execute("components.setEnabled", json!({"id": id, "enabled": false})).unwrap();
        assert_eq!(r["effects"], json!(1));
        let q = s.active_sequence().unwrap();
        assert!(!q.components[0].enabled);
        assert!(!q.find_item(a.id).unwrap().1.effect("motion").unwrap().enabled, "bypass switches the component's effect off");
        assert_eq!(s.execute("components.list", json!({})).unwrap()["components"][0]["enabled"], json!(false));
        s.execute("components.setEnabled", json!({"enabled": true})).unwrap();
        let q = s.active_sequence().unwrap();
        assert!(q.components[0].enabled && q.find_item(a.id).unwrap().1.effect("motion").unwrap().enabled);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(!s.active_sequence().unwrap().components[0].enabled, "each toggle is one undo step");
        assert!(s.execute("components.setEnabled", json!({"id": 99_999, "enabled": false})).is_err());
        assert!(s.execute("components.setEnabled", json!({"id": id})).is_err());
    }

    #[test]
    fn a_look_on_a_range_becomes_an_adjustment_layer_that_the_component_follows() {
        let mut s = demo();
        let a = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let fd = s.active_sequence().unwrap().settings.frame_rate.frame_duration();
        let tracks_before = s.active_sequence().unwrap().video_tracks.len();
        let color = json!({"mean": [0.3, 0.02, -0.08], "std": [0.1, 0.03, 0.04], "weight": 1000.0});
        let (start, end) = (a.start + Tick(fd.0 * 4), a.start + Tick(fd.0 * 20));
        let before = s.history.undo.len();
        let r = s
            .execute("components.applyLookRange", json!({"start": start.0, "end": end.0, "color": color, "name": "Night look", "summary": "Look: low-key"}))
            .unwrap();
        assert_eq!(s.history.undo.len(), before + 1, "one undo step");
        let q = s.active_sequence().unwrap().clone();
        let clip = ClipId(r["clip"].as_u64().unwrap());
        let (track, it) = q.find_item(clip).expect("the adjustment clip is in the sequence");
        assert_eq!(it.start, start);
        assert_eq!(it.duration, Tick(fd.0 * 16));
        assert!(matches!(s.project.item(it.item).unwrap().kind, ItemKind::AdjustmentLayer { .. }));
        let placed_on = q.track(track).expect("the clip's track");
        assert!(placed_on.items.iter().filter(|i| i.id != clip).all(|i| !i.range().overlaps(&it.range())), "placed where nothing else is");
        assert!(q.video_tracks.len() >= tracks_before);
        let lumetri = it.effect("lumetri").expect("Lumetri on the adjustment clip");
        assert_eq!(lumetri.param("look_lut").unwrap().value, ParamValue::Text(r["lut"].as_str().unwrap().to_string()));
        assert!(s.project.luts.iter().any(|l| format!("lib:{}", l.id) == r["lut"].as_str().unwrap() && l.text.contains("LUT_3D_SIZE 17")));
        let c = q.components.iter().find(|c| c.clip == clip).expect("a component on the clip");
        assert!(c.follows_clip && c.kind == ComponentKind::Look);
        s.edit_sequence("move", |q, _, _| {
            let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
            it.start += Tick(fd.0 * 10);
            Ok(())
        })
        .unwrap();
        let q = s.active_sequence().unwrap().clone();
        let (now_start, _) = q.components.iter().find(|c| c.clip == clip).unwrap().span(&q);
        assert_eq!(now_start, start + Tick(fd.0 * 10), "the component follows the clip wherever it is");
        let listed = s.execute("components.list", json!({})).unwrap();
        assert_eq!(listed["components"][0]["followsClip"], json!(true));
        let cut = start + Tick(fd.0 * 10) + Tick(fd.0 * 6);
        s.execute("timeline.razor", json!({"time": cut.0, "clip": clip.0})).unwrap();
        let q = s.active_sequence().unwrap().clone();
        let looks: Vec<&AppliedComponent> = q.components.iter().filter(|c| c.kind == ComponentKind::Look).collect();
        assert_eq!(looks.len(), 2, "the razor leaves a look on each piece: {looks:?}");
        let pieces: Vec<(Tick, Tick)> = looks.iter().map(|c| c.span(&q)).collect();
        assert!(pieces.iter().any(|(s0, d)| *s0 + *d == cut) && pieces.iter().any(|(s0, _)| *s0 == cut), "{pieces:?}");
        let right = (*looks.iter().find(|c| c.span(&q).0 == cut).unwrap()).clone();
        s.execute("components.remove", json!({"id": right.id.0})).unwrap();
        let q = s.active_sequence().unwrap().clone();
        assert!(q.find_item(right.clip).is_none(), "removing a layer's look removes the layer clip too");
        assert_eq!(q.components.iter().filter(|c| c.kind == ComponentKind::Look).count(), 1);
        assert!(
            s.execute("components.applyLookRange", json!({"start": end.0, "end": start.0, "color": {"mean":[0.0,0.0,0.0],"std":[0.1,0.1,0.1],"weight":1.0}}))
                .is_err()
        );
        assert!(s.execute("components.applyLookRange", json!({"start": start.0, "end": end.0})).is_err(), "colour statistics are required");
    }

    #[test]
    fn deleting_the_clip_prunes_its_components() {
        let mut s = demo();
        let a = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
        let fd = s.active_sequence().unwrap().settings.frame_rate.frame_duration();
        s.execute("components.add", json!({"clip": a.id.0, "name": "pan", "start": a.start.0, "duration": fd.0 * 2})).unwrap();
        assert_eq!(s.active_sequence().unwrap().components.len(), 1);
        s.execute("timeline.select", json!({"clips": [a.id.0]})).unwrap();
        s.execute("edit.clear", json!({})).unwrap();
        assert!(s.active_sequence().unwrap().find_item(a.id).is_none());
        assert!(s.active_sequence().unwrap().components.is_empty());
    }
}
