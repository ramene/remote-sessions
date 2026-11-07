//! End card integration: render a 2 s MP4, append the card, check the duration grows by `duration_s` and the .cast is untouched.
//! Skips cleanly when ffmpeg (or its drawtext filter) is not installed.
use remote_sessions_core::endcard::{self, Outcome, Spec};
use std::process::Command;

#[test]
fn card_adds_duration_and_leaves_the_cast_untouched() {
    let Some(ffmpeg) = endcard::find_ffmpeg() else { eprintln!("skip: no ffmpeg"); return };
    let filters = Command::new(&ffmpeg).args(["-hide_banner", "-filters"]).output().unwrap();
    if !String::from_utf8_lossy(&filters.stdout).contains(" drawtext ") { eprintln!("skip: no drawtext"); return; }
    let dir = std::env::temp_dir().join(format!("endcard-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cast = dir.join("demo.cast");
    std::fs::write(&cast, "{\"version\":2,\"width\":80,\"height\":24}\n[0.5,\"o\",\"hi\\r\\n\"]\n").unwrap();
    let mp4 = dir.join("demo.mp4");
    let made = Command::new(&ffmpeg).args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=2", "-pix_fmt", "yuv420p", "-c:v", "libx264"]).arg(&mp4).status().unwrap();
    assert!(made.success());
    let dur = |p: &std::path::Path| {
        let o = Command::new(&ffmpeg).args(["-hide_banner", "-i"]).arg(p).output().unwrap();
        endcard::parse_duration(&String::from_utf8_lossy(&o.stderr)).unwrap()
    };
    let (before, sha_before) = (dur(&mp4), endcard::sha256_file(&cast).unwrap());
    let spec = Spec { duration_s: 3.0, ..Spec::default() };
    let outcome = endcard::apply_with(&spec, "remote-sessions", &cast, &mp4);
    let (after, sha_after) = (dur(&mp4), endcard::sha256_file(&cast).unwrap());
    eprintln!("duration before={before} after={after} cast sha256 before={sha_before} after={sha_after}");
    assert_eq!(outcome, Outcome::Applied);
    assert!((after - before - 3.0).abs() < 0.25, "duration grew by {} not 3", after - before);
    assert_eq!(sha_before, sha_after, "the .cast must never be modified");
    assert!(!dir.join("demo.endcard-tmp.mp4").exists());
    // disabled: the MP4 is untouched
    let off = Spec { enabled: false, ..Spec::default() };
    assert_eq!(endcard::apply_with(&off, "remote-sessions", &cast, &mp4), Outcome::Disabled);
    assert!((dur(&mp4) - after).abs() < 0.01);
    let _ = std::fs::remove_dir_all(&dir);
}
