//! M8-P13 — serde round-trip + JSON-shape locks for the bridge wire types.
//! Catches accidental rename / field-reorder before M9's wire code lands.

#![allow(clippy::unwrap_used)]

use bridge::{
    AuthChallenge, AuthResponse, BridgeRequest, BridgeResponse, BridgeWireError, Capabilities,
    ClientHello, ServerHello,
};

fn roundtrip<T>(v: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let json = serde_json::to_string(v).unwrap();
    serde_json::from_str(&json).unwrap()
}

#[test]
fn capabilities_default_shape() {
    let c = Capabilities::default();
    let json = serde_json::to_string(&c).unwrap();
    assert_eq!(
        json,
        r#"{"supports_streaming":true,"supports_tools":true,"supports_skills":true,"supports_commands":true}"#
    );
}

#[test]
fn client_hello_roundtrip() {
    let h = ClientHello {
        protocol_version: "0.1.0".into(),
        client_name: "lingxi-ios/0.9.0".into(),
        capabilities: Capabilities::default(),
    };
    assert_eq!(roundtrip(&h), h);
}

#[test]
fn server_hello_roundtrip() {
    let h = ServerHello {
        protocol_version: "0.1.0".into(),
        server_name: "lingxi-engine-desktop/0.9.0".into(),
        capabilities: Capabilities::default(),
    };
    assert_eq!(roundtrip(&h), h);
}

#[test]
fn bridge_request_roundtrip() {
    let r = BridgeRequest {
        id: 7,
        method: "run_turn".into(),
        params: serde_json::json!({ "text": "hi" }),
    };
    assert_eq!(roundtrip(&r), r);
}

#[test]
fn bridge_response_roundtrip() {
    let ok = BridgeResponse {
        id: 7,
        result: Some(serde_json::json!({ "ok": true })),
        error: None,
    };
    assert_eq!(roundtrip(&ok), ok);
    let err = BridgeResponse {
        id: 8,
        result: None,
        error: Some(BridgeWireError {
            code: -32000,
            message: "boom".into(),
        }),
    };
    assert_eq!(roundtrip(&err), err);
}

#[test]
fn auth_roundtrip() {
    let c = AuthChallenge {
        nonce: "abc".into(),
    };
    assert_eq!(roundtrip(&c), c);
    let r = AuthResponse {
        token: "xyz".into(),
    };
    assert_eq!(roundtrip(&r), r);
}
