//! AC-002-01: every protocol fixture parses and round-trips.

use rustybee::domain::inbound_uid;
use rustybee::protocol::{self, Frame, PairRequest, PairResponse};
use serde_json::Value;
use std::path::PathBuf;

fn fixtures() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../protocol/fixtures");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("fixtures dir")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            (
                p.file_stem().unwrap().to_string_lossy().to_string(),
                std::fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn ac_002_01_every_fixture_parses_and_round_trips() {
    let all = fixtures();
    assert!(
        all.len() >= 13,
        "expected all fixtures, found {}",
        all.len()
    );
    for (name, text) in all {
        let original: Value = serde_json::from_str(&text).unwrap();
        let reencoded: Value = match name.as_str() {
            "pair_request" => {
                serde_json::to_value(serde_json::from_str::<PairRequest>(&text).unwrap()).unwrap()
            }
            "pair_response" => {
                serde_json::to_value(serde_json::from_str::<PairResponse>(&text).unwrap()).unwrap()
            }
            _ => {
                let (id, frame) = protocol::decode(&text)
                    .unwrap_or_else(|e| panic!("{name}: {} {}", e.code, e.message));
                assert_eq!(original["type"], frame.kind(), "{name}");
                serde_json::from_str(&frame.into_envelope(id).to_text()).unwrap()
            }
        };
        assert_eq!(reencoded, original, "{name} round-trip");
    }
}

#[test]
fn ac_002_01_inbound_uid_matches_hash() {
    let text = fixtures()
        .into_iter()
        .find(|(n, _)| n == "inbound")
        .unwrap()
        .1;
    let (_, frame) = protocol::decode(&text).unwrap();
    let Frame::Inbound(m) = frame else {
        panic!("not inbound")
    };
    assert_eq!(m.uid, inbound_uid(&m.from, m.received_at, &m.body, m.sim));
}
