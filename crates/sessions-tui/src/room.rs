//! The control room: a styled, clickable, interactive view over Remote Control sessions and cloud sessions.
//!
//! Two bordered panels (Remote Control, Cloud), a detail panel for the selected session, a status line and a footer
//! of actions. Keyboard and mouse. Like the transcript UI it is a pure core: [`reduce`] is a function over
//! [`RoomState`] (messages in, effects out), [`layout`] is a function of the state and the screen size (it is also
//! what turns a click position into a row), and [`draw`] only paints. Nothing here starts a process or opens a
//! socket: the host program implements [`RoomBackend`] (see `room_app` for the loop that drives it).
//!
//! Everything that came from a session (ids, titles, activity text, tool input) is untrusted and is cleaned before it
//! is drawn. Cloud sessions raise no permission prompts, so there is no approve or deny for them.
use crate::text::{clean, one_line, truncate, width, wrap_plain};
use crate::theme::Theme;
use crate::view::{border_set, sp, tool_args};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use serde_json::Value;
use unicode_width::UnicodeWidthStr;

/// A cloud session whose newest event is younger than this (and that is not finished) is shown as running.
pub const RUNNING_WINDOW_MS: u64 = 300_000;
/// A cloud row's latest activity older than this is read again when the row is selected.
pub const STALE_MS: u64 = 10_000;
pub const MIN_W: u16 = 30;
pub const MIN_H: u16 = 9;
const DETAIL_H: u16 = 7;
const SEND_MAX: usize = 4_000;
/// Width of the header's close button: ` x close ` (the glyph is one column in both glyph sets).
const CLOSE_W: u16 = 9;

// ---- data ---------------------------------------------------------------------------------------------------------

/// A pending permission prompt of a Remote Control session.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptInfo {
    pub request_id: String,
    pub tool: String,
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RState {
    Waiting,
    Idle,
    Unreadable(String),
    /// No claude.ai login, so what the session waits on cannot be read.
    Unknown,
    /// Not a session: the host of this row could not be listed (the reason, e.g. "ssh timed out").
    HostDown(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteRow {
    /// The machine that runs the session; empty when the room lists one machine only (no host column).
    pub host: String,
    pub id: String,
    pub pid: Option<u32>,
    pub state: RState,
    /// Newest first.
    pub prompts: Vec<PromptInfo>,
}

impl RemoteRow {
    /// A row that stands for a host that could not be listed, not for a session: never selected.
    pub fn is_host_down(&self) -> bool {
        matches!(self.state, RState::HostDown(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CState {
    Running,
    Idle,
    Finished,
    Unreadable,
    /// Its activity has not been read yet.
    Unknown,
}

/// What a cloud-panel row is. Routine and Note rows are dim group rows: drawn, never selected, never read.
#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    /// A tracked or account session.
    Session,
    /// A run that a routine started (a normal, selectable row).
    Run,
    /// A routine's header: its name is the row title.
    Routine { schedule: String, enabled: bool },
    /// A one-line note in the panel (for example, routines could not be read).
    Note,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloudRow {
    pub id: String,
    pub title: String,
    pub repo: String,
    /// The newest event as one line (kind and text).
    pub last: String,
    pub last_at_ms: Option<u64>,
    pub finished: bool,
    pub error: Option<String>,
    /// When the activity was read; None = not yet.
    pub fetched_ms: Option<u64>,
    /// `last_at_ms` and `finished` come from the list itself, so the row has a state on the first frame without an
    /// activity read. (`events` is still read for the selected row's `last` detail.)
    pub from_list: bool,
    pub kind: RowKind,
}

impl CloudRow {
    /// A dim group row (never selectable).
    pub fn is_group(&self) -> bool {
        matches!(self.kind, RowKind::Routine { .. } | RowKind::Note)
    }
    pub fn listed(id: &str, title: &str, repo: &str) -> CloudRow {
        CloudRow { id: id.into(), title: title.into(), repo: repo.into(), last: String::new(), last_at_ms: None, finished: false, error: None, fetched_ms: None, from_list: false, kind: RowKind::Session }
    }
    pub fn state(&self, now_ms: u64) -> CState {
        if self.error.is_some() {
            return CState::Unreadable;
        }
        if self.fetched_ms.is_none() && !self.from_list {
            return CState::Unknown;
        }
        if self.finished {
            return CState::Finished;
        }
        match self.last_at_ms {
            Some(at) if now_ms.saturating_sub(at) < RUNNING_WINDOW_MS => CState::Running,
            _ => CState::Idle,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RemoteStatus {
    Loading,
    Ready,
    NoLogin,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum CloudStatus {
    Loading,
    /// `tracked` is how many sessions are tracked in all (more than the rows when the list is capped).
    Ready { tracked: usize },
    Absent,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteSnapshot {
    /// The machines listed, in order, when the room lists more than one (empty: one machine, no host column).
    pub hosts: Vec<String>,
    pub rows: Vec<RemoteRow>,
    pub status: RemoteStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CloudSnapshot {
    pub rows: Vec<CloudRow>,
    pub status: CloudStatus,
}

/// What reading one cloud session's newest event gave.
#[derive(Clone, Debug, PartialEq)]
pub struct CloudActivity {
    pub id: String,
    pub finished: bool,
    pub last: String,
    pub last_at_ms: Option<u64>,
    pub error: Option<String>,
}

// ---- more panels: the panel interface ------------------------------------------------------------------------------
//
// Remote Control and Cloud are the room's own two panels. A host program can give the room more panels: each one is a
// [`RoomPanel`] with a name ([`PanelSpec`]), rows ([`PanelRow`], read by [`RoomPanel::read`]) and one action, the
// drop-in ([`RoomPanel::drop_in`]). The room draws such a panel only after its first read answered (a status other than
// [`PanelStatus::Absent`]), so a panel whose source is not there costs no screen space.

/// The color role of a row's state glyph and state text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Error,
    Dim,
}

/// A row's state glyph (each has an ASCII form in the ASCII glyph set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Dot,
    Ring,
    Done,
    Cross,
    Pause,
    Notice,
}

/// How a piece of text is styled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// The style of the row's [`Tone`].
    Tone,
    Accent,
    Accent2,
    Dim,
    Bold,
    Plain,
}

/// How wide a column of a row is, given `left`, the columns still free on the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Exactly this many columns (padded or cut).
    Fixed(usize),
    /// `left / div`, kept within `min..=max`.
    Share { div: usize, min: usize, max: usize },
    /// The rest of the line (cut, not padded). Use it last.
    Rest,
}

/// The name column of a panel's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameFit {
    /// The same width for every row, so the columns line up.
    Fixed(usize),
    /// The name's own width, kept within `min..=max`.
    Clamp(usize, usize),
}

/// One piece of text. `parts` are joined with the theme's separator glyph; an empty part after the first is left out.
#[derive(Clone, Debug, PartialEq)]
pub struct Txt {
    pub parts: Vec<String>,
}

impl Txt {
    pub fn new(text: impl Into<String>) -> Txt {
        Txt { parts: vec![text.into()] }
    }
    pub fn joined(parts: Vec<String>) -> Txt {
        Txt { parts }
    }
    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| p.is_empty())
    }
    fn render(&self, th: &Theme) -> String {
        let mut out = String::new();
        for (i, p) in self.parts.iter().enumerate() {
            if i == 0 {
                out.push_str(p);
            } else if !p.is_empty() {
                out.push_str(&format!(" {} {p}", th.sep().trim()));
            }
        }
        out
    }
    /// Control characters gone, line breaks become spaces, at most `max` columns. Spaces are kept (labels such as
    /// `"Enter "` rely on them); a source that wants whitespace collapsed uses `one_line` first.
    fn cleaned(mut self, max: usize) -> Txt {
        for p in &mut self.parts {
            *p = truncate(&clean(p, max * 8 + 64).replace('\n', " "), max);
        }
        self
    }
}

/// A column after the name: shown only when at least `min_left` columns are free (and, with `skip_empty`, only when
/// it has text). Each column starts with two spaces.
#[derive(Clone, Debug, PartialEq)]
pub struct Col {
    pub text: Txt,
    pub ink: Ink,
    pub fit: Fit,
    pub min_left: usize,
    pub skip_empty: bool,
}

/// The longest a detail piece may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Max {
    Any,
    Cols(usize),
    /// The detail panel's width less this many columns.
    WidthLess(usize),
}

/// One piece of a detail line.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub text: Txt,
    pub ink: Ink,
    pub max: Max,
}

impl Piece {
    pub fn new(text: impl Into<String>, ink: Ink, max: Max) -> Piece {
        Piece { text: Txt::new(text), ink, max }
    }
}

/// One row of a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelRow {
    /// The room's key for this row (and what [`RoomPanel::drop_in`] is given).
    pub id: String,
    pub name: String,
    /// Rows are shown lowest rank first; rows of one rank keep their order.
    pub rank: u8,
    pub glyph: Glyph,
    pub tone: Tone,
    pub cols: Vec<Col>,
    /// What the detail panel shows for this row, one line each.
    pub detail: Vec<Vec<Piece>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PanelStatus {
    /// Nothing answers: the panel is not shown at all.
    Absent,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelSnapshot {
    pub rows: Vec<PanelRow>,
    pub status: PanelStatus,
}

/// What a panel is called and how it words things. Fixed for the life of the room.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelSpec {
    /// The panel title.
    pub title: String,
    /// The short word for the header counts and the status line.
    pub noun: String,
    /// The dim note at the panel's top right when it has rows.
    pub tag: String,
    /// A count in the title of the rows with this tone, shown with this glyph and word (for example "2 working").
    pub badge: Option<(Tone, Glyph, String)>,
    /// What an empty panel says.
    pub empty: String,
    /// What a failed read says, before the error.
    pub failed: String,
    /// The toast for `a` or `d` on a row of this panel.
    pub approve_hint: String,
    /// The toast for `s` on a row of this panel.
    pub send_hint: String,
    pub name_fit: NameFit,
    /// How often the room reads the panel.
    pub every_ms: u64,
}

/// What a panel is given to read and act: the environment and the machine the room looks at (None = this one).
pub struct PanelCx<'a> {
    pub env: &'a (dyn Fn(&str) -> Option<String> + Sync),
    pub host: Option<&'a str>,
}

/// A full-screen two-way conversation view that the host program draws for a drop-in. `turns` are
/// (user | assistant | dim, text), oldest first.
pub struct DropSource<'a> {
    /// The header's label, for example "inside a cloud session".
    pub label: String,
    /// The dim third header line.
    pub info: String,
    /// Who the assistant turns are shown as.
    pub name: String,
    pub every: std::time::Duration,
    pub meta: &'a (dyn Fn() -> (String, String) + Sync),
    pub turns: &'a (dyn Fn() -> Result<Vec<(String, String)>, String> + Sync),
    pub send: &'a (dyn Fn(&str) -> Result<String, String> + Sync),
    /// Show the typed line as a "you" turn at once. Off when the source echoes it itself (a terminal does).
    pub echo: bool,
}

/// The host program's two-way view, for panels that drop in through it.
pub trait DropHost {
    fn two_way(&self, src: &DropSource) -> Result<String, String>;
}

/// A panel beyond Remote Control and Cloud. Every method is one bounded call.
pub trait RoomPanel: Send + Sync {
    fn spec(&self) -> PanelSpec;
    fn read(&self, cx: &PanelCx) -> PanelSnapshot;
    /// Drop into the row `id`. Blocks while the terminal is handed over.
    fn drop_in(&self, id: &str, cx: &PanelCx, view: &dyn DropHost) -> Result<String, String>;
}

/// A panel as the room holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelView {
    pub spec: PanelSpec,
    pub rows: Vec<PanelRow>,
    pub status: PanelStatus,
    pub at_ms: Option<u64>,
}

impl PanelView {
    pub fn new(spec: PanelSpec) -> PanelView {
        PanelView { spec, rows: vec![], status: PanelStatus::Absent, at_ms: None }
    }
    pub fn visible(&self) -> bool {
        self.status != PanelStatus::Absent
    }
    pub fn row(&self, id: &str) -> Option<&PanelRow> {
        self.rows.iter().find(|r| r.id == id)
    }
}

/// A selected row: which panel and which session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sel {
    Remote(String),
    Cloud(String),
    /// A row of the extra panel with this index (the order the backend gave them in).
    Panel(usize, String),
}

impl Sel {
    pub fn id(&self) -> &str {
        match self {
            Sel::Remote(i) | Sel::Cloud(i) | Sel::Panel(_, i) => i,
        }
    }
    /// 0 Remote Control, 1 Cloud, then 2 + the extra panel's index.
    fn panel(&self) -> usize {
        match self {
            Sel::Remote(_) => 0,
            Sel::Cloud(_) => 1,
            Sel::Panel(k, _) => 2 + k,
        }
    }
}

/// The host program: how the room reads and acts. Every method is one bounded call (a single process or request).
pub trait RoomBackend: Send + Sync {
    /// Remote Control sessions and what each waits on.
    fn remote(&self) -> RemoteSnapshot;
    /// The tracked cloud sessions (a listing; no per-session reads).
    fn cloud(&self) -> CloudSnapshot;
    /// One cloud session's newest event.
    fn activity(&self, id: &str) -> CloudActivity;
    /// Answer a Remote Control permission prompt.
    fn answer(&self, session: &str, request_id: &str, allow: bool) -> Result<String, String>;
    /// Send a follow-up message into a cloud session.
    fn cloud_send(&self, id: &str, text: &str) -> Result<String, String>;
    /// Drop into the session. Blocks while the terminal is handed over (the room has already left the alternate screen).
    fn drop_in(&self, target: &Sel) -> Result<String, String>;
    /// The extra panels, in the order they are drawn (none by default).
    fn panels(&self) -> Vec<PanelSpec> {
        vec![]
    }
    /// Read the extra panel with index `k`.
    fn panel(&self, _k: usize) -> PanelSnapshot {
        PanelSnapshot { rows: vec![], status: PanelStatus::Absent }
    }
    /// Whether this backend can show Routines at all (the `R` key and its footer hint exist only then).
    fn routines_supported(&self) -> bool {
        false
    }
    /// Whether Routines are shown now.
    fn routines_on(&self) -> bool {
        false
    }
    /// Show or hide Routines: while off the backend makes no `routines` call at all.
    fn set_routines(&self, _on: bool) {}
}

// ---- state --------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Browse,
    Confirm { allow: bool, session: String, request_id: String, tool: String, args: String },
    Send { id: String, text: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub ok: bool,
}

#[derive(Clone, Debug)]
pub struct RoomState {
    pub host: String,
    pub remote: Vec<RemoteRow>,
    pub remote_status: RemoteStatus,
    /// The machines listed (several machines only), in order, for the per-host counts in the panel title.
    pub remote_hosts: Vec<String>,
    pub remote_at_ms: Option<u64>,
    pub cloud: Vec<CloudRow>,
    pub cloud_status: CloudStatus,
    pub cloud_at_ms: Option<u64>,
    /// The extra panels (see [`RoomPanel`]), in drawing order.
    pub panels: Vec<PanelView>,
    /// The backend can show Routines (the `R` key is offered).
    pub routines_available: bool,
    /// Routines are shown.
    pub routines: bool,
    /// The header is a three-row band with the logo image at its left (the app sets it when there is a logo and room for it).
    pub band: bool,
    /// A blank row between panels where the height allows (the renderSVG look).
    pub air: bool,
    pub sel: Option<Sel>,
    /// First visible row of each panel: Remote Control, Cloud, then the extra panels.
    pub top: Vec<usize>,
    pub mode: Mode,
    pub toast: Option<Toast>,
    pub size: (u16, u16),
    pub now_ms: u64,
    /// The cloud session whose activity is being read (at most one at a time).
    pub fetching: Option<String>,
    pub demo: bool,
}

impl RoomState {
    pub fn new(host: &str) -> RoomState {
        RoomState {
            host: one_line(host, 24),
            remote: vec![],
            remote_status: RemoteStatus::Loading,
            remote_hosts: vec![],
            remote_at_ms: None,
            cloud: vec![],
            cloud_status: CloudStatus::Loading,
            cloud_at_ms: None,
            panels: vec![],
            routines_available: false,
            routines: false,
            band: false,
            air: false,
            sel: None,
            top: vec![0, 0],
            mode: Mode::Browse,
            toast: None,
            size: (80, 24),
            now_ms: 0,
            fetching: None,
            demo: false,
        }
    }

    /// Give the room its extra panels (in drawing order), each hidden until its first read answers.
    pub fn with_panels(mut self, specs: Vec<PanelSpec>) -> RoomState {
        self.panels = specs.into_iter().map(PanelView::new).collect();
        self.top = vec![0; 2 + self.panels.len()];
        self
    }

    /// Every row in the order the arrow keys walk: Remote Control first, then Cloud, then the extra panels.
    pub fn order(&self) -> Vec<Sel> {
        let extra = self.panels.iter().enumerate().flat_map(|(k, p)| p.rows.iter().map(move |r| Sel::Panel(k, r.id.clone())));
        self.remote.iter().filter(|r| !r.is_host_down()).map(|r| Sel::Remote(r.id.clone())).chain(self.cloud.iter().filter(|c| !c.is_group()).map(|c| Sel::Cloud(c.id.clone()))).chain(extra).collect()
    }
    pub fn remote_row(&self, id: &str) -> Option<&RemoteRow> {
        self.remote.iter().find(|r| r.id == id)
    }
    pub fn cloud_row(&self, id: &str) -> Option<&CloudRow> {
        self.cloud.iter().find(|c| c.id == id)
    }
    pub fn panel_row(&self, k: usize, id: &str) -> Option<&PanelRow> {
        self.panels.get(k).and_then(|p| p.row(id))
    }
    /// How many panels there are in all (Remote Control, Cloud and the extra ones).
    fn n_panels(&self) -> usize {
        2 + self.panels.len()
    }
    fn len_of(&self, p: usize) -> usize {
        match p {
            0 => self.remote.len(),
            1 => self.cloud.len(),
            k => self.panels.get(k - 2).map_or(0, |v| v.rows.len()),
        }
    }
    fn top_of(&self, p: usize) -> usize {
        self.top.get(p).copied().unwrap_or(0)
    }
    fn set_top(&mut self, p: usize, v: usize) {
        if self.top.len() <= p {
            self.top.resize(p + 1, 0);
        }
        self.top[p] = v;
    }
    pub fn waiting(&self) -> usize {
        self.remote.iter().filter(|r| r.state == RState::Waiting).count()
    }
    pub fn running(&self) -> usize {
        self.cloud.iter().filter(|c| !c.is_group() && c.state(self.now_ms) == CState::Running).count()
    }

    fn area(&self) -> Rect {
        Rect { x: 0, y: 0, width: self.size.0, height: self.size.1 }
    }

    /// Keep the selection on a row that exists; select the first row when nothing is selected.
    fn reselect(&mut self) {
        let order = self.order();
        match &self.sel {
            Some(s) if order.contains(s) => {}
            _ => self.sel = order.into_iter().next(),
        }
        self.clamp_tops();
        if let Some(s) = self.sel.clone() {
            self.ensure_visible(&s);
        }
    }

    fn caps(&self) -> Vec<usize> {
        layout(self.area(), self).caps
    }

    fn clamp_tops(&mut self) {
        let caps = self.caps();
        for p in 0..self.n_panels() {
            let cap = caps.get(p).copied().unwrap_or(0);
            let v = self.top_of(p).min(self.len_of(p).saturating_sub(cap.max(1)));
            self.set_top(p, v);
        }
    }

    fn ensure_visible(&mut self, sel: &Sel) {
        let p = sel.panel();
        let idx = match sel {
            Sel::Remote(id) => self.remote.iter().position(|r| &r.id == id),
            Sel::Cloud(id) => self.cloud.iter().position(|c| &c.id == id),
            Sel::Panel(k, id) => self.panels.get(*k).and_then(|v| v.rows.iter().position(|r| &r.id == id)),
        };
        let cap = self.caps().get(p).copied().unwrap_or(0).max(1);
        if let Some(i) = idx {
            let top = self.top_of(p);
            if i < top {
                self.set_top(p, i);
            } else if i >= top + cap {
                self.set_top(p, i + 1 - cap);
            }
        }
    }

    fn select(&mut self, sel: Sel) -> Vec<RoomEffect> {
        self.sel = Some(sel.clone());
        self.ensure_visible(&sel);
        self.want_activity(true).into_iter().collect()
    }

    /// At most one activity read in flight. A selected cloud row is read first when it was never read or is stale;
    /// on a timer tick (not user driven) the first row never read is read too. Never a chain of reads.
    fn want_activity(&mut self, user_driven: bool) -> Option<RoomEffect> {
        if self.fetching.is_some() {
            return None;
        }
        let stale = |c: &CloudRow, now: u64| c.fetched_ms.map_or(true, |t| now.saturating_sub(t) >= STALE_MS);
        let mut pick: Option<String> = None;
        if let Some(Sel::Cloud(id)) = &self.sel {
            if let Some(c) = self.cloud_row(id) {
                if stale(c, self.now_ms) {
                    pick = Some(c.id.clone());
                }
            }
        }
        if pick.is_none() && !user_driven {
            pick = self.cloud.iter().find(|c| c.fetched_ms.is_none() && !c.from_list && !c.is_group()).map(|c| c.id.clone());
        }
        let id = pick?;
        self.fetching = Some(id.clone());
        Some(RoomEffect::FetchActivity(id))
    }

    fn apply_remote(&mut self, snap: RemoteSnapshot) {
        self.remote_hosts = snap.hosts.iter().map(|h| one_line(h, 40)).collect();
        self.remote = snap.rows.into_iter().map(clean_remote).collect();
        self.remote_status = snap.status;
        self.remote_at_ms = Some(self.now_ms);
        // A confirmation for a prompt that is gone (answered elsewhere, session ended) is withdrawn.
        if let Mode::Confirm { session, request_id, .. } = &self.mode {
            let live = self.remote_row(session).is_some_and(|r| r.prompts.iter().any(|p| &p.request_id == request_id));
            if !live {
                self.mode = Mode::Browse;
                self.toast = Some(Toast { text: "that prompt is no longer pending".into(), ok: false });
            }
        }
        self.reselect();
    }

    fn apply_cloud(&mut self, snap: CloudSnapshot) {
        let old = std::mem::take(&mut self.cloud);
        self.cloud = snap
            .rows
            .into_iter()
            .map(|n| {
                let mut n = clean_cloud(n);
                if let Some(o) = old.iter().find(|o| o.id == n.id) {
                    if n.fetched_ms.is_none() && n.from_list {
                        // The list knows the row's age and archive state; the older read keeps its detail line.
                        n.last = o.last.clone();
                        n.last_at_ms = n.last_at_ms.max(o.last_at_ms);
                        n.finished = n.finished || o.finished;
                        n.error = o.error.clone();
                        n.fetched_ms = o.fetched_ms;
                    } else if n.fetched_ms.is_none() {
                        n.last = o.last.clone();
                        n.last_at_ms = o.last_at_ms;
                        n.finished = o.finished;
                        n.error = o.error.clone();
                        n.fetched_ms = o.fetched_ms;
                    }
                }
                n
            })
            .collect();
        self.cloud_status = snap.status;
        self.cloud_at_ms = Some(self.now_ms);
        if matches!(&self.mode, Mode::Send { id, .. } if self.cloud_row(id).is_none()) {
            self.mode = Mode::Browse;
            self.toast = Some(Toast { text: "that cloud session is no longer listed".into(), ok: false });
        }
        self.reselect();
    }

    fn apply_panel(&mut self, k: usize, snap: PanelSnapshot) {
        let now = self.now_ms;
        let Some(p) = self.panels.get_mut(k) else { return };
        let mut rows: Vec<PanelRow> = snap.rows.into_iter().map(clean_panel_row).collect();
        rows.sort_by_key(|r| r.rank); // stable: the source's own order within a rank
        p.rows = rows;
        p.status = snap.status;
        p.at_ms = Some(now);
        self.reselect();
    }

    fn apply_activity(&mut self, a: CloudActivity) {
        if let Some(c) = self.cloud.iter_mut().find(|c| c.id == a.id) {
            if c.from_list && a.error.is_none() {
                // The list already told the age and the archive state; a read that finds no event must not blank them.
                c.finished = a.finished || c.finished;
                c.last_at_ms = a.last_at_ms.max(c.last_at_ms);
            } else {
                c.finished = a.finished;
                c.last_at_ms = a.last_at_ms;
            }
            c.last = one_line(&a.last, 120);
            c.error = a.error.map(|e| one_line(&e, 80));
            c.fetched_ms = Some(self.now_ms);
        }
        if self.fetching.as_deref() == Some(a.id.as_str()) {
            self.fetching = None;
        }
    }
}

fn clean_remote(mut r: RemoteRow) -> RemoteRow {
    r.id = one_line(&r.id, 100);
    r.host = one_line(&r.host, 40);
    r.state = match r.state {
        RState::Unreadable(e) => RState::Unreadable(one_line(&e, 80)),
        RState::HostDown(e) => RState::HostDown(one_line(&e, 80)),
        s => s,
    };
    for p in &mut r.prompts {
        p.request_id = one_line(&p.request_id, 100);
        p.tool = one_line(&p.tool, 60);
    }
    r
}

/// Text from a panel's source is untrusted: one line each, control characters gone, bounded.
fn clean_panel_row(mut r: PanelRow) -> PanelRow {
    r.id = one_line(&r.id, 200);
    r.name = one_line(&r.name, 120);
    r.cols = r.cols.into_iter().map(|mut c| {
        c.text = c.text.cleaned(200);
        c
    }).collect();
    r.detail = r.detail.into_iter().map(|line| line.into_iter().map(|mut p| {
        p.text = p.text.cleaned(240);
        p
    }).collect()).collect();
    r
}

fn clean_cloud(mut c: CloudRow) -> CloudRow {
    c.id = one_line(&c.id, 100);
    c.title = one_line(&c.title, 120);
    c.repo = one_line(&c.repo, 80);
    c.last = one_line(&c.last, 120);
    c.error = c.error.map(|e| one_line(&e, 80));
    if let RowKind::Routine { schedule, .. } = &mut c.kind {
        *schedule = one_line(schedule, 60);
    }
    c
}

// ---- messages, keys, effects --------------------------------------------------------------------------------------

#[derive(Debug)]
pub enum RoomMsg {
    Key(KeyEvent),
    /// A left click at (column, row).
    Click(u16, u16),
    /// The wheel at (column, row): +1 up, -1 down.
    Wheel(u16, u16, i8),
    Paste(String),
    Resize(u16, u16),
    /// The wall clock, unix milliseconds.
    Tick(u64),
    Remote(RemoteSnapshot),
    Cloud(CloudSnapshot),
    Activity(CloudActivity),
    /// A read of the extra panel with this index.
    Panel(usize, PanelSnapshot),
    /// An action finished (answer, send, or coming back from a drop-in).
    Done(Toast),
}

#[derive(Debug, PartialEq)]
pub enum RoomEffect {
    Quit,
    /// Leave the screen, hand the terminal to the session's own client, come back.
    DropIn(Sel),
    Answer { session: String, request_id: String, allow: bool },
    CloudSend { id: String, text: String },
    FetchActivity(String),
    /// Read both panels now.
    Refresh,
    /// Show (true) or hide (false) Routines in the backend, then read the Cloud panel again.
    Routines(bool),
}

/// What a key press means.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Move(isize),
    First,
    Last,
    Page(isize),
    OtherPanel,
    DropIn,
    Approve,
    Deny,
    Send,
    Refresh,
    ToggleRoutines,
    Quit,
    Confirm,
    Cancel,
    Char(char),
    Backspace,
    ClearLine,
    Submit,
    None,
}

pub fn map_key(mode: &Mode, k: KeyEvent) -> Act {
    if k.kind == KeyEventKind::Release {
        return Act::None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match mode {
        Mode::Browse => match k.code {
            KeyCode::Char('c') if ctrl => Act::Quit,
            KeyCode::Up | KeyCode::Char('k') => Act::Move(-1),
            KeyCode::Down | KeyCode::Char('j') => Act::Move(1),
            KeyCode::PageUp => Act::Page(-1),
            KeyCode::PageDown => Act::Page(1),
            KeyCode::Home | KeyCode::Char('g') => Act::First,
            KeyCode::End | KeyCode::Char('G') => Act::Last,
            KeyCode::Tab | KeyCode::BackTab => Act::OtherPanel,
            KeyCode::Enter => Act::DropIn,
            KeyCode::Char('a') if !ctrl => Act::Approve,
            KeyCode::Char('d') if !ctrl => Act::Deny,
            KeyCode::Char('s') if !ctrl => Act::Send,
            KeyCode::Char('r') if !ctrl => Act::Refresh,
            KeyCode::Char('R') if !ctrl => Act::ToggleRoutines,
            KeyCode::Char('q') | KeyCode::Esc => Act::Quit,
            _ => Act::None,
        },
        // Only `y` confirms: an Enter pressed by habit must never answer a permission prompt.
        Mode::Confirm { .. } => match k.code {
            KeyCode::Char('c') if ctrl => Act::Cancel,
            KeyCode::Char('y') | KeyCode::Char('Y') => Act::Confirm,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => Act::Cancel,
            _ => Act::None,
        },
        Mode::Send { .. } => match k.code {
            KeyCode::Char('c') if ctrl => Act::Cancel,
            KeyCode::Char('u') if ctrl => Act::ClearLine,
            KeyCode::Esc => Act::Cancel,
            KeyCode::Enter => Act::Submit,
            KeyCode::Backspace => Act::Backspace,
            KeyCode::Char(c) if !ctrl => Act::Char(c),
            _ => Act::None,
        },
    }
}

fn toast(text: impl Into<String>, ok: bool) -> Option<Toast> {
    Some(Toast { text: text.into(), ok })
}

pub fn reduce(mut s: RoomState, m: RoomMsg) -> (RoomState, Vec<RoomEffect>) {
    let mut fx: Vec<RoomEffect> = Vec::new();
    match m {
        RoomMsg::Tick(now) => s.now_ms = now,
        RoomMsg::Resize(w, h) => {
            s.size = (w, h);
            s.clamp_tops();
            if let Some(sel) = s.sel.clone() {
                s.ensure_visible(&sel);
            }
        }
        RoomMsg::Remote(snap) => s.apply_remote(snap),
        RoomMsg::Cloud(snap) => {
            s.apply_cloud(snap);
            fx.extend(s.want_activity(false));
        }
        RoomMsg::Panel(k, snap) => s.apply_panel(k, snap),
        RoomMsg::Activity(a) => {
            s.apply_activity(a);
            fx.extend(s.want_activity(true));
        }
        RoomMsg::Done(t) => s.toast = Some(t),
        RoomMsg::Paste(t) => {
            if let Mode::Send { text, .. } = &mut s.mode {
                let add = clean(&t, SEND_MAX).replace('\n', " ");
                if text.chars().count() + add.chars().count() <= SEND_MAX {
                    text.push_str(&add);
                }
            }
        }
        RoomMsg::Click(x, y) if layout(s.area(), &s).on_close(x, y) => fx.push(RoomEffect::Quit),
        RoomMsg::Click(x, y) => {
            if s.mode == Mode::Browse {
                s.toast = None;
                let hit = layout(s.area(), &s).hit(x, y).cloned();
                if let Some(sel) = hit {
                    // A click on a row selects it; a click on the selected row drops in.
                    if s.sel.as_ref() == Some(&sel) {
                        fx.push(RoomEffect::DropIn(sel));
                    } else {
                        fx.extend(s.select(sel));
                    }
                }
            }
        }
        RoomMsg::Wheel(x, y, dir) => {
            if s.mode == Mode::Browse {
                let l = layout(s.area(), &s);
                let panel = if contains(l.remote, x, y) {
                    0
                } else if contains(l.cloud, x, y) {
                    1
                } else if let Some(k) = l.panels.iter().position(|r| contains(*r, x, y)) {
                    2 + k
                } else {
                    s.sel.as_ref().map_or(0, Sel::panel)
                };
                let max = s.len_of(panel).saturating_sub(l.caps.get(panel).copied().unwrap_or(0).max(1));
                let top = s.top_of(panel);
                s.set_top(panel, if dir > 0 { top.saturating_sub(1) } else { (top + 1).min(max) });
            }
        }
        RoomMsg::Key(k) => {
            let act = map_key(&s.mode, k);
            if act != Act::None {
                s.toast = None;
            }
            fx.extend(act_on(&mut s, act));
        }
    }
    (s, fx)
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn act_on(s: &mut RoomState, act: Act) -> Vec<RoomEffect> {
    let mut fx = Vec::new();
    match act {
        Act::None => {}
        Act::Move(d) => fx.extend(step(s, d)),
        Act::Page(d) => {
            let n = s.caps().iter().copied().max().unwrap_or(1).max(2) as isize - 1;
            fx.extend(step(s, d * n));
        }
        Act::First => {
            if let Some(f) = s.order().into_iter().next() {
                fx.extend(s.select(f));
            }
        }
        Act::Last => {
            if let Some(l) = s.order().into_iter().last() {
                fx.extend(s.select(l));
            }
        }
        Act::OtherPanel => {
            // Next panel that has rows: Remote Control -> Cloud -> each extra panel -> Remote Control.
            let n = s.n_panels();
            let cur = s.sel.as_ref().map_or(n - 1, Sel::panel);
            let first = |p: usize| match p {
                0 => s.remote.iter().find(|r| !r.is_host_down()).map(|r| Sel::Remote(r.id.clone())),
                1 => s.cloud.iter().find(|c| !c.is_group()).map(|c| Sel::Cloud(c.id.clone())),
                k => s.panels.get(k - 2).and_then(|v| v.rows.first()).map(|r| Sel::Panel(k - 2, r.id.clone())),
            };
            if let Some(p) = (1..=n).map(|k| (cur + k) % n).find_map(first) {
                fx.extend(s.select(p));
            }
        }
        Act::DropIn => match s.sel.clone() {
            Some(sel) => fx.push(RoomEffect::DropIn(sel)),
            None => s.toast = toast("nothing to drop into yet", false),
        },
        Act::Approve | Act::Deny => {
            let allow = act == Act::Approve;
            match s.sel.clone() {
                Some(Sel::Cloud(_)) => s.toast = toast("Cloud sessions raise no permission prompts, so there is nothing to approve.", false),
                Some(Sel::Panel(k, _)) => s.toast = toast(s.panels.get(k).map_or(String::new(), |p| p.spec.approve_hint.clone()), false),
                Some(Sel::Remote(id)) => match s.remote_row(&id).and_then(|r| r.prompts.first().map(|p| (r.id.clone(), p.clone()))) {
                    Some((session, p)) => {
                        s.mode = Mode::Confirm { allow, session, request_id: p.request_id.clone(), args: tool_args(&p.tool, &p.input, 200), tool: p.tool };
                    }
                    None => s.toast = toast("that session is not waiting on a prompt", false),
                },
                None => s.toast = toast("select a session first", false),
            }
        }
        Act::Send => match s.sel.clone() {
            Some(Sel::Cloud(id)) => s.mode = Mode::Send { id, text: String::new() },
            Some(Sel::Remote(_)) => s.toast = toast("send is for cloud sessions; Enter drops into a Remote Control session", false),
            Some(Sel::Panel(k, _)) => s.toast = toast(s.panels.get(k).map_or(String::new(), |p| p.spec.send_hint.clone()), false),
            None => s.toast = toast("select a session first", false),
        },
        Act::ToggleRoutines => {
            if !s.routines_available {
                s.toast = toast("routines are not available here", false);
            } else {
                s.routines = !s.routines;
                if !s.routines {
                    // Gone at once; the next listing would drop them anyway.
                    s.cloud.retain(|c| !(c.kind == RowKind::Run || c.id.starts_with("routine~") || c.id.starts_with("note~routines") || c.id.starts_with("note~runs~")));
                    s.reselect();
                }
                s.toast = toast(if s.routines { "routines shown" } else { "routines hidden" }, true);
                fx.push(RoomEffect::Routines(s.routines));
            }
        }
        Act::Refresh => {
            s.toast = toast("refreshing", true);
            fx.push(RoomEffect::Refresh);
        }
        Act::Quit => fx.push(RoomEffect::Quit),
        Act::Confirm => {
            if let Mode::Confirm { allow, session, request_id, tool, .. } = std::mem::replace(&mut s.mode, Mode::Browse) {
                s.toast = toast(format!("{} {} ...", if allow { "approving" } else { "denying" }, tool), true);
                fx.push(RoomEffect::Answer { session, request_id, allow });
            }
        }
        Act::Cancel => s.mode = Mode::Browse,
        Act::Char(c) => {
            if let Mode::Send { text, .. } = &mut s.mode {
                if !c.is_control() && text.chars().count() < SEND_MAX {
                    text.push(c);
                }
            }
        }
        Act::Backspace => {
            if let Mode::Send { text, .. } = &mut s.mode {
                text.pop();
            }
        }
        Act::ClearLine => {
            if let Mode::Send { text, .. } = &mut s.mode {
                text.clear();
            }
        }
        Act::Submit => {
            if let Mode::Send { id, text } = std::mem::replace(&mut s.mode, Mode::Browse) {
                if text.trim().is_empty() {
                    s.mode = Mode::Send { id, text };
                } else {
                    s.toast = toast("sending ...", true);
                    fx.push(RoomEffect::CloudSend { id, text });
                }
            }
        }
    }
    fx
}

fn step(s: &mut RoomState, d: isize) -> Vec<RoomEffect> {
    let order = s.order();
    if order.is_empty() {
        return vec![];
    }
    let cur = s.sel.as_ref().and_then(|c| order.iter().position(|o| o == c)).unwrap_or(0) as isize;
    let next = (cur + d).clamp(0, order.len() as isize - 1) as usize;
    s.select(order[next].clone())
}

// ---- layout (also the click map) ----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Layout {
    pub small: bool,
    pub header: Rect,
    pub remote: Rect,
    pub cloud: Rect,
    /// One rectangle per extra panel (zero height when it is not drawn).
    pub panels: Vec<Rect>,
    pub detail: Option<Rect>,
    pub status: Rect,
    pub footer: Rect,
    /// The clickable close affordance at the right end of the header.
    pub close: Rect,
    /// Rows that fit in each panel: Remote Control, Cloud, then the extra panels.
    pub caps: Vec<usize>,
    /// Each visible row and what it is.
    pub rows: Vec<(Rect, Sel)>,
}

impl Layout {
    pub fn on_close(&self, x: u16, y: u16) -> bool {
        contains(self.close, x, y)
    }
    /// Which row a click at (x, y) landed on.
    pub fn hit(&self, x: u16, y: u16) -> Option<&Sel> {
        self.rows.iter().find(|(r, _)| contains(*r, x, y)).map(|(_, s)| s)
    }
}

fn split_even(total: u16, nr: u16, nc: u16) -> (u16, u16) {
    let r = ((total as u32 * nr as u32 + (nr as u32 + nc as u32) / 2) / (nr as u32 + nc as u32).max(1)) as u16;
    let r = r.clamp(3, total.saturating_sub(3).max(3));
    (r, total.saturating_sub(r))
}

/// A terminal needs at least this many rows for the logo band (the room's own minimum plus the band's two extra rows plus lists).
pub const BAND_MIN_H: u16 = 18;
/// The band's text starts at this column; the logo image sits at column 1, 6 cells wide, 3 rows tall.
pub const BAND_TEXT_COL: u16 = 8;

pub fn layout(area: Rect, s: &RoomState) -> Layout {
    if s.band && area.height >= BAND_MIN_H && area.width >= MIN_W {
        // Lay the room out two rows lower, then give the header the three rows on top (the third is the old header row).
        let inner = Rect { x: area.x, y: area.y + 2, width: area.width, height: area.height - 2 };
        let mut l = layout_core(inner, s);
        if !l.small {
            l.header = Rect { x: area.x, y: area.y, width: area.width, height: 3 };
            l.close = Rect { x: area.x + area.width - CLOSE_W, y: area.y, width: CLOSE_W, height: 1 };
        }
        return l;
    }
    layout_core(area, s)
}

fn layout_core(area: Rect, s: &RoomState) -> Layout {
    let (w, h) = (area.width, area.height);
    let empty = Rect { x: area.x, y: area.y, width: 0, height: 0 };
    let line = |y: u16| Rect { x: area.x, y: area.y + y.min(h.saturating_sub(1)), width: w, height: 1.min(h) };
    if w < MIN_W || h < MIN_H {
        return Layout { small: true, header: line(0), remote: empty, cloud: empty, panels: vec![empty; s.panels.len()], detail: None, status: empty, footer: empty, close: empty, caps: vec![0; s.n_panels()], rows: vec![] };
    }
    let avail = h - 3;
    // Remote Control and Cloud always; an extra panel when it answered and each can have its 3 rows.
    let mut panels = vec![0usize, 1];
    for (k, p) in s.panels.iter().enumerate() {
        if p.visible() {
            panels.push(2 + k);
        }
    }
    while panels.len() > 2 && (3 * panels.len()) as u16 > avail {
        panels.pop();
    }
    if panels.len() > 2 {
        return layout_n(area, s, avail, &panels);
    }
    // renderSVG spacing: a blank row after each of the two panels, when the height allows.
    let g: u16 = if s.air && avail >= 10 + DETAIL_H + 2 + 4 { 1 } else { 0 };
    let avail = avail - 2 * g;
    let nr = s.remote.len().max(1) as u16 + 2;
    let nc = s.cloud.len().max(1) as u16 + 2;
    let (mut rh, mut ch, dh);
    if nr + nc + DETAIL_H <= avail {
        rh = nr;
        ch = nc;
        dh = avail - nr - nc;
    } else if avail >= 10 + DETAIL_H {
        (rh, ch) = split_even(avail - DETAIL_H, nr, nc);
        dh = DETAIL_H;
    } else if nr + nc + 4 <= avail {
        // every row fits and the detail panel gets what is left (two lines at least)
        rh = nr;
        ch = nc;
        dh = avail - nr - nc;
    } else {
        // short terminal: the lists come first, the detail panel is left out
        (rh, ch) = split_even(avail, nr, nc);
        dh = 0;
    }
    // Spare room beyond a roomy detail panel goes back to the lists.
    let dh = if dh > DETAIL_H {
        let extra = dh - DETAIL_H;
        let (er, ec) = (extra * nr / (nr + nc), extra - extra * nr / (nr + nc));
        rh += er;
        ch += ec;
        DETAIL_H
    } else {
        dh
    };
    let rect = |y: u16, hh: u16| Rect { x: area.x, y: area.y + y, width: w, height: hh };
    let remote = rect(1, rh);
    let cloud = rect(1 + rh + g, ch);
    let detail = if dh >= 4 { Some(rect(1 + rh + g + ch + g, dh)) } else { None };
    let status = rect(h - 2, 1);
    let footer = rect(h - 1, 1);
    let mut caps = vec![0usize; s.n_panels()];
    caps[0] = rh.saturating_sub(2) as usize;
    caps[1] = ch.saturating_sub(2) as usize;
    let mut rows = Vec::new();
    for (p, panel, ids) in [(0usize, remote, s.remote.iter().map(|r| (!r.is_host_down()).then(|| Sel::Remote(r.id.clone()))).collect::<Vec<_>>()), (1, cloud, s.cloud.iter().map(|c| (!c.is_group()).then(|| Sel::Cloud(c.id.clone()))).collect())] {
        for (n, sel) in ids.into_iter().skip(s.top_of(p)).take(caps[p]).enumerate() {
            let Some(sel) = sel else { continue };
            rows.push((Rect { x: panel.x + 1, y: panel.y + 1 + n as u16, width: panel.width.saturating_sub(2), height: 1 }, sel));
        }
    }
    let close = Rect { x: area.x + w - CLOSE_W, y: area.y, width: CLOSE_W, height: 1 };
    Layout { small: false, header: rect(0, 1), remote, cloud, panels: vec![empty; s.panels.len()], detail, status, footer, close, caps, rows }
}

/// Heights for `want.len()` lists within `total`: what each wants when it all fits, otherwise at least 3 each and the
/// rest one row at a time to the list furthest below what it wants.
pub fn alloc_n(total: u16, want: &[u16]) -> Vec<u16> {
    if want.iter().map(|&x| x as u32).sum::<u32>() <= total as u32 {
        return want.to_vec();
    }
    let n = want.len();
    let mut h = vec![3u16; n];
    let mut left = total.saturating_sub(3 * n as u16);
    while left > 0 {
        let i = (0..n).max_by_key(|&i| (want[i].saturating_sub(h[i]), std::cmp::Reverse(i))).unwrap_or(0);
        if want[i] <= h[i] {
            break;
        }
        h[i] += 1;
        left -= 1;
    }
    h
}

/// Heights for three lists (kept for callers and tests that use the three-panel form).
pub fn alloc3(total: u16, want: [u16; 3]) -> [u16; 3] {
    let v = alloc_n(total, &want);
    [v[0], v[1], v[2]]
}

/// The layout with more than two panels. `panels` holds the visible ones in order: 0 Remote Control, 1 Cloud, then
/// 2 + k for extra panel k; then comes the detail panel.
fn layout_n(area: Rect, s: &RoomState, avail: u16, panels: &[usize]) -> Layout {
    let (w, h) = (area.width, area.height);
    let n = panels.len();
    let g: u16 = if s.air && avail >= 3 * n as u16 + DETAIL_H + 2 * n as u16 + 1 { 1 } else { 0 };
    let avail = avail - n as u16 * g;
    let want: Vec<u16> = panels.iter().map(|&p| s.len_of(p).max(1) as u16 + 2).collect();
    let wsum: u16 = want.iter().sum();
    let budget = if wsum + DETAIL_H <= avail || avail < 3 * n as u16 + DETAIL_H { avail } else { avail - DETAIL_H };
    let mut hs = alloc_n(budget, &want);
    let mut dh = avail.saturating_sub(hs.iter().sum::<u16>());
    if dh > DETAIL_H {
        // Spare room beyond a roomy detail panel goes back to the lists.
        let mut extra = dh - DETAIL_H;
        let mut i = 0;
        while extra > 0 {
            hs[i % n] += 1;
            extra -= 1;
            i += 1;
        }
        dh = DETAIL_H;
    }
    let rect = |y: u16, hh: u16| Rect { x: area.x, y: area.y + y, width: w, height: hh };
    let empty = Rect { x: area.x, y: area.y, width: 0, height: 0 };
    let mut rects = vec![empty; s.n_panels()];
    let mut caps = vec![0usize; s.n_panels()];
    let mut y = 1u16;
    for (k, &p) in panels.iter().enumerate() {
        rects[p] = rect(y, hs[k]);
        caps[p] = hs[k].saturating_sub(2) as usize;
        y += hs[k] + g;
    }
    let detail = if dh >= 4 { Some(rect(y, dh)) } else { None };
    let mut rows = Vec::new();
    for &p in panels {
        let ids: Vec<Option<Sel>> = match p {
            0 => s.remote.iter().map(|r| (!r.is_host_down()).then(|| Sel::Remote(r.id.clone()))).collect(),
            1 => s.cloud.iter().map(|c| (!c.is_group()).then(|| Sel::Cloud(c.id.clone()))).collect(),
            k => s.panels[k - 2].rows.iter().map(|r| Some(Sel::Panel(k - 2, r.id.clone()))).collect(),
        };
        let panel = rects[p];
        for (k, sel) in ids.into_iter().skip(s.top_of(p)).take(caps[p]).enumerate() {
            let Some(sel) = sel else { continue };
            rows.push((Rect { x: panel.x + 1, y: panel.y + 1 + k as u16, width: panel.width.saturating_sub(2), height: 1 }, sel));
        }
    }
    let close = Rect { x: area.x + w - CLOSE_W, y: area.y, width: CLOSE_W, height: 1 };
    Layout { small: false, header: rect(0, 1), remote: rects[0], cloud: rects[1], panels: rects[2..].to_vec(), detail, status: rect(h - 2, 1), footer: rect(h - 1, 1), close, caps, rows }
}

// ---- drawing ------------------------------------------------------------------------------------------------------

/// `12s`, `5m`, `3h`, `2d` since `at_ms`.
pub fn ago(now_ms: u64, at_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    if s < 90 {
        format!("{s}s")
    } else if s < 5400 {
        format!("{}m", s / 60)
    } else if s < 129_600 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// `text` cut or padded to exactly `w` display columns.
fn cell(text: &str, w: usize) -> String {
    let t = truncate(text, w);
    let pad = w.saturating_sub(width(&t));
    format!("{t}{}", " ".repeat(pad))
}

fn lead(sel: bool, th: &Theme) -> Span<'static> {
    if sel {
        sp(format!("{} ", th.prompt_glyph()), th.accent())
    } else {
        sp("  ", Style::default())
    }
}

fn finish_row(mut spans: Vec<Span<'static>>, w: usize, sel: bool, th: &Theme) -> Line<'static> {
    let used: usize = spans.iter().map(|s| width(&s.content)).sum();
    if used < w {
        spans.push(sp(" ".repeat(w - used), Style::default()));
    }
    let mut line = Line::from(spans);
    if sel {
        let bg = th.row_bg();
        line = Line::from(line.spans.into_iter().map(|s| Span::styled(s.content, s.style.patch(bg))).collect::<Vec<_>>());
    }
    line
}

/// Width of the host column (shown only when the room lists more than one machine).
pub const HOST_W: usize = 12;

pub fn remote_line(r: &RemoteRow, w: usize, sel: bool, th: &Theme) -> Line<'static> {
    let (badge, bstyle) = match &r.state {
        RState::Waiting => (th.pause_glyph(), th.warn()),
        RState::Idle => (th.ring(), th.dim()),
        RState::Unreadable(_) | RState::HostDown(_) => (th.cross(), th.error()),
        RState::Unknown => ("?", th.dim()),
    };
    let mut spans = vec![lead(sel, th), sp(cell(badge, 3), bstyle)];
    let mut body = w.saturating_sub(5);
    let column = !r.host.is_empty() && body >= 30;
    if column {
        spans.push(sp(format!("{} ", cell(&r.host, HOST_W)), th.accent2()));
        body -= HOST_W + 1;
    }
    if let RState::HostDown(e) = &r.state {
        let text = if column { e.clone() } else { format!("{}: {e}", r.host) };
        spans.push(sp(truncate(&text, body), th.error()));
        return finish_row(spans, w, false, th);
    }
    let id_w = if body < 40 { body.min(width(&r.id)).max(8).min(body) } else { width(&r.id).min(body.saturating_sub(24)).max(14).min(body) };
    let id_style = if sel { th.bold() } else { Style::default() };
    spans.push(sp(cell(&r.id, id_w), id_style));
    let mut left = body.saturating_sub(id_w);
    if left >= 44 {
        let pid = r.pid.map(|p| format!("pid {p}")).unwrap_or_default();
        spans.push(sp(format!("  {}", cell(&pid, 9)), th.dim()));
        left -= 11;
    }
    if left >= 8 {
        let (text, style) = match &r.state {
            RState::Waiting => match r.prompts.first() {
                Some(p) => {
                    let more = if r.prompts.len() > 1 { format!(" (+{})", r.prompts.len() - 1) } else { String::new() };
                    (format!("waiting on {} {}{}", p.tool, tool_args(&p.tool, &p.input, 60), more).replace("  ", " "), th.warn())
                }
                None => ("waiting".to_string(), th.warn()),
            },
            RState::Idle => ("idle".to_string(), th.dim()),
            RState::Unreadable(e) | RState::HostDown(e) => (format!("unreadable: {e}"), th.error()),
            RState::Unknown => ("unknown (no claude.ai login)".to_string(), th.dim()),
        };
        spans.push(sp(format!("  {}", truncate(text.trim_end(), left - 2)), style));
    }
    finish_row(spans, w, sel, th)
}

fn cstate_look(st: CState, th: &Theme) -> (&'static str, &'static str, Style) {
    match st {
        CState::Running => (th.dot(), "running", th.ok()),
        CState::Idle => (th.ring(), "idle", th.dim()),
        CState::Finished => (th.done_mark(), "finished", th.dim()),
        CState::Unreadable => (th.cross(), "unreadable", th.error()),
        CState::Unknown => (th.notice_glyph(), "...", th.dim()),
    }
}

pub fn cloud_line(c: &CloudRow, now_ms: u64, w: usize, sel: bool, th: &Theme) -> Line<'static> {
    match &c.kind {
        RowKind::Routine { schedule, enabled } => {
            // One plain dim line of text: no glyph column, the name once.
            let sep = format!(" {} ", th.notice_glyph());
            let name = if c.title.is_empty() { "(unnamed)" } else { c.title.as_str() };
            let line = format!("routine{sep}{name}{sep}{schedule}{sep}{}", if *enabled { "on" } else { "off" });
            let spans = vec![lead(false, th), sp(truncate(&line, w.saturating_sub(2)), th.dim())];
            return finish_row(spans, w, false, th);
        }
        RowKind::Note => {
            let spans = vec![lead(false, th), sp(truncate(&c.title, w.saturating_sub(3)), th.dim())];
            return finish_row(spans, w, false, th);
        }
        _ => {}
    }
    let st = c.state(now_ms);
    let (glyph, word, style) = cstate_look(st, th);
    let mut spans = vec![lead(sel, th), sp(cell(glyph, 3), style)];
    let mut left = w.saturating_sub(5);
    if left >= 40 {
        spans.push(sp(cell(word, 10), style));
        left -= 10;
        let age = c.last_at_ms.map(|at| ago(now_ms, at)).unwrap_or_else(|| "-".into());
        spans.push(sp(format!("{:>4}  ", truncate(&age, 4)), th.dim()));
        left = left.saturating_sub(6);
    }
    if left >= 50 {
        let rw = (left / 4).clamp(10, 22);
        spans.push(sp(cell(&c.repo, rw), th.accent2()));
        spans.push(sp("  ", Style::default()));
        left -= rw + 2;
    }
    let last = if left >= 84 && !c.last.is_empty() { Some(format!("  {} {}", th.sep().trim(), c.last)) } else { None };
    let title_w = match &last {
        Some(l) => left.saturating_sub(width(l).min(left / 2)),
        None => left,
    };
    let title = if c.title.is_empty() { "(untitled)" } else { &c.title };
    let ts = truncate(title, title_w);
    let tw = width(&ts);
    spans.push(sp(ts, if sel { th.bold() } else { Style::default() }));
    if let Some(l) = last {
        spans.push(sp(truncate(&l, left.saturating_sub(tw)), th.dim()));
    }
    finish_row(spans, w, sel, th)
}

fn panel_block(title: Vec<Span<'static>>, right: Option<String>, focused: bool, th: &Theme) -> Block<'static> {
    let border_style = th.panel_border(focused);
    let mut b = Block::new().borders(Borders::ALL).border_set(border_set(th)).border_style(border_style).title_top(Line::from(title));
    if let Some(r) = right {
        b = b.title_top(Line::from(Span::styled(format!(" {r} "), th.dim())).right_aligned());
    }
    b
}

fn range_note(top: usize, cap: usize, len: usize) -> Option<String> {
    if len > cap {
        Some(format!("{}-{} of {}", top + 1, (top + cap).min(len), len))
    } else {
        None
    }
}

fn message_line(text: &str, style: Style, w: usize) -> Line<'static> {
    Line::from(sp(format!("  {}", truncate(text, w.saturating_sub(2))), style))
}

fn draw_remote(f: &mut Frame, l: &Layout, s: &RoomState, th: &Theme) {
    let focused = matches!(s.sel, Some(Sel::Remote(_)));
    let waiting = s.waiting();
    let mut title = vec![sp(" Remote Control ", th.panel_title(focused))];
    if waiting > 0 {
        title.push(sp(format!("{} {} waiting ", th.pause_glyph(), waiting), th.warn()));
    }
    // Several machines: the sessions per host, and an x for a host that could not be listed.
    let mut per_host: Vec<(&str, usize, bool)> = s.remote_hosts.iter().map(|h| (h.as_str(), 0, false)).collect();
    for r in s.remote.iter().filter(|r| !r.host.is_empty()) {
        let down = r.is_host_down();
        match per_host.iter_mut().find(|h| h.0 == r.host) {
            Some(h) => { h.1 += usize::from(!down); h.2 |= down; }
            None => per_host.push((&r.host, usize::from(!down), down)),
        }
    }
    if !per_host.is_empty() {
        let parts: Vec<String> = per_host.iter().map(|(h, n, down)| if *down { format!("{h} {}", th.cross()) } else { format!("{h} {n}") }).collect();
        title.push(sp(format!("{} ", parts.join(&format!(" {} ", th.sep().trim()))), th.dim()));
    }
    let block = panel_block(title, range_note(s.top[0], l.caps[0], s.remote.len()), focused, th);
    let inner = block.inner(l.remote);
    f.render_widget(block, l.remote);
    let w = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if s.remote.is_empty() {
        lines.push(match &s.remote_status {
            RemoteStatus::Loading => message_line("reading ...", th.dim(), w),
            RemoteStatus::Failed(e) => message_line(&format!("could not list sessions: {e}"), th.error(), w),
            RemoteStatus::NoLogin => message_line("no claude.ai login: sign in to Claude Code, or set REMOTE_SESSIONS_TOKEN", th.warn(), w),
            RemoteStatus::Ready if !s.remote_hosts.is_empty() => message_line("no Remote Control session on these machines (claude remote-control)", th.dim(), w),
            RemoteStatus::Ready => message_line("no Remote Control session is served here (claude remote-control)", th.dim(), w),
        });
    } else {
        for r in s.remote.iter().skip(s.top[0]).take(l.caps[0]) {
            lines.push(remote_line(r, w, s.sel == Some(Sel::Remote(r.id.clone())), th));
        }
    }
    f.render_widget(Paragraph::new(lines).style(th.body()), inner);
}

fn draw_cloud(f: &mut Frame, l: &Layout, s: &RoomState, th: &Theme) {
    let focused = matches!(s.sel, Some(Sel::Cloud(_)));
    let running = s.running();
    let mut title = vec![sp(" Cloud ", th.panel_title(focused))];
    if running > 0 {
        title.push(sp(format!("{} {} running ", th.dot(), running), th.ok()));
    }
    let shown = s.cloud.iter().filter(|c| c.kind == RowKind::Session).count();
    let more = match s.cloud_status {
        CloudStatus::Ready { tracked } if tracked > shown => Some(format!("+{} more tracked", tracked - shown)),
        _ => None,
    };
    let right = range_note(s.top[1], l.caps[1], s.cloud.len()).or(more);
    let block = panel_block(title, right, focused, th);
    let inner = block.inner(l.cloud);
    f.render_widget(block, l.cloud);
    let w = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if s.cloud.is_empty() {
        lines.push(match &s.cloud_status {
            CloudStatus::Loading => message_line("reading ...", th.dim(), w),
            CloudStatus::Absent => message_line("Install cloud-sessions to see cloud sessions here.", th.dim(), w),
            CloudStatus::Failed(e) => message_line(&format!("cloud-sessions could not be read: {e}"), th.error(), w),
            CloudStatus::Ready { .. } => message_line("no cloud session is tracked (cloud-sessions track <link>)", th.dim(), w),
        });
    } else {
        for c in s.cloud.iter().skip(s.top[1]).take(l.caps[1]) {
            lines.push(cloud_line(c, s.now_ms, w, s.sel == Some(Sel::Cloud(c.id.clone())), th));
        }
    }
    f.render_widget(Paragraph::new(lines).style(th.body()), inner);
}

fn tone_style(t: Tone, th: &Theme) -> Style {
    match t {
        Tone::Ok => th.ok(),
        Tone::Warn => th.warn(),
        Tone::Error => th.error(),
        Tone::Dim => th.dim(),
    }
}

fn glyph_of(g: Glyph, th: &Theme) -> &'static str {
    match g {
        Glyph::Dot => th.dot(),
        Glyph::Ring => th.ring(),
        Glyph::Done => th.done_mark(),
        Glyph::Cross => th.cross(),
        Glyph::Pause => th.pause_glyph(),
        Glyph::Notice => th.notice_glyph(),
    }
}

fn ink_style(ink: Ink, tone: Tone, th: &Theme) -> Style {
    match ink {
        Ink::Tone => tone_style(tone, th),
        Ink::Accent => th.accent(),
        Ink::Accent2 => th.accent2(),
        Ink::Dim => th.dim(),
        Ink::Bold => th.bold(),
        Ink::Plain => Style::default(),
    }
}

/// One row of an extra panel: the state glyph, the name, then each column that fits.
pub fn panel_line(r: &PanelRow, fit: NameFit, w: usize, sel: bool, th: &Theme) -> Line<'static> {
    let style = tone_style(r.tone, th);
    let mut spans = vec![lead(sel, th), sp(cell(glyph_of(r.glyph, th), 3), style)];
    let mut left = w.saturating_sub(5);
    let nw = match fit {
        NameFit::Fixed(n) => n.min(left),
        NameFit::Clamp(lo, hi) => width(&r.name).clamp(lo, hi).min(left),
    };
    spans.push(sp(cell(&r.name, nw), if sel { th.bold() } else { Style::default() }));
    left = left.saturating_sub(nw);
    for c in &r.cols {
        if left < c.min_left || (c.skip_empty && c.text.is_empty()) {
            continue;
        }
        let text = c.text.render(th);
        let st = ink_style(c.ink, r.tone, th);
        match c.fit {
            Fit::Fixed(n) => {
                spans.push(sp(format!("  {}", cell(&text, n)), st));
                left = left.saturating_sub(n + 2);
            }
            Fit::Share { div, min, max } => {
                let n = (left / div.max(1)).clamp(min, max);
                spans.push(sp(format!("  {}", cell(&text, n)), st));
                left = left.saturating_sub(n + 2);
            }
            Fit::Rest => {
                spans.push(sp(format!("  {}", truncate(&text, left.saturating_sub(2))), st));
                left = 0;
            }
        }
    }
    finish_row(spans, w, sel, th)
}

fn draw_panel(f: &mut Frame, l: &Layout, s: &RoomState, k: usize, th: &Theme) {
    let (Some(v), Some(&area)) = (s.panels.get(k), l.panels.get(k)) else { return };
    let p = 2 + k;
    let focused = matches!(s.sel, Some(Sel::Panel(j, _)) if j == k);
    let mut title = vec![sp(format!(" {} ", v.spec.title), th.panel_title(focused))];
    if let Some((tone, glyph, word)) = &v.spec.badge {
        let n = v.rows.iter().filter(|r| r.tone == *tone).count();
        if n > 0 {
            title.push(sp(format!("{} {} {} ", glyph_of(*glyph, th), n, word), tone_style(*tone, th)));
        }
    }
    let cap = l.caps.get(p).copied().unwrap_or(0);
    let right = range_note(s.top_of(p), cap, v.rows.len()).or_else(|| (!v.rows.is_empty()).then(|| v.spec.tag.clone()));
    let block = panel_block(title, right, focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let w = inner.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if v.rows.is_empty() {
        lines.push(match &v.status {
            PanelStatus::Loading => message_line("reading ...", th.dim(), w),
            PanelStatus::Failed(e) => message_line(&format!("{}: {e}", v.spec.failed), th.error(), w),
            _ => message_line(&v.spec.empty, th.dim(), w),
        });
    } else {
        for r in v.rows.iter().skip(s.top_of(p)).take(cap) {
            lines.push(panel_line(r, v.spec.name_fit, w, s.sel == Some(Sel::Panel(k, r.id.clone())), th));
        }
    }
    f.render_widget(Paragraph::new(lines).style(th.body()), inner);
}

/// The command that drops into a session, shown in the detail panel (and used when a drop-in cannot start).
pub fn drop_in_text(sel: &Sel) -> String {
    match sel {
        Sel::Remote(id) => format!("remote-sessions attach {id} --tui"),
        Sel::Cloud(id) => format!("cloud-sessions watch {id}"),
        Sel::Panel(_, id) => format!("drop in: {id}"),
    }
}

fn detail_lines(s: &RoomState, w: usize, th: &Theme) -> Vec<Line<'static>> {
    let key = |k: &str| sp(k.to_string(), th.accent());
    let dim = |t: String| sp(t, th.dim());
    let mut out: Vec<Line<'static>> = Vec::new();
    match &s.sel {
        None => out.push(Line::from(dim("Nothing selected. Up and Down choose a session; Enter drops into it.".into()))),
        Some(Sel::Remote(id)) => {
            let Some(r) = s.remote_row(id) else { return out };
            out.push(Line::from(sp(truncate(&r.id, w), th.bold())));
            let state = match &r.state {
                RState::Waiting => format!("{} waiting on {} permission prompt{}", th.pause_glyph(), r.prompts.len(), if r.prompts.len() == 1 { "" } else { "s" }),
                RState::Idle => "idle: nothing waiting on you".into(),
                RState::Unreadable(e) => format!("unreadable: {e}"),
                RState::Unknown => "unknown: no claude.ai login to read its prompts".into(),
                RState::HostDown(e) => format!("{}: {e}", r.host),
            };
            let pid = r.pid.map(|p| format!("pid {p}  ")).unwrap_or_default();
            let st = match r.state {
                RState::Waiting => th.warn(),
                RState::Unreadable(_) => th.error(),
                _ => th.dim(),
            };
            out.push(Line::from(vec![dim(pid), sp(truncate(&state, w.saturating_sub(10)), st)]));
            for p in r.prompts.iter().take(2) {
                out.push(Line::from(vec![sp(format!("  {} ", th.pause_glyph()), th.warn()), sp(format!("{} ", p.tool), th.bold()), sp(truncate(&tool_args(&p.tool, &p.input, 200), w.saturating_sub(8 + width(&p.tool))), th.code())]));
            }
            out.push(Line::from(vec![key("Enter "), dim(truncate(&drop_in_text(&Sel::Remote(r.id.clone())), w.saturating_sub(7)))]));
        }
        Some(Sel::Cloud(id)) => {
            let Some(c) = s.cloud_row(id) else { return out };
            let title = if c.title.is_empty() { "(untitled)" } else { &c.title };
            out.push(Line::from(sp(truncate(title, w), th.bold())));
            let st = c.state(s.now_ms);
            let (_, word, style) = cstate_look(st, th);
            let age = c.last_at_ms.map(|at| format!("  ·  {} ago", ago(s.now_ms, at))).unwrap_or_default();
            let word = if st == CState::Unknown { "not read yet" } else { word };
            out.push(Line::from(vec![sp(truncate(&c.repo, 40), th.accent2()), dim("  ".into()), sp(word.to_string(), style), dim(age)]));
            if let Some(e) = &c.error {
                out.push(Line::from(sp(truncate(&format!("could not read: {e}"), w), th.error())));
            } else if !c.last.is_empty() {
                for (i, l) in wrap_plain(&c.last, w.saturating_sub(6)).into_iter().take(2).enumerate() {
                    out.push(Line::from(vec![dim(if i == 0 { "last  ".into() } else { "      ".into() }), sp(l, Style::default())]));
                }
            }
            out.push(Line::from(vec![key("Enter "), dim(truncate(&drop_in_text(&Sel::Cloud(c.id.clone())), w.saturating_sub(7)))]));
            out.push(Line::from(vec![key("s     "), dim("send a follow-up  (cloud sessions raise no prompts: nothing to approve)".to_string())]));
        }
        Some(Sel::Panel(k, id)) => {
            let Some(r) = s.panel_row(*k, id) else { return out };
            for line in &r.detail {
                let spans: Vec<Span<'static>> = line
                    .iter()
                    .map(|p| {
                        let text = p.text.render(th);
                        let text = match p.max {
                            Max::Any => text,
                            Max::Cols(n) => truncate(&text, n),
                            Max::WidthLess(d) => truncate(&text, w.saturating_sub(d)),
                        };
                        sp(text, ink_style(p.ink, r.tone, th))
                    })
                    .collect();
                out.push(Line::from(spans));
            }
        }
    }
    out
}

fn draw_detail(f: &mut Frame, r: Rect, s: &RoomState, th: &Theme) {
    let block = panel_block(vec![sp(" Selected ", th.title())], None, false, th);
    let inner = block.inner(r);
    f.render_widget(block, r);
    let lines = detail_lines(s, (inner.width as usize).saturating_sub(2), th);
    let lines: Vec<Line<'static>> = lines
        .into_iter()
        .take(inner.height as usize)
        .map(|l| {
            let mut spans = vec![sp(" ", Style::default())];
            spans.extend(l.spans);
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(th.body()), inner);
}

/// The three-row header: the logo's cells (blank here; the app writes the image after the frame), then the usual header line,
/// the session counts, and a blank row. Same texts as the one-row header, so no new glyph.
fn draw_band(f: &mut Frame, l: &Layout, s: &RoomState, th: &Theme) {
    let h = l.header;
    f.render_widget(Block::new().style(th.band()), h);
    let (x, w) = (h.x + BAND_TEXT_COL, h.width.saturating_sub(BAND_TEXT_COL));
    f.render_widget(Paragraph::new(header_line(s, w as usize, th)).style(th.band()), Rect { x, y: h.y, width: w, height: 1 });
    let dot = format!("  {}  ", th.notice_glyph());
    let mut counts = format!(" remote {}{dot}cloud {}", s.remote.len(), s.cloud.iter().filter(|c| c.kind == RowKind::Session || c.kind == RowKind::Run).count());
    for p in s.panels.iter().filter(|p| p.visible()) {
        counts.push_str(&format!("{dot}{} {}", p.spec.noun, p.rows.len()));
    }
    f.render_widget(Paragraph::new(Line::from(sp(truncate(&counts, w as usize), th.dim()))).style(th.band()), Rect { x, y: h.y + 1, width: w, height: 1 });
}

fn header_line(s: &RoomState, w: usize, th: &Theme) -> Line<'static> {
    let left = vec![sp(" remote-sessions ", th.brand()), sp(format!("{} control room", th.notice_glyph()), th.dim())];
    let mut right: Vec<Span<'static>> = Vec::new();
    if !s.host.is_empty() {
        right.push(sp(format!("{}  ", s.host), th.dim()));
    }
    if s.demo {
        right.push(sp("demo  ", th.accent2()));
    }
    let waiting = s.waiting();
    right.push(if waiting > 0 { sp(format!("{} {} waiting", th.pause_glyph(), waiting), th.warn()) } else { sp("nothing waiting", th.dim()) });
    right.push(sp(format!("  {}  ", th.notice_glyph()), th.dim()));
    right.push(sp(format!("{} {} running ", th.dot(), s.running()), th.ok()));
    let lw: usize = left.iter().map(|x| width(&x.content)).sum();
    let rw: usize = right.iter().map(|x| width(&x.content)).sum();
    let free = w.saturating_sub(CLOSE_W as usize);
    let mut spans = left;
    if lw + rw < free {
        spans.push(sp(" ".repeat(free - lw - rw), Style::default()));
        spans.extend(right);
    } else if lw < free {
        spans.push(sp(" ".repeat(free - lw), Style::default()));
    }
    // The close button: always there, coral, so it is plain how to leave.
    spans.push(sp(format!(" {} close ", th.cross()), th.close_style()));
    Line::from(spans)
}

fn status_line(s: &RoomState, w: usize, th: &Theme) -> Line<'static> {
    if s.toast.is_none() && matches!(s.mode, Mode::Send { .. }) {
        return Line::from(sp(truncate(&format!(" Enter send  {}  Esc cancel  {}  Ctrl-U clear", th.notice_glyph(), th.notice_glyph()), w), th.dim()));
    }
    if let Some(t) = &s.toast {
        let style = if t.ok { th.ok() } else { th.warn() };
        return Line::from(sp(format!(" {}", truncate(&one_line(&t.text, 200), w.saturating_sub(1))), style));
    }
    let age = |at: Option<u64>| at.map(|t| format!("{} ago", ago(s.now_ms, t))).unwrap_or_else(|| "...".into());
    let mut more = String::new();
    for p in s.panels.iter().filter(|p| p.visible()) {
        more.push_str(&format!("  {}  {} {}", th.notice_glyph(), p.spec.noun, age(p.at_ms)));
    }
    Line::from(sp(truncate(&format!(" remote {}  {}  cloud {}{}", age(s.remote_at_ms), th.notice_glyph(), age(s.cloud_at_ms), more), w), th.dim()))
}

/// The footer's action chips, most important first, as (key, label, drop priority). Higher priority number goes first.
fn footer_items(s: &RoomState, th: &Theme) -> Vec<(String, String, u8)> {
    let updown = if th.ascii { "Up/Dn" } else { "↑↓" };
    match &s.mode {
        Mode::Confirm { allow, .. } => vec![("y".into(), if *allow { "allow" } else { "deny" }.into(), 0), ("n/Esc".into(), "cancel".into(), 0)],
        Mode::Send { .. } => vec![("Enter".into(), "send".into(), 0), ("Esc".into(), "cancel".into(), 0)],
        Mode::Browse => {
            let mut v = vec![(updown.to_string(), "select".to_string(), 1)];
            match &s.sel {
                Some(Sel::Remote(id)) => {
                    v.push(("Enter/click".into(), "attach".into(), 1));
                    if s.remote_row(id).is_some_and(|r| !r.prompts.is_empty()) {
                        v.push(("a".into(), "approve".into(), 2));
                        v.push(("d".into(), "deny".into(), 2));
                    }
                }
                Some(Sel::Cloud(_)) => {
                    v.push(("Enter/click".into(), "watch".into(), 1));
                    v.push(("s".into(), "send".into(), 2));
                }
                Some(Sel::Panel(..)) => v.push(("Enter/click".into(), "drop in".into(), 1)),
                None => v.push(("Enter/click".into(), "drop in".into(), 1)),
            }
            v.push(("Tab".into(), "panel".into(), 4));
            if s.routines_available {
                v.push(("R".into(), "routines".into(), 5));
            }
            v.push(("r".into(), "refresh".into(), 3));
            v.push((format!("q/Esc/{}", th.cross()), "close".into(), 0));
            v
        }
    }
}

fn footer_line(s: &RoomState, w: usize, th: &Theme) -> Line<'static> {
    let mut items = footer_items(s, th);
    let len = |v: &Vec<(String, String, u8)>| v.iter().map(|(k, l, _)| width(k) + 1 + width(l) + 2).sum::<usize>() + 1;
    while len(&items) > w {
        // Drop the least important chip that is still droppable (highest priority number).
        let worst = items.iter().enumerate().max_by_key(|(i, (_, _, p))| (*p, *i)).map(|(i, (_, _, p))| (i, *p));
        match worst {
            Some((i, p)) if p > 0 => {
                items.remove(i);
            }
            _ => break,
        }
    }
    let mut spans = vec![sp(" ", Style::default())];
    for (k, l, _) in items {
        spans.push(sp(k, th.accent()));
        spans.push(sp(format!(" {}  ", l), th.dim()));
    }
    Line::from(spans)
}

fn draw_modal(f: &mut Frame, area: Rect, s: &RoomState, th: &Theme) {
    let Mode::Confirm { allow, session, tool, args, .. } = &s.mode else { return };
    let iw = (area.width as usize).saturating_sub(4).max(1);
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![sp(format!(" {tool}"), th.bold()), sp(if *allow { " wants to run, and you would allow it:" } else { " is waiting, and you would deny it:" }, Style::default())]),
        Line::default(),
    ];
    for l in wrap_plain(args, iw).into_iter().take(4) {
        lines.push(Line::from(vec![sp("  ", Style::default()), sp(l, th.code())]));
    }
    lines.push(Line::default());
    lines.push(Line::from(sp(format!(" {}", truncate(session, iw)), th.dim())));
    lines.push(Line::from(vec![sp(" y ", th.accent()), sp(if *allow { "allow  " } else { "deny  " }, Style::default()), sp(" n/Esc ", th.accent()), sp("cancel", Style::default())]));
    let (mh, mw) = ((lines.len() as u16 + 2).min(area.height), area.width.min(66));
    let r = Rect { x: area.x + (area.width - mw) / 2, y: area.y + (area.height - mh) / 2, width: mw, height: mh };
    let style = if *allow { th.warn() } else { th.error() };
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(Block::new().borders(Borders::ALL).border_set(border_set(th)).border_style(style).title_top(Line::from(Span::styled(if *allow { " approve " } else { " deny " }, style)))), r);
}

fn draw_send(f: &mut Frame, r: Rect, id: &str, text: &str, th: &Theme) {
    let label = format!(" send to {} {} ", truncate(id, 24), th.prompt_glyph());
    let lw = width(&label);
    let room = (r.width as usize).saturating_sub(lw + 1);
    let shown: String = {
        let chars: Vec<char> = text.chars().collect();
        let mut w = 0;
        let mut start = chars.len();
        while start > 0 && w + crate::text::cw(chars[start - 1]) < room {
            start -= 1;
            w += crate::text::cw(chars[start]);
        }
        chars[start..].iter().collect()
    };
    let line = Line::from(vec![sp(label, th.accent()), sp(shown.clone(), Style::default()), sp(if th.ascii { "_" } else { "▏" }, th.accent())]);
    f.render_widget(Paragraph::new(line), r);
}

pub fn draw(f: &mut Frame, s: &RoomState, th: &Theme) -> Layout {
    let area = f.area();
    let l = layout(area, s);
    if l.small {
        f.render_widget(Paragraph::new(Line::from(sp(truncate("terminal too small", area.width as usize), th.warn()))), area);
        th.fold_buffer(f.buffer_mut());
        return l;
    }
    let w = area.width as usize;
    if s.band && l.header.height >= 3 {
        draw_band(f, &l, s, th);
    } else {
        f.render_widget(Paragraph::new(header_line(s, w, th)).style(th.band()), l.header);
    }
    draw_remote(f, &l, s, th);
    draw_cloud(f, &l, s, th);
    for k in 0..l.panels.len() {
        if l.panels[k].height > 0 {
            draw_panel(f, &l, s, k, th);
        }
    }
    if let Some(d) = l.detail {
        draw_detail(f, d, s, th);
    }
    f.render_widget(Paragraph::new(status_line(s, w, th)), l.status);
    match &s.mode {
        Mode::Send { id, text } => draw_send(f, l.footer, id, text, th),
        _ => f.render_widget(Paragraph::new(footer_line(s, w, th)), l.footer),
    }
    if matches!(s.mode, Mode::Confirm { .. }) {
        let over = Rect { x: area.x, y: area.y + 1, width: area.width, height: area.height.saturating_sub(3) };
        draw_modal(f, over, s, th);
    }
    th.fold_buffer(f.buffer_mut());
    l
}

// ---- one frame as text --------------------------------------------------------------------------------------------

/// The buffer as plain text (trailing spaces trimmed), or with ANSI colors when `ansi` is set.
pub fn dump(buf: &Buffer, ansi: bool) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut row = String::new();
        let mut cur = Style::default();
        let mut x = 0;
        let mut text_end = 0;
        let mut pieces: Vec<(String, Style)> = Vec::new();
        while x < buf.area.width {
            let c = &buf[(x, y)];
            let sym = c.symbol().to_string();
            x += UnicodeWidthStr::width(sym.as_str()).max(1) as u16;
            if sym.trim().is_empty() && c.bg == Color::Reset {
                pieces.push((sym, Style::default()));
            } else {
                text_end = pieces.len() + 1;
                pieces.push((sym, Style::default().fg(c.fg).bg(c.bg).add_modifier(c.modifier)));
            }
        }
        if !ansi {
            let line: String = pieces.iter().map(|(s, _)| s.as_str()).collect();
            out.push_str(line.trim_end());
            out.push('\n');
            continue;
        }
        pieces.truncate(text_end);
        for (sym, st) in pieces {
            if st != cur {
                row.push_str(&sgr(&st));
                cur = st;
            }
            row.push_str(&sym);
        }
        if cur != Style::default() {
            row.push_str("\x1b[0m");
        }
        out.push_str(&row);
        out.push('\n');
    }
    out
}

fn color_code(c: Color, bg: bool) -> Option<String> {
    let base = if bg { 40 } else { 30 };
    Some(match c {
        Color::Reset => return None,
        Color::Black => format!("{}", base),
        Color::Red => format!("{}", base + 1),
        Color::Green => format!("{}", base + 2),
        Color::Yellow => format!("{}", base + 3),
        Color::Blue => format!("{}", base + 4),
        Color::Magenta => format!("{}", base + 5),
        Color::Cyan => format!("{}", base + 6),
        Color::Gray => format!("{}", base + 7),
        Color::DarkGray => format!("{}", base + 60),
        Color::LightRed => format!("{}", base + 61),
        Color::LightGreen => format!("{}", base + 62),
        Color::LightYellow => format!("{}", base + 63),
        Color::LightBlue => format!("{}", base + 64),
        Color::LightMagenta => format!("{}", base + 65),
        Color::LightCyan => format!("{}", base + 66),
        Color::White => format!("{}", base + 67),
        Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", if bg { 48 } else { 38 }),
        Color::Indexed(i) => format!("{};5;{i}", if bg { 48 } else { 38 }),
    })
}

fn sgr(st: &Style) -> String {
    let mut codes: Vec<String> = vec!["0".into()];
    let m = st.add_modifier;
    for (flag, code) in [(Modifier::BOLD, "1"), (Modifier::DIM, "2"), (Modifier::UNDERLINED, "4"), (Modifier::REVERSED, "7")] {
        if m.contains(flag) {
            codes.push(code.into());
        }
    }
    if let Some(c) = st.fg.and_then(|c| color_code(c, false)) {
        codes.push(c);
    }
    if let Some(c) = st.bg.and_then(|c| color_code(c, true)) {
        codes.push(c);
    }
    format!("\x1b[{}m", codes.join(";"))
}

/// One frame of the room drawn off-screen (ratatui `TestBackend`) as text: plain, or with ANSI colors when `ansi`.
pub fn render_text(s: &RoomState, th: &Theme, w: u16, h: u16, ansi: bool) -> String {
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).expect("test backend");
    term.draw(|f| {
        draw(f, s, th);
    })
    .expect("draw");
    dump(term.backend().buffer(), ansi)
}
