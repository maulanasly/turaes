//! serve-static boots with a bare environment: supervised units carry no
//! EnvironmentFile, so config loading (and its release gates) must never run
//! for it. Regression test spawns the real binary with `env -i` plus a
//! deliberately broken `TURAES_CONFIG` and probes `/health` — any config load
//! would fatal out before serving.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn wait_for_health(port: u16) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut stream) = TcpStream::connect(format!("127.0.0.1:{port}")) {
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let _ = stream.write_all(b"GET /health HTTP/1.0\r\n\r\n");
            let mut buf = vec![0u8; 1024];
            if let Ok(n) = stream.read(&mut buf) {
                let body = String::from_utf8_lossy(&buf[..n]).into_owned();
                if body.contains("200") && body.contains("ok") {
                    return true;
                }
            }
            return false;
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn serve_static_boots_without_config() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<h1>hi</h1>").unwrap();
    let garbage = dir.path().join("garbage.toml");
    std::fs::write(&garbage, "[[[this is not toml").unwrap();
    let bin = env!("CARGO_BIN_EXE_turaes");

    let mut child = std::process::Command::new(bin)
        .args([
            "serve-static",
            "--dir",
            &dir.path().to_string_lossy(),
            "--port",
            "19777",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TURAES_CONFIG", &garbage)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn turaes serve-static");

    let healthy = wait_for_health(19777);
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        healthy,
        "serve-static answered /health with a bare environment"
    );
}
