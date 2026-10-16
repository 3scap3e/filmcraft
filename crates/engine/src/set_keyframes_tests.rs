use crate::Session;
use filmcraft_project::{ParamValue, TrackItem};
use filmcraft_time::Tick;
use serde_json::json;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn clip(s: &Session, id: u64) -> TrackItem {
    s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(id)).unwrap().1.clone()
}

fn motion_at(s: &Session, it: &TrackItem, t: Tick) -> filmcraft_geom::Affine {
    let seq = s.active_sequence().unwrap();
    let src = s.project.source_size(it.item).unwrap();
    filmcraft_render::motion_matrix(seq, it, src, s.project.source_par(it.item), it.effect_time_at(t))
}

#[test]
fn set_keyframes_writes_every_parameter_in_one_undo_step_and_renders_the_similarity() {
    let mut s = demo();
    let a = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
    let fd = s.active_sequence().unwrap().settings.frame_rate.frame_duration();
    let src = s.project.source_size(a.item).unwrap();
    let anchor = [src.0 as f64 / 2.0, src.1 as f64 / 2.0];
    let times: Vec<Tick> = (0..3).map(|k| a.start + Tick(fd.0 * k)).collect();
    let frames: Vec<([f64; 2], f64, f64)> = vec![([960.0, 540.0], 100.0, 0.0), ([963.0, 538.0], 101.0, 0.5), ([966.0, 536.0], 102.0, 1.0)];
    let keys = |index: usize| -> Vec<serde_json::Value> {
        times.iter().zip(&frames).map(|(t, f)| json!({"time": t.0, "value": match index { 0 => json!(f.0), 1 => json!(f.1), _ => json!(f.2) }})).collect()
    };
    let before = s.history.undo.len();
    let r = s
        .execute(
            "effects.setKeyframes",
            json!({"clip": a.id.0, "effect": "motion", "params": {"position": keys(0), "scale": keys(1), "rotation": keys(2)}, "set": {"uniform_scale": true}}),
        )
        .unwrap();
    assert_eq!(r["keyframes"], json!(9));
    assert_eq!(s.history.undo.len(), before + 1, "one undo step for every keyframe");
    let it = clip(&s, a.id.0);
    let motion = it.effect("motion").unwrap();
    assert!(motion.enabled);
    for (pid, n) in [("position", 3), ("scale", 3), ("rotation", 3)] {
        assert_eq!(motion.param(pid).unwrap().keyframes.len(), n, "{pid}");
    }
    assert_eq!(motion.param("uniform_scale").unwrap().value, ParamValue::Bool(true));
    let fit = if it.scale_to_frame { (1920.0 / src.0 as f64).min(1080.0 / src.1 as f64) } else { 1.0 };
    for (t, (position, scale, rotation)) in times.iter().zip(&frames) {
        let m = motion_at(&s, &it, *t);
        let s_eff = scale / 100.0 * fit;
        let (sin, cos) = rotation.to_radians().sin_cos();
        let (a, b) = (s_eff * cos, s_eff * sin);
        let expected = [a, b, position[0] - (a * anchor[0] - b * anchor[1]), position[1] - (b * anchor[0] + a * anchor[1])];
        assert!((m.a - expected[0]).abs() < 1e-9 && (m.b - expected[1]).abs() < 1e-9, "{m:?} vs {expected:?}");
        assert!((m.e - expected[2]).abs() < 1e-6 && (m.f - expected[3]).abs() < 1e-6, "{m:?} vs {expected:?}");
    }
    let mid = times[0] + Tick(fd.0 / 2);
    let m = motion_at(&s, &it, mid);
    assert!(((m.a * m.a + m.b * m.b).sqrt() - 1.005 * fit).abs() < 1e-6, "linear interpolation between frames: {m:?}");
    s.execute("edit.undo", json!({})).unwrap();
    let it = clip(&s, a.id.0);
    assert!(it.effect("motion").is_none_or(|m| m.param("position").is_none_or(|p| p.keyframes.is_empty())), "undo removes every keyframe at once");
}

#[test]
fn set_keyframes_replaces_the_span_and_refuses_bad_input() {
    let mut s = demo();
    let a = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
    let fd = s.active_sequence().unwrap().settings.frame_rate.frame_duration();
    let t = |k: i64| a.start + Tick(fd.0 * k);
    s.execute("effects.setParam", json!({"clip": a.id.0, "effect": "motion", "param": "scale", "value": 50.0, "keyframe": true, "time": t(1).0})).unwrap();
    s.execute("effects.setParam", json!({"clip": a.id.0, "effect": "motion", "param": "scale", "value": 60.0, "keyframe": true, "time": t(5).0})).unwrap();
    s.execute("effects.setParam", json!({"clip": a.id.0, "effect": "motion", "param": "scale", "value": 70.0, "keyframe": true, "time": t(9).0})).unwrap();
    s.execute(
        "effects.setKeyframes",
        json!({"clip": a.id.0, "effect": "motion", "params": {"scale": [{"time": t(2).0, "value": 80.0}, {"time": t(6).0, "value": 90.0}]}}),
    )
    .unwrap();
    let scale: Vec<f64> = clip(&s, a.id.0).effect("motion").unwrap().param("scale").unwrap().keyframes.iter().map(|k| k.value.as_f64().unwrap()).collect();
    assert_eq!(scale, vec![50.0, 80.0, 90.0, 70.0], "keyframes inside the written span are replaced, the others kept");
    s.execute(
        "effects.setKeyframes",
        json!({"clip": a.id.0, "effect": "motion", "params": {"scale": [{"time": t(3).0, "value": 85.0}]}, "replace": false, "interpolation": "hold"}),
    )
    .unwrap();
    let keys = clip(&s, a.id.0).effect("motion").unwrap().param("scale").unwrap().keyframes.clone();
    assert_eq!(keys.len(), 5);
    assert_eq!(keys[2].interp, filmcraft_project::Interpolation::Hold);
    let bad = |params: serde_json::Value| s.execute("effects.setKeyframes", params).is_err();
    let mut s2 = demo();
    let b = s2.active_sequence().unwrap().video_tracks[0].items[0].clone();
    let t2 = |k: i64| b.start + Tick(fd.0 * k);
    assert!(
        s2.execute("effects.setKeyframes", json!({"clip": b.id.0, "effect": "motion", "params": {"scale": [{"time": (b.end() + Tick(1)).0, "value": 1.0}]}}))
            .is_err()
    );
    assert!(s2.execute("effects.setKeyframes", json!({"clip": b.id.0, "effect": "motion", "params": {"scale": [{"time": t2(1).0, "value": "x"}]}})).is_err());
    assert!(s2.execute("effects.setKeyframes", json!({"clip": b.id.0, "effect": "motion", "params": {"nope": [{"time": t2(1).0, "value": 1.0}]}})).is_err());
    assert!(
        s2.execute("effects.setKeyframes", json!({"clip": b.id.0, "effect": "motion", "params": {"uniform_scale": [{"time": t2(1).0, "value": true}]}}))
            .is_err()
    );
    assert!(s2.execute("effects.setKeyframes", json!({"clip": b.id.0, "effect": "motion", "params": {"scale": [{"value": 1.0}]}})).is_err());
    assert!(
        s2.execute(
            "effects.setKeyframes",
            json!({"clip": b.id.0, "effect": "motion", "params": {"scale": [{"time": t2(1).0, "value": 1.0}]}, "interpolation": "bezier"})
        )
        .is_err()
    );
    assert!(s2.execute("effects.setKeyframes", json!({"clip": b.id.0, "effect": "nope", "params": {}})).is_err());
    assert!(s2.execute("effects.setKeyframes", json!({"clip": b.id.0, "effect": "motion"})).is_err());
    assert!(clip(&s2, b.id.0).effect("motion").is_none_or(|m| m.param("scale").is_none_or(|p| p.keyframes.is_empty())), "a refused call changes nothing");
    let _ = bad;
}
