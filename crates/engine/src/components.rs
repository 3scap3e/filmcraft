//! Reverse-engineering components applied to clips (the timeline's RE lane): `components.add`
//! writes a component's keyframes and its record in one undo step, `components.remove` takes
//! both away again, `components.list` reports them with their change marks.
use filmcraft_project::{AppliedComponent, ComponentId, ComponentKind, ComponentMode, ComponentSample, ComponentSource, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, clip_p, has_seq, keyframe_request, str_p, u64_p, write_keyframes};
use crate::{Result, Session};

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
        spec("components.list", "List Components", r#"{}"#, always, list, false),
    ]
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = clip_p(p, "clip").ok_or_else(|| bad("components.add", "need `clip`"))?;
    let kind = match str_p(p, "kind").unwrap_or("camera2d") {
        "camera2d" => ComponentKind::Camera2d,
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
    let (id, written) = s.edit_sequence("Apply Component", |q, ctx, _| {
        let (track, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
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
            samples: samples.clone(),
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

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let id = ComponentId(u64_p(p, "id").ok_or_else(|| bad("components.remove", "need `id`"))?);
    let removed = s.edit_sequence("Remove Component", |q, _, _| {
        let index = q.components.iter().position(|c| c.id == id).ok_or_else(|| bad("components.remove", "no such component"))?;
        let component = q.components.remove(index);
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
            json!({
                "id": c.id.0,
                "kind": c.kind,
                "name": c.name,
                "mode": c.mode,
                "clip": c.clip.0,
                "clipPresent": present,
                "start": c.start.0,
                "duration": c.duration.0,
                "effect": c.effect,
                "params": c.params,
                "source": c.source,
                "frames": c.samples.len(),
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
