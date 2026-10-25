//! The cross-language vectors of `packages/bridge-client/test/fixtures/remote`.

mod support;

use remote_control_client::encoding::{b64u_decode, b64u_encode, sha256};
use remote_control_client::framing::{encode_message, Reassembler};
use remote_control_client::keys::{
    hkdf_sha256, id_from_key, verify_signature, EphemeralKey, IdentityKey, SharedSecret,
};
use remote_control_client::pairing::pairing_code;
use remote_control_client::signaling::{
    answer_signing_input, offer_signing_input, parse_answer_record, parse_offer_record,
    sign_answer, sign_offer, verify_answer, verify_offer, SigningContext, VerifyAnswerOptions,
    VerifyFailure,
};
use remote_control_client::tunnel_crypto::{
    derive_tunnel_keys, open_frame, pack_fragments, seal_frame, tunnel_aad, tunnel_nonce,
    unpack_fragments, TunnelDir, TunnelKey,
};
use serde_json::Value;
use support::{fixture, hex_decode, hex_encode, sha256_hex, x25519_ref};

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_else(|| panic!("missing {key}"))
}

fn dir_of(name: &str) -> TunnelDir {
    match name {
        "up" => TunnelDir::Up,
        "down" => TunnelDir::Down,
        other => panic!("dir {other}"),
    }
}

// --- identity and signing ---------------------------------------------------

#[test]
fn rfc8032_signature_and_ids() {
    let fx = fixture("signing");
    let t = &fx["rfc8032_test1"];
    let key = IdentityKey::from_seed(&hex_decode(s(t, "seed_hex"))).unwrap();
    assert_eq!(hex_encode(&key.public_key()), s(t, "public_hex"));
    assert_eq!(hex_encode(&key.sign_bytes(b"")), s(t, "signature_hex"));
    assert_eq!(key.id(), s(t, "id"));
    assert!(verify_signature(
        &key.public_key(),
        &hex_decode(s(t, "signature_hex")),
        b""
    ));
    for name in ["host", "device", "device2"] {
        let k = &fx["keys"][name];
        let key = IdentityKey::from_seed(&hex_decode(s(k, "seed_hex"))).unwrap();
        assert_eq!(hex_encode(&key.public_key()), s(k, "public_hex"), "{name}");
        assert_eq!(key.public_key_b64u(), s(k, "public_b64u"), "{name}");
        assert_eq!(key.id(), s(k, "id"), "{name}");
        assert_eq!(id_from_key(&key.public_key()), s(k, "id"));
    }
}

#[test]
fn key_export_and_import_round_trip() {
    let key = IdentityKey::generate();
    let again = IdentityKey::from_pkcs8(&key.to_pkcs8()).unwrap();
    assert_eq!(again.public_key(), key.public_key());
    let again = IdentityKey::from_seed(key.seed().as_ref()).unwrap();
    assert_eq!(again.public_key(), key.public_key());
    assert!(IdentityKey::from_pkcs8(&[0u8; 48]).is_err());
    assert!(IdentityKey::from_seed(&[0u8; 31]).is_err());
    let debug = format!("{key:?}");
    assert!(!debug.contains(&hex_encode(key.seed().as_ref())));
    assert!(debug.contains(&key.id()));
}

fn host_key(fx: &Value) -> IdentityKey {
    IdentityKey::from_seed(&hex_decode(s(&fx["keys"]["host"], "seed_hex"))).unwrap()
}

fn device_keys(fx: &Value) -> [IdentityKey; 2] {
    ["device", "device2"]
        .map(|n| IdentityKey::from_seed(&hex_decode(s(&fx["keys"][n], "seed_hex"))).unwrap())
}

fn ctx(entry: &Value) -> SigningContext {
    SigningContext {
        uid: s(entry, "uid").into(),
        host_id: s(entry, "host_id").into(),
        conn_id: s(entry, "conn_id").into(),
    }
}

#[tokio::test]
async fn offers_match_the_fixture_byte_for_byte() {
    let fx = fixture("signing");
    let devices = device_keys(&fx);
    let now = fx["now_for_verification"].as_u64().unwrap();
    for offer in fx["offers"].as_array().unwrap() {
        let name = s(offer, "name");
        let record = &offer["record"];
        let parsed = parse_offer_record(record).unwrap_or_else(|e| panic!("{name}: {e}"));
        let ctx = ctx(offer);
        assert_eq!(
            offer_signing_input(&ctx, &parsed.draft).unwrap(),
            s(offer, "signing_input"),
            "{name}"
        );
        let signer = devices
            .iter()
            .find(|k| k.id() == parsed.draft.device.id)
            .expect("known device");
        let signed = sign_offer(&ctx, signer, parsed.draft.clone())
            .await
            .unwrap();
        assert_eq!(
            signed.sig,
            s(record, "sig"),
            "{name}: deterministic signature"
        );
        let verified =
            verify_offer(&ctx, record, now, None).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(verified, parsed);
        assert_eq!(verified.to_value()["sdp"], record["sdp"]);
    }
}

#[test]
fn offer_verification_failures() {
    let fx = fixture("signing");
    let now = fx["now_for_verification"].as_u64().unwrap();
    let offer = &fx["offers"][0];
    let ctx_ok = ctx(offer);
    let record = offer["record"].clone();

    let reason = |record: &Value, ctx: &SigningContext, now: u64| {
        verify_offer(ctx, record, now, None).unwrap_err().reason
    };

    let mut other_conn = ctx_ok.clone();
    other_conn.conn_id = "xxxxxxxxxxxxxxxxxxxxxx".into();
    assert_eq!(
        reason(&record, &other_conn, now),
        VerifyFailure::BadSignature
    );
    let mut other_uid = ctx_ok.clone();
    other_uid.uid = "someone-else".into();
    assert_eq!(
        reason(&record, &other_uid, now),
        VerifyFailure::BadSignature
    );

    let mut tampered = record.clone();
    tampered["sdp"] = Value::String("v=0\r\n".into());
    assert_eq!(reason(&tampered, &ctx_ok, now), VerifyFailure::BadSignature);
    let mut tampered = record.clone();
    tampered["device"]["name"] = Value::String("Mallory".into());
    assert_eq!(reason(&tampered, &ctx_ok, now), VerifyFailure::BadSignature);

    let mut wrong_id = record.clone();
    wrong_id["device"]["id"] = Value::String("AAAAAAAAAAAAAAAAAAAAAA".into());
    assert_eq!(reason(&wrong_id, &ctx_ok, now), VerifyFailure::IdMismatch);

    assert_eq!(
        reason(&record, &ctx_ok, now + 5 * 60_000 + 1),
        VerifyFailure::Expired
    );
    assert_eq!(
        reason(&record, &ctx_ok, now - 5 * 60_000 - 1),
        VerifyFailure::Expired
    );
    assert!(verify_offer(&ctx_ok, &record, now + 5 * 60_000, None).is_ok());

    for bad in [
        Value::Null,
        Value::String("x".into()),
        serde_json::json!({}),
        {
            let mut r = record.clone();
            r["v"] = 2.into();
            r
        },
        {
            let mut r = record.clone();
            r["transport"] = "tunnel".into(); // webrtc offer fields with a tunnel transport
            r
        },
        {
            let mut r = record.clone();
            r["eph"] = "short".into();
            r
        },
        {
            let mut r = record.clone();
            r["device"]["platform"] = "windows".into();
            r
        },
        {
            let mut r = record.clone();
            r["device"]["name"] = "line\nbreak".into();
            r
        },
        {
            let mut r = record.clone();
            r["sdp"] = "a".repeat(16385).into();
            r
        },
    ] {
        assert_eq!(reason(&bad, &ctx_ok, now), VerifyFailure::BadShape, "{bad}");
    }
}

#[tokio::test]
async fn answers_match_the_fixture_byte_for_byte() {
    let fx = fixture("signing");
    let host = host_key(&fx);
    for answer in fx["answers"].as_array().unwrap() {
        let name = s(answer, "name");
        let record = &answer["record"];
        let parsed = parse_answer_record(record).unwrap_or_else(|e| panic!("{name}: {e}"));
        let ctx = ctx(answer);
        let offer_nonce = s(answer, "offer_nonce");
        assert_eq!(
            answer_signing_input(&ctx, offer_nonce, &parsed.draft).unwrap(),
            s(answer, "signing_input"),
            "{name}"
        );
        let signed = sign_answer(&ctx, &host, offer_nonce, parsed.draft.clone())
            .await
            .unwrap();
        assert_eq!(signed.sig, s(record, "sig"), "{name}");
        let host_key_b64 = host.public_key_b64u();
        let options = VerifyAnswerOptions {
            ctx: &ctx,
            record,
            offer_nonce,
            expected_host_key: &host_key_b64,
            now_ms: 0,
            max_age_ms: None,
        };
        assert_eq!(verify_answer(&options).unwrap(), parsed, "{name}");

        // Wrong offer nonce: the signature binds the answer to one offer.
        let wrong_nonce = VerifyAnswerOptions {
            offer_nonce: "AAAAAAAAAAAAAAAAAAAAAA",
            ..options.clone()
        };
        assert_eq!(
            verify_answer(&wrong_nonce).unwrap_err().reason,
            VerifyFailure::BadSignature
        );
        // Pinned to another key.
        let other = IdentityKey::generate().public_key_b64u();
        let pinned = VerifyAnswerOptions {
            expected_host_key: &other,
            ..options.clone()
        };
        assert_eq!(
            verify_answer(&pinned).unwrap_err().reason,
            VerifyFailure::KeyMismatch
        );
        // Different host id dialled.
        let mut other_ctx = ctx.clone();
        other_ctx.host_id = "AAAAAAAAAAAAAAAAAAAAAA".into();
        let dialled = VerifyAnswerOptions {
            ctx: &other_ctx,
            ..options.clone()
        };
        assert_eq!(
            verify_answer(&dialled).unwrap_err().reason,
            VerifyFailure::IdMismatch
        );
        // Age check only when asked for.
        let aged = VerifyAnswerOptions {
            now_ms: 10 * 60_000_000_000,
            max_age_ms: Some(60_000),
            ..options.clone()
        };
        assert_eq!(
            verify_answer(&aged).unwrap_err().reason,
            VerifyFailure::Expired
        );
    }
}

#[test]
fn answer_shape_rules() {
    let fx = fixture("signing");
    let accepted = fx["answers"][0]["record"].clone();
    let tunnel = fx["answers"][1]["record"].clone();
    let rejected = fx["answers"][2]["record"].clone();
    assert!(parse_answer_record(&accepted).is_ok());
    assert!(parse_answer_record(&tunnel).is_ok());
    assert!(parse_answer_record(&rejected).is_ok());
    let mut r = accepted.clone();
    r.as_object_mut().unwrap().remove("sdp");
    assert!(
        parse_answer_record(&r).is_err(),
        "webrtc answer needs an sdp"
    );
    let mut r = tunnel.clone();
    r.as_object_mut().unwrap().remove("eph");
    assert!(
        parse_answer_record(&r).is_err(),
        "tunnel answer needs an eph"
    );
    let mut r = accepted.clone();
    r["reason"] = "revoked".into();
    assert!(
        parse_answer_record(&r).is_err(),
        "reason only on rejections"
    );
    let mut r = rejected.clone();
    r["reason"] = "mystery".into();
    assert!(parse_answer_record(&r).is_err());
    let mut r = accepted;
    r.as_object_mut().unwrap().remove("transport");
    assert!(parse_answer_record(&r).is_err());
}

// --- pairing ----------------------------------------------------------------

#[test]
fn pairing_codes() {
    let fx = fixture("pairing");
    for v in fx["vectors"].as_array().unwrap() {
        let name = s(v, "name");
        let code = pairing_code(
            &hex_decode(s(v, "host_key_hex")),
            &hex_decode(s(v, "device_key_hex")),
            &hex_decode(s(v, "offer_nonce_hex")),
            &hex_decode(s(v, "answer_nonce_hex")),
        )
        .unwrap();
        assert_eq!(code, s(v, "code"), "{name}");
    }
    assert!(pairing_code(&[0; 31], &[0; 32], &[0; 16], &[0; 16]).is_err());
    assert!(pairing_code(&[0; 32], &[0; 32], &[0; 15], &[0; 16]).is_err());
}

// --- framing ----------------------------------------------------------------

fn message_text(spec: &Value) -> String {
    if let Some(text) = spec.get("text") {
        return text.as_str().unwrap().to_owned();
    }
    let unit = s(spec, "unit");
    let count = spec["count"].as_u64().unwrap() as usize;
    format!(
        "{}{}{}",
        spec.get("prefix").and_then(Value::as_str).unwrap_or(""),
        unit.repeat(count),
        spec.get("suffix").and_then(Value::as_str).unwrap_or("")
    )
}

fn feed(reassembler: &mut Reassembler, fragments: &[Vec<u8>]) -> Result<Vec<String>, String> {
    let mut messages = Vec::new();
    for fragment in fragments {
        if let Some(m) = reassembler.push(fragment).map_err(|e| e.0)? {
            messages.push(m);
        }
    }
    Ok(messages)
}

#[test]
fn framing_cases() {
    let fx = fixture("framing");
    assert_eq!(fx["fragment_max_bytes"], 16384);
    assert_eq!(fx["message_max_bytes"], 16_777_216);
    for case in fx["cases"].as_array().unwrap() {
        let name = s(case, "name");
        let text = message_text(&case["message"]);
        assert_eq!(
            text.len(),
            case["message_utf8_length"].as_u64().unwrap() as usize,
            "{name}"
        );
        assert_eq!(
            sha256_hex(text.as_bytes()),
            s(case, "message_sha256_hex"),
            "{name}"
        );
        let max = case["max_fragment_bytes"].as_u64().unwrap() as usize;
        let fragments = encode_message(&text, max).unwrap();
        let expected = case["fragments"].as_array().unwrap();
        assert_eq!(fragments.len(), expected.len(), "{name}");
        for (got, want) in fragments.iter().zip(expected) {
            assert_eq!(got[0] as u64, want["kind"].as_u64().unwrap(), "{name}");
            assert_eq!(got.len() as u64, want["length"].as_u64().unwrap(), "{name}");
            assert_eq!(sha256_hex(got), s(want, "sha256_hex"), "{name}");
            if let Some(hex) = want.get("hex").and_then(Value::as_str) {
                assert_eq!(hex_encode(got), hex, "{name}");
            }
        }
        let mut reassembler = Reassembler::new();
        assert_eq!(
            feed(&mut reassembler, &fragments).unwrap(),
            vec![text],
            "{name}"
        );
        assert_eq!(reassembler.pending_bytes(), 0);
    }
}

fn fragment_from(spec: &Value) -> Vec<u8> {
    if let Some(recipe) = spec.get("recipe") {
        let mut out = vec![recipe["kind"].as_u64().unwrap() as u8];
        let fill = hex_decode(s(recipe, "fill_hex"))[0];
        out.resize(
            1 + recipe["payload_length"].as_u64().unwrap() as usize,
            fill,
        );
        out
    } else {
        hex_decode(s(spec, "hex"))
    }
}

#[test]
fn framing_valid_and_invalid_sequences() {
    let fx = fixture("framing");
    for seq in fx["valid_sequences"].as_array().unwrap() {
        let name = s(seq, "name");
        let fragments: Vec<Vec<u8>> = seq["fragments"]
            .as_array()
            .unwrap()
            .iter()
            .map(fragment_from)
            .collect();
        let messages =
            feed(&mut Reassembler::new(), &fragments).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(messages.len(), 1, "{name}");
        assert_eq!(
            hex_encode(messages[0].as_bytes()),
            s(seq, "message_utf8_hex"),
            "{name}"
        );
    }
    for seq in fx["invalid_sequences"].as_array().unwrap() {
        let name = s(seq, "name");
        let fragments: Vec<Vec<u8>> = seq["fragments"]
            .as_array()
            .unwrap()
            .iter()
            .map(fragment_from)
            .collect();
        assert!(feed(&mut Reassembler::new(), &fragments).is_err(), "{name}");
    }
}

#[test]
fn framing_message_size_cap() {
    let mut reassembler = Reassembler::new();
    // 1024 fragments of 16383 bytes fill 16 MiB - 1024 bytes; the cap is on total payload.
    let mut total = 0usize;
    let chunk = vec![1u8; 16384]; // kind 1 + 16383 'x' would be needed; use ascii payload
    let mut fragment = chunk.clone();
    fragment[1..].fill(b'a');
    let mut failed = false;
    for _ in 0..1100 {
        match reassembler.push(&fragment) {
            Ok(None) => total += 16383,
            Ok(Some(_)) => panic!("no end sent"),
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    assert!(failed, "the 16 MiB cap must trip");
    assert!(total <= 16 * 1024 * 1024);
    assert_eq!(
        reassembler.pending_bytes(),
        0,
        "an error resets the reassembler"
    );
    let too_big = "a".repeat(16 * 1024 * 1024 + 1);
    assert!(encode_message(&too_big, 16384).is_err());
    assert!(encode_message("x", 1).is_err());
    assert!(encode_message("x", 16385).is_err());
}

// --- tunnel -----------------------------------------------------------------

#[test]
fn ephemeral_keys_and_shared_secret() {
    let fx = fixture("tunnel");
    let base = x25519_ref::base_point();
    let viewer_seed: [u8; 32] = hex_decode(s(&fx["viewer_eph"], "seed_hex"))
        .try_into()
        .unwrap();
    let host_seed: [u8; 32] = hex_decode(s(&fx["host_eph"], "seed_hex"))
        .try_into()
        .unwrap();
    let viewer_pub = x25519_ref::x25519(&viewer_seed, &base);
    let host_pub = x25519_ref::x25519(&host_seed, &base);
    // The reference implementation reproduces the fixture, so it is trustworthy for the rest.
    assert_eq!(hex_encode(&viewer_pub), s(&fx["viewer_eph"], "public_hex"));
    assert_eq!(hex_encode(&host_pub), s(&fx["host_eph"], "public_hex"));
    let shared = x25519_ref::x25519(&viewer_seed, &host_pub);
    assert_eq!(hex_encode(&shared), s(&fx, "shared_hex"));
    assert_eq!(x25519_ref::x25519(&host_seed, &viewer_pub), shared);

    // `ring` agrees with the reference on the fixture's public keys: a random ring key
    // against the fixture host key yields what the reference computes from the ring key's
    // public half.
    let ring_key = EphemeralKey::generate();
    let ring_pub = ring_key.public_key();
    let via_ring = ring_key.agree(&host_pub).unwrap();
    assert_eq!(
        via_ring.as_bytes(),
        &x25519_ref::x25519(&host_seed, &ring_pub)
    );
}

#[test]
fn x25519_rejects_low_order_points() {
    for point in [[0u8; 32], {
        let mut one = [0u8; 32];
        one[0] = 1;
        one
    }] {
        assert!(EphemeralKey::generate().agree(&point).is_err());
    }
    assert!(EphemeralKey::generate().agree(&[1u8; 31]).is_err());
}

#[test]
fn hkdf_and_directional_keys() {
    let fx = fixture("tunnel");
    let shared = hex_decode(s(&fx, "shared_hex"));
    let h = &fx["hkdf"];
    let conn_id = s(&fx, "conn_id");
    assert_eq!(s(h, "salt_utf8"), conn_id);
    let up = hkdf_sha256(
        &shared,
        conn_id.as_bytes(),
        s(h, "info_up_utf8").as_bytes(),
        32,
    );
    let down = hkdf_sha256(
        &shared,
        conn_id.as_bytes(),
        s(h, "info_down_utf8").as_bytes(),
        32,
    );
    assert_eq!(hex_encode(&up), s(h, "k_up_hex"));
    assert_eq!(hex_encode(&down), s(h, "k_down_hex"));
    // derive_tunnel_keys produces keys that interoperate with the fixture's frames.
    let keys = derive_tunnel_keys(&shared, conn_id).unwrap();
    let frame = &fx["frames"][0];
    let opened = open_frame(&keys.up, conn_id, TunnelDir::Up, 0, s(frame, "stored_b64u")).unwrap();
    assert_eq!(hex_encode(&opened), s(frame, "plaintext_hex"));
    assert!(format!("{:?}", keys.up).contains(".."));
    let _ = SharedSecret::from_bytes([0; 32]);
}

fn fragments_from_hex(frame: &Value) -> Vec<Vec<u8>> {
    frame["fragments_hex"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| hex_decode(h.as_str().unwrap()))
        .collect()
}

#[test]
fn tunnel_frames() {
    let fx = fixture("tunnel");
    let conn_id = s(&fx, "conn_id");
    let up = TunnelKey::from_bytes(&hex_decode(s(&fx["hkdf"], "k_up_hex"))).unwrap();
    let down = TunnelKey::from_bytes(&hex_decode(s(&fx["hkdf"], "k_down_hex"))).unwrap();
    for frame in fx["frames"].as_array().unwrap() {
        let name = s(frame, "name");
        let dir = dir_of(s(frame, "dir"));
        let seq = frame["seq"].as_u64().unwrap();
        let key = if dir == TunnelDir::Up { &up } else { &down };
        assert_eq!(
            remote_control_client::signaling::frame_key(seq).unwrap(),
            s(frame, "key"),
            "{name}"
        );
        assert_eq!(
            hex_encode(&tunnel_nonce(seq)),
            s(frame, "nonce_hex"),
            "{name}"
        );
        assert_eq!(
            String::from_utf8(tunnel_aad(conn_id, dir, seq)).unwrap(),
            s(frame, "aad_utf8"),
            "{name}"
        );
        let fragments = fragments_from_hex(frame);
        let plaintext = pack_fragments(&fragments).unwrap();
        assert_eq!(hex_encode(&plaintext), s(frame, "plaintext_hex"), "{name}");
        let stored = seal_frame(key, conn_id, dir, seq, &plaintext).unwrap();
        assert_eq!(stored, s(frame, "stored_b64u"), "{name}");
        let opened = open_frame(key, conn_id, dir, seq, &stored).unwrap();
        assert_eq!(opened, plaintext, "{name}");
        assert_eq!(unpack_fragments(&opened).unwrap(), fragments, "{name}");
    }
    // The close notice is the frame without fragments: its stored value is the bare tag.
    let notice = fx["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| s(f, "name").contains("close notice"))
        .unwrap();
    let stored = s(notice, "stored_b64u");
    assert_eq!(b64u_decode(stored).unwrap().len(), 16);
    let opened = open_frame(
        &up,
        conn_id,
        TunnelDir::Up,
        notice["seq"].as_u64().unwrap(),
        stored,
    )
    .unwrap();
    assert!(unpack_fragments(&opened).unwrap().is_empty());
}

#[test]
fn tunnel_large_frames() {
    let fx = fixture("tunnel");
    let conn_id = s(&fx, "conn_id");
    let up = TunnelKey::from_bytes(&hex_decode(s(&fx["hkdf"], "k_up_hex"))).unwrap();
    let down = TunnelKey::from_bytes(&hex_decode(s(&fx["hkdf"], "k_down_hex"))).unwrap();
    for frame in fx["large_frames"].as_array().unwrap() {
        let name = s(frame, "name");
        let dir = dir_of(s(frame, "dir"));
        let seq = frame["seq"].as_u64().unwrap();
        let key = if dir == TunnelDir::Up { &up } else { &down };
        let fragments: Vec<Vec<u8>> = frame["fragments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let mut out = vec![f["kind"].as_u64().unwrap() as u8];
                out.resize(
                    1 + f["payload_length"].as_u64().unwrap() as usize,
                    hex_decode(s(f, "fill_hex"))[0],
                );
                out
            })
            .collect();
        let plaintext = pack_fragments(&fragments).unwrap();
        assert_eq!(
            plaintext.len() as u64,
            frame["plaintext_length"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            sha256_hex(&plaintext),
            s(frame, "plaintext_sha256_hex"),
            "{name}"
        );
        let stored = seal_frame(key, conn_id, dir, seq, &plaintext).unwrap();
        assert_eq!(
            stored.len() as u64,
            frame["stored_length"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            sha256_hex(stored.as_bytes()),
            s(frame, "stored_sha256_hex"),
            "{name}"
        );
        assert_eq!(
            open_frame(key, conn_id, dir, seq, &stored).unwrap(),
            plaintext,
            "{name}"
        );
    }
    // One byte over the plaintext cap cannot be sealed.
    assert!(seal_frame(&up, conn_id, TunnelDir::Up, 0, &vec![0u8; 49137]).is_err());
}

#[test]
fn tunnel_open_and_unpack_failures() {
    let fx = fixture("tunnel");
    let up = TunnelKey::from_bytes(&hex_decode(s(&fx["hkdf"], "k_up_hex"))).unwrap();
    for case in fx["open_failures"].as_array().unwrap() {
        let name = s(case, "name");
        let result = open_frame(
            &up,
            s(case, "conn_id"),
            dir_of(s(case, "dir")),
            case["seq"].as_u64().unwrap(),
            s(case, "stored_b64u"),
        );
        assert!(result.is_err(), "{name}");
    }
    for case in fx["unpack_failures"].as_array().unwrap() {
        let name = s(case, "name");
        let plaintext = if let Some(hex) = case.get("plaintext_hex").and_then(Value::as_str) {
            hex_decode(hex)
        } else {
            let mut out = Vec::new();
            for f in case["fragments"].as_array().unwrap() {
                let length = f["length"].as_u64().unwrap() as usize;
                out.extend_from_slice(&(length as u32).to_be_bytes());
                out.resize(out.len() + length, hex_decode(s(f, "fill_hex"))[0]);
            }
            out
        };
        assert!(unpack_fragments(&plaintext).is_err(), "{name}");
    }
    // A sealed value over the 65536-character bound is refused before decoding.
    assert!(open_frame(&up, "c", TunnelDir::Up, 0, &"A".repeat(65537)).is_err());
    assert!(open_frame(&up, "c", TunnelDir::Up, 0, "AA=").is_err());
    let _ = (b64u_encode(&[]), sha256(&[]));
}
