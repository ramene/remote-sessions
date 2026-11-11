//! State, wire-level items and messages. Everything here is plain data: the reducer (`reduce.rs`) is a pure function
//! over it, and the views (`view.rs`) only read it.
use crossterm::event::KeyEvent;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::mpsc::Sender;

/// The permission modes a session accepts (same list as `claude --permission-mode`).
pub const MODES: [&str; 6] = ["default", "acceptEdits", "auto", "plan", "dontAsk", "bypassPermissions"];

/// Short footer label of a mode.
pub fn mode_label(mode: &str) -> &'static str {
    match mode {
        "default" => "manual",
        "acceptEdits" => "accept-edits",
        "auto" => "auto",
        "plan" => "plan",
        "dontAsk" => "dont-ask",
        "bypassPermissions" => "bypass",
        _ => "?",
    }
}

/// A mode name from user input: the wire name or the footer label.
pub fn parse_mode(text: &str) -> Option<&'static str> {
    let t = text.trim();
    MODES.iter().copied().find(|m| m.eq_ignore_ascii_case(t) || mode_label(m) == t.to_ascii_lowercase())
}

/// The Tab cycle. `auto`, `dontAsk` and `bypassPermissions` are never entered by cycling (and leave to `default`).
pub fn next_mode(current: Option<&str>) -> &'static str {
    match current {
        None | Some("default") => "acceptEdits",
        Some("acceptEdits") => "plan",
        _ => "default",
    }
}

/// One structured piece of an event (see the backend adapter; mirrors the core's `events::Block`).
#[derive(Clone, Debug, PartialEq)]
pub enum Blk {
    Text(String),
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, text: String, is_error: bool },
    Thinking,
    Request { request_id: String, subtype: String, tool_name: String, input: Value, mode: Option<String> },
    Response { request_id: String },
    Cancel { request_id: String },
}

/// What a `result` event says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResultInfo {
    pub subtype: String,
    pub turns: Option<u64>,
    pub duration_ms: Option<u64>,
}

/// What a `system` `init` event says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InitInfo {
    pub model: Option<String>,
    pub mode: Option<String>,
}

/// One session event, already decoded by the backend.
#[derive(Clone, Debug, PartialEq)]
pub struct Ev {
    pub seq: Option<u64>,
    pub id: String,
    /// The raw `type`: user, assistant, result, system, control_request, ...
    pub kind: String,
    pub blocks: Vec<Blk>,
    pub result: Option<ResultInfo>,
    pub init: Option<InitInfo>,
    /// A one-line text for kinds without blocks (system notices, rate limits, unknown types).
    pub note: Option<String>,
}

impl Ev {
    pub fn new(seq: Option<u64>, id: &str, kind: &str, blocks: Vec<Blk>) -> Ev {
        Ev { seq, id: id.to_string(), kind: kind.to_string(), blocks, result: None, init: None, note: None }
    }
}

/// What a stream or poll delivers.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Event(Ev),
    /// `status` is the suffix of `DELIVERY_STATUS_*`: RECEIVED, PROCESSING or PROCESSED.
    Receipt { event_id: String, status: String },
}

/// The first load: the newest events oldest-first, and where the stream continues.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub host: String,
    pub events: Vec<Ev>,
    /// True when older entries exist that are not in `events`.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StreamEnd {
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Allow,
    Deny(Option<String>),
}

/// Result of a call made on a worker thread.
#[derive(Clone, Debug, PartialEq)]
pub enum Net {
    Say { uuid: String, res: Result<(), String> },
    Interrupt(Result<(), String>),
    Mode { mode: String, res: Result<(), String> },
    Answer { request_id: String, allow: bool, res: Result<(), String> },
}

/// Everything the main loop reacts to.
#[derive(Clone, Debug)]
pub enum Msg {
    Key(KeyEvent),
    Paste(String),
    /// Wheel: positive = up (older), in notches.
    Wheel(i32),
    /// A click on the transcript row that belongs to this block.
    Click(usize),
    /// A mouse press at (column, row); the app turns it into Click when it lands on a block.
    ClickAt(u16, u16),
    Resize(u16, u16),
    /// Milliseconds since the TUI started.
    Tick(u64),
    Snapshot(Result<Snapshot, String>),
    /// The stream connected.
    Open,
    Item(Item),
    StreamEnded(StreamEnd),
    /// Time to try the stream again while polling.
    Retry,
    PollFailed(String),
    Net(Net),
}

/// What the app must do after a reduction.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    StartStream { from: Option<u64> },
    StartPoll,
    StopPoll,
    Say { uuid: String, text: String },
    Interrupt,
    SetMode(String),
    Answer { request_id: String, decision: Decision },
    Mouse(bool),
    Redraw,
    SaveHistory(String),
    Quit,
}

/// The seam between the TUI and a session service. All calls block; the app calls them from worker threads.
pub trait Backend: Send + Sync {
    /// The newest events (oldest first) plus session meta.
    fn snapshot(&self) -> Result<Snapshot, String>;
    /// Follow the session from `from` (exclusive). Blocks: sends `Msg::Open` once connected, `Msg::Item` per item, and
    /// `Msg::StreamEnded` last. Returns when the receiver is gone.
    fn stream(&self, from: Option<u64>, tx: Sender<Msg>);
    /// Send a user turn. `uuid` is the event id the server will echo.
    fn say(&self, uuid: &str, text: &str) -> Result<(), String>;
    fn interrupt(&self) -> Result<(), String>;
    fn set_mode(&self, mode: &str) -> Result<(), String>;
    fn answer(&self, request_id: &str, decision: Decision) -> Result<(), String>;
    /// Events to merge while the stream is down (the reducer de-duplicates).
    fn poll(&self) -> Result<Vec<Ev>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Conn {
    Connecting,
    Live,
    Polling,
    Reconnecting(u32),
    Offline,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Turn {
    Idle,
    Waiting { since: u64 },
    Working { since: u64, tool: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    None,
    Permission { choice: usize, reason: Option<String>, sending: bool, full: bool },
    ModePicker { choice: usize },
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Local,
    Sent,
    Processed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Level {
    Dim,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOut {
    pub text: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    User { text: String, mark: Mark },
    Assistant(String),
    Tool { tool_id: String, name: String, input: Value, result: Option<ToolOut> },
    /// A tool result whose call is not in the transcript.
    Result(ToolOut),
    Notice { text: String, level: Level },
    Divider(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: usize,
    /// The event uuid (empty for local notices).
    pub key: String,
    pub kind: BlockKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
}

/// The prompt text with its cursor (a char index).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}

#[derive(Clone, Debug)]
pub struct State {
    pub name: String,
    pub host: String,
    pub mode: Option<String>,
    pub conn: Conn,
    pub turn: Turn,
    pub modal: Modal,
    pub blocks: Vec<Block>,
    pub seen: HashSet<String>,
    pub last_seq: Option<u64>,
    pub prompts: Vec<Prompt>,
    /// The request we are answering right now (so its closing event is not "answered elsewhere").
    pub answering: Option<String>,
    pub input: Input,
    pub history: Vec<String>,
    pub hist_pos: Option<usize>,
    pub hist_query: Option<String>,
    pub complete_idx: usize,
    pub complete_base: Option<String>,
    pub expand_all: bool,
    pub expanded: HashSet<usize>,
    pub show_log: bool,
    pub mouse: bool,
    /// Lines scrolled up from the bottom; 0 = pinned.
    pub scroll: usize,
    pub new_count: usize,
    /// Set by the app after each draw.
    pub view_total: usize,
    pub view_h: usize,
    pub size: (u16, u16),
    pub now_ms: u64,
    pub last_event_ms: u64,
    pub last_esc_ms: Option<u64>,
    pub ended: bool,
    pub fatal: Option<String>,
    pub poll_active: bool,
    pub rec_since: Option<u64>,
    pub queued: Vec<(String, String)>,
    pub last_rate: Option<String>,
    pub last_init: Option<String>,
    pub next_id: usize,
    pub seed: u64,
    pub uuid_n: u64,
    pub quit: bool,
}

impl State {
    pub fn new(name: &str, host: &str) -> State {
        State {
            name: name.to_string(), host: host.to_string(), mode: None, conn: Conn::Connecting, turn: Turn::Idle, modal: Modal::None,
            blocks: Vec::new(), seen: HashSet::new(), last_seq: None, prompts: Vec::new(), answering: None, input: Input::default(), history: Vec::new(),
            hist_pos: None, hist_query: None, complete_idx: 0, complete_base: None, expand_all: false, expanded: HashSet::new(), show_log: false, mouse: true, scroll: 0, new_count: 0,
            view_total: 0, view_h: 0, size: (80, 24), now_ms: 0, last_event_ms: 0, last_esc_ms: None, ended: false, fatal: None, poll_active: false,
            rec_since: None, queued: Vec::new(), last_rate: None, last_init: None, next_id: 1, seed: 0x5eed_5eed_5eed_5eed, uuid_n: 0, quit: false,
        }
    }

    pub fn busy(&self) -> bool {
        !matches!(self.turn, Turn::Idle)
    }

    pub fn spinner_active(&self) -> bool {
        self.fatal.is_none() && !self.ended && (matches!(self.conn, Conn::Connecting) || (self.busy() && self.modal_is_none_or_help() && self.prompts.is_empty()))
    }

    fn modal_is_none_or_help(&self) -> bool {
        matches!(self.modal, Modal::None | Modal::Help | Modal::ModePicker { .. })
    }

    /// A fresh version-4-shaped uuid, deterministic for a given seed (the app seeds it from the OS).
    pub fn new_uuid(&mut self) -> String {
        self.uuid_n += 1;
        let mut z = self.seed ^ self.uuid_n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut next = || {
            z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        };
        let (a, b) = (next(), next());
        let h = format!("{a:016x}{b:016x}");
        format!("{}-{}-4{}-{}{}-{}", &h[..8], &h[8..12], &h[13..16], ["8", "9", "a", "b"][(b & 3) as usize], &h[17..20], &h[20..32])
    }
}
