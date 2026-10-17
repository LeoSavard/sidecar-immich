// SPDX-License-Identifier: Apache-2.0
//! `sidecar-ml serve` driven the way the worker drives it: one end of a socket pair as its stdin.
//! Needs the weights (`dev/ml-models`) and `testdata/cache/img1024.rgb`; skipped without them.

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};

fn send(
    socket: &mut UnixStream,
    header: serde_json::Value,
    pixels: &[u8],
) -> (serde_json::Value, Vec<u8>) {
    let header = serde_json::to_vec(&header).unwrap();
    socket
        .write_all(&(header.len() as u32).to_le_bytes())
        .unwrap();
    socket.write_all(&header).unwrap();
    socket.write_all(pixels).unwrap();
    let mut len = [0u8; 4];
    socket.read_exact(&mut len).unwrap();
    let mut reply = vec![0u8; u32::from_le_bytes(len) as usize];
    socket.read_exact(&mut reply).unwrap();
    let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    let size = ["width", "height", "channels"]
        .iter()
        .map(|k| reply[k].as_u64().unwrap_or(0))
        .product::<u64>();
    let mut payload = vec![0u8; size as usize];
    socket.read_exact(&mut payload).unwrap();
    (reply, payload)
}

#[test]
fn answers_on_its_socket_and_exits_when_it_closes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let models = root.join("target/ml-models");
    let Ok(photo) = std::fs::read(root.join("testdata/cache/img1024.rgb")) else {
        return eprintln!("skipped: no test photo");
    };
    if !models.join("birefnet.bpk").is_file() {
        return eprintln!("skipped: no weights");
    }
    let (mut ours, theirs) = UnixStream::pair().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_sidecar-ml"))
        .args(["serve", "--idle", "60", "--models"])
        .arg(&models)
        .stdin(Stdio::from(OwnedFd::from(theirs)))
        .spawn()
        .unwrap();

    let (reply, _) = send(
        &mut ours,
        serde_json::json!({"task": "faces", "width": 1024, "height": 1024}),
        &photo,
    );
    assert_eq!(reply["faces"].as_array().map(Vec::len), Some(1), "{reply}");

    let (reply, mask) = send(
        &mut ours,
        serde_json::json!({"task": "subject", "width": 1024, "height": 1024, "longEdge": 256}),
        &photo,
    );
    assert_eq!(
        (
            reply["width"].as_u64(),
            reply["height"].as_u64(),
            mask.len()
        ),
        (Some(256), Some(256), 256 * 256),
        "{reply}"
    );

    // A picture as small as 2 × 2 works; an unknown task is refused, and the process carries on.
    let (reply, _) = send(
        &mut ours,
        serde_json::json!({"task": "parts", "width": 2, "height": 2}),
        &[0; 2 * 2 * 3],
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let (reply, _) = send(&mut ours, serde_json::json!({"task": "nonsense"}), &[]);
    assert_eq!(reply["ok"], false, "{reply}");
    let (reply, _) = send(&mut ours, serde_json::json!({"task": "warm"}), &[]);
    assert_eq!(reply["ok"], true, "{reply}");

    drop(ours);
    assert!(child.wait().unwrap().success());
}
