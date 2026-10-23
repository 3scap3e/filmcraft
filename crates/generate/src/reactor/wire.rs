//! The `reactor_wire.v1` protobuf messages of Reactor's `data` and `control` channels (the
//! schema is published by reactor-team/reactor-runtime, Apache-2.0; the field numbers here follow
//! wire version 1.20261013.11), and the JSON ⇄ `google.protobuf.Struct` bridge commands use.

use std::collections::BTreeMap;

use prost::Message;
use prost_types::value::Kind;
use prost_types::{ListValue, Struct, Value as PbValue};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum MessageKind {
    Unspecified = 0,
    Request = 1,
    Response = 2,
    Notification = 3,
}

#[derive(Clone, PartialEq, Message)]
pub struct Error {
    #[prost(string, tag = "1")]
    pub code: String,
    #[prost(string, tag = "2")]
    pub message: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct UploadReference {
    #[prost(string, tag = "1")]
    pub upload_id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub mime_type: String,
    #[prost(int64, tag = "4")]
    pub size: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Command {
    #[prost(string, tag = "1")]
    pub r#type: String,
    #[prost(message, optional, tag = "2")]
    pub data: Option<Struct>,
    #[prost(btree_map = "string, message", tag = "3")]
    pub uploads: BTreeMap<String, UploadReference>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ModelMessage {
    #[prost(string, tag = "1")]
    pub r#type: String,
    #[prost(message, optional, tag = "2")]
    pub data: Option<Struct>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DataClientMessage {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(enumeration = "MessageKind", tag = "2")]
    pub kind: i32,
    #[prost(oneof = "data_client_payload::Payload", tags = "10, 40")]
    pub payload: Option<data_client_payload::Payload>,
}

pub mod data_client_payload {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Payload {
        #[prost(message, tag = "10")]
        Command(super::Command),
        #[prost(message, tag = "40")]
        Error(super::Error),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct DataServerMessage {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(enumeration = "MessageKind", tag = "2")]
    pub kind: i32,
    #[prost(oneof = "data_server_payload::Payload", tags = "10, 40")]
    pub payload: Option<data_server_payload::Payload>,
}

pub mod data_server_payload {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Payload {
        #[prost(message, tag = "10")]
        Message(super::ModelMessage),
        #[prost(message, tag = "40")]
        Error(super::Error),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct Ping {}

#[derive(Clone, PartialEq, Message)]
pub struct RequestSchema {}

#[derive(Clone, PartialEq, Message)]
pub struct FileUploaded {
    #[prost(string, tag = "1")]
    pub upload_id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub mime_type: String,
    #[prost(int64, tag = "4")]
    pub size: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestClip {
    #[prost(double, tag = "1")]
    pub duration_seconds: f64,
}

#[derive(Clone, PartialEq, Message)]
pub struct RequestRecording {}

#[derive(Clone, PartialEq, Message)]
pub struct TrackName {
    #[prost(string, tag = "1")]
    pub name: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ControlClientMessage {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(enumeration = "MessageKind", tag = "2")]
    pub kind: i32,
    #[prost(oneof = "control_client_payload::Payload", tags = "10, 11, 12, 13, 14, 15, 16, 17, 18, 40")]
    pub payload: Option<control_client_payload::Payload>,
}

pub mod control_client_payload {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Payload {
        #[prost(message, tag = "10")]
        Ping(super::Ping),
        #[prost(message, tag = "11")]
        RequestSchema(super::RequestSchema),
        #[prost(message, tag = "12")]
        FileUploaded(super::FileUploaded),
        #[prost(message, tag = "13")]
        RequestClip(super::RequestClip),
        #[prost(message, tag = "14")]
        RequestRecording(super::RequestRecording),
        #[prost(message, tag = "15")]
        PublishTrack(super::TrackName),
        #[prost(message, tag = "16")]
        PauseTrack(super::TrackName),
        #[prost(message, tag = "17")]
        ResumeTrack(super::TrackName),
        #[prost(message, tag = "18")]
        UnpublishTrack(super::TrackName),
        #[prost(message, tag = "40")]
        Error(super::Error),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct ModelSchema {
    #[prost(message, optional, tag = "1")]
    pub openapi: Option<Struct>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ClipReady {
    #[prost(string, tag = "1")]
    pub session_id: String,
    #[prost(string, tag = "2")]
    pub kind: String,
    #[prost(double, tag = "3")]
    pub start_marker: f64,
    #[prost(double, tag = "4")]
    pub end_marker: f64,
    #[prost(double, tag = "5")]
    pub now_marker: f64,
    #[prost(int64, tag = "6")]
    pub predicted_ready_at_ms: i64,
    #[prost(string, tag = "7")]
    pub playlist_url: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ClipFailed {
    #[prost(string, tag = "1")]
    pub reason: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct Moderation {
    #[prost(string, tag = "1")]
    pub action: String,
    #[prost(string, tag = "2")]
    pub input_kind: String,
    #[prost(string, tag = "3")]
    pub command: String,
    #[prost(string, repeated, tag = "4")]
    pub categories: Vec<String>,
    #[prost(string, tag = "5")]
    pub message: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct SessionEnded {
    #[prost(string, tag = "1")]
    pub reason: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct ControlServerMessage {
    #[prost(string, tag = "1")]
    pub request_id: String,
    #[prost(enumeration = "MessageKind", tag = "2")]
    pub kind: i32,
    #[prost(oneof = "control_server_payload::Payload", tags = "10, 11, 12, 13, 14, 15, 40")]
    pub payload: Option<control_server_payload::Payload>,
}

pub mod control_server_payload {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Payload {
        #[prost(message, tag = "10")]
        ModelSchema(super::ModelSchema),
        #[prost(message, tag = "11")]
        ClipReady(super::ClipReady),
        #[prost(message, tag = "12")]
        ClipFailed(super::ClipFailed),
        #[prost(message, tag = "13")]
        Moderation(super::Moderation),
        #[prost(message, tag = "14")]
        PublishTrack(super::TrackName),
        #[prost(message, tag = "15")]
        SessionEnded(super::SessionEnded),
        #[prost(message, tag = "40")]
        Error(super::Error),
    }
}

/// Deepest JSON nesting converted (a hostile payload cannot recurse without bound).
const MAX_DEPTH: usize = 64;

/// A JSON object as a protobuf `Struct`; anything else is an error.
pub fn struct_of(v: &Value) -> Option<Struct> {
    let Value::Object(map) = v else { return None };
    let mut fields = BTreeMap::new();
    for (k, v) in map {
        fields.insert(k.clone(), value_of(v, 0)?);
    }
    Some(Struct { fields })
}

fn value_of(v: &Value, depth: usize) -> Option<PbValue> {
    if depth > MAX_DEPTH {
        return None;
    }
    let kind = match v {
        Value::Null => Kind::NullValue(0),
        Value::Bool(b) => Kind::BoolValue(*b),
        Value::Number(n) => Kind::NumberValue(n.as_f64()?),
        Value::String(s) => Kind::StringValue(s.clone()),
        Value::Array(items) => {
            let mut values = Vec::with_capacity(items.len());
            for it in items {
                values.push(value_of(it, depth + 1)?);
            }
            Kind::ListValue(ListValue { values })
        }
        Value::Object(map) => {
            let mut fields = BTreeMap::new();
            for (k, v) in map {
                fields.insert(k.clone(), value_of(v, depth + 1)?);
            }
            Kind::StructValue(Struct { fields })
        }
    };
    Some(PbValue { kind: Some(kind) })
}

/// A protobuf `Struct` as JSON (`None` becomes an empty object).
pub fn json_of(s: Option<&Struct>) -> Value {
    match s {
        Some(s) => json_struct(s, 0),
        None => Value::Object(Default::default()),
    }
}

fn json_struct(s: &Struct, depth: usize) -> Value {
    let mut out = serde_json::Map::new();
    for (k, v) in &s.fields {
        out.insert(k.clone(), json_value(v, depth + 1));
    }
    Value::Object(out)
}

fn json_value(v: &PbValue, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return Value::Null;
    }
    match &v.kind {
        None | Some(Kind::NullValue(_)) => Value::Null,
        Some(Kind::BoolValue(b)) => Value::Bool(*b),
        Some(Kind::NumberValue(n)) if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e15 => Value::Number((*n as i64).into()),
        Some(Kind::NumberValue(n)) => serde_json::Number::from_f64(*n).map(Value::Number).unwrap_or(Value::Null),
        Some(Kind::StringValue(s)) => Value::String(s.clone()),
        Some(Kind::ListValue(l)) => Value::Array(l.values.iter().map(|x| json_value(x, depth + 1)).collect()),
        Some(Kind::StructValue(s)) => json_struct(s, depth + 1),
    }
}

/// Encode a message for the wire.
pub fn encode<M: Message>(m: &M) -> Vec<u8> {
    m.encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn commands_round_trip_through_protobuf_and_struct() {
        let data = json!({"prompt": "a cat", "seconds": 8.5, "seed": null, "tags": ["x", 1, true], "nested": {"a": {"b": 2}}});
        let cmd = Command { r#type: "enqueue".into(), data: struct_of(&data), uploads: Default::default() };
        let msg = DataClientMessage {
            request_id: "data_1".into(),
            kind: MessageKind::Request as i32,
            payload: Some(data_client_payload::Payload::Command(cmd.clone())),
        };
        let bytes = encode(&msg);
        let back = DataClientMessage::decode(&bytes[..]).unwrap();
        assert_eq!(back.request_id, "data_1");
        assert_eq!(back.kind, 1);
        let Some(data_client_payload::Payload::Command(c)) = back.payload else { panic!("not a command") };
        assert_eq!(c.r#type, "enqueue");
        assert_eq!(json_of(c.data.as_ref()), data);
        assert!(struct_of(&json!([1, 2])).is_none(), "a command's data must be an object");
    }

    #[test]
    fn control_messages_decode_with_their_oneof_tags() {
        let ready = ControlServerMessage {
            request_id: "ctrl_3".into(),
            kind: MessageKind::Response as i32,
            payload: Some(control_server_payload::Payload::ClipReady(ClipReady {
                session_id: "s".into(),
                kind: "snap".into(),
                start_marker: 1.0,
                end_marker: 9.0,
                now_marker: 9.5,
                predicted_ready_at_ms: 1700000000000,
                playlist_url: "https://x/y.m3u8".into(),
            })),
        };
        let back = ControlServerMessage::decode(&encode(&ready)[..]).unwrap();
        assert_eq!(back, ready);
        let clip = ControlClientMessage {
            request_id: "ctrl_1".into(),
            kind: MessageKind::Request as i32,
            payload: Some(control_client_payload::Payload::RequestClip(RequestClip { duration_seconds: 10.0 })),
        };
        let bytes = encode(&clip);
        assert_eq!(bytes[0], 0x0a, "field 1 (request_id), wire type 2");
        assert!(bytes.windows(2).any(|w| w == [0x6a, 0x09]), "field 13 (request_clip) wire type 2 with a 9-byte body");
        let deep = (0..70).fold(json!(1), |acc, _| json!({"k": acc}));
        assert!(struct_of(&json!({"d": deep})).is_none(), "nesting is bounded");
    }
}
