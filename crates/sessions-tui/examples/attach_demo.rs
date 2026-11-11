//! A canned session for the `attach --tui` screen, for trying it (and testing the logo header) without a Remote Control session:
//!   cargo run -p sessions-tui --example attach_demo
//! Fake ids and text only; nothing is sent anywhere. Quit with Ctrl-C or Esc.
use sessions_tui::*;
use std::sync::mpsc::Sender;
use std::sync::Arc;

struct Demo;

impl Backend for Demo {
    fn snapshot(&self) -> Result<Snapshot, String> {
        Ok(Snapshot {
            name: "session_01ExampleDemoAAAAAAAAAAA".into(),
            host: "mac-a".into(),
            events: vec![
                Ev::new(Some(1), "u1", "user", vec![Blk::Text("fix the flaky parser test".into())]),
                Ev::new(Some(2), "a1", "assistant", vec![Blk::Text("Reading the failing test first.\n\nThe parser drops a trailing newline; I will keep it.".into())]),
            ],
            truncated: false,
        })
    }
    fn stream(&self, _from: Option<u64>, tx: Sender<Msg>) {
        let _ = tx.send(Msg::Open);
        // Stay "connected" until the process ends.
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    fn say(&self, _uuid: &str, _text: &str) -> Result<(), String> {
        Ok(())
    }
    fn interrupt(&self) -> Result<(), String> {
        Ok(())
    }
    fn set_mode(&self, _mode: &str) -> Result<(), String> {
        Ok(())
    }
    fn answer(&self, _request_id: &str, _decision: Decision) -> Result<(), String> {
        Ok(())
    }
    fn poll(&self) -> Result<Vec<Ev>, String> {
        Ok(vec![])
    }
}

fn main() {
    let history = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("attach-demo-history")));
    let opts = Options { name: "session_01ExampleDemoAAAAAAAAAAA".into(), host: "mac-a".into(), history_path: history, ..Default::default() };
    match run(Arc::new(Demo), opts) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("attach_demo: {e}");
            std::process::exit(1);
        }
    }
}
