//! The event-loop and worker guards (issue #5): the UI's threading law as
//! tests, not review hopes.
//!
//! **Nothing blocks the event loop.** Every file read or write, network
//! request and file dialog runs on a worker thread. The detector reads the
//! controller sources and fails on `fs::`, `rfd::`, `ureq::` and `sleep(`
//! written outside the two places that are off the event loop: a
//! `spawn_worker(...)` call's extent, and a function whose doc comment
//! opens with `Worker body` — the written contract that it only ever runs
//! on a worker (`launch::watch_game` is the one). The permitted remainder
//! is the stat class that validates UI input before spawning:
//! `fs::metadata(`, `fs::symlink_metadata(`, `fs::try_exists(` and the
//! `Path` methods (`exists`, `is_file`, …), which carry no `fs::` token.
//! `ureq::Agent` construction sends nothing (browse.rs builds one on the
//! loop on purpose) and is permitted; a request is not.
//!
//! **Workers park, they never poll.** A loop that re-checks shared state
//! under a lock on a `sleep` timer is a poll: it burns battery while idle
//! and must park on a `Condvar` (`wait`/`wait_timeout`) instead —
//! `browse::thumb_worker` is the pattern this protects. Sleeping while
//! waiting on the world outside the process (the game process, a clock, an
//! atomic cancel flag) has no condvar to park on and is lawful; that is
//! what `watch_game`'s one-second poll is.
//!
//! Honesty of scope: the detector is lexical. It cannot see call graphs,
//! so a blocking call hidden behind an innocent helper's name is invisible
//! to it; what it pins is the direct writing of a blocking call into
//! controller code, and the queue poll — the two shapes that actually
//! regress. `#[cfg(test)]` items stand outside both guards: test code
//! deliberately fakes blocking workers. main.rs modules outside the guard
//! are named with reasons in [`MODULES_OUTSIDE_THE_GUARDS`]; `dnd/wayland.rs`
//! is `dnd`'s unix submodule, a dedicated listener thread that is
//! worker-side by construction. Each guard is itself tested against bad
//! and good fixtures below, so a detector that stops catching the bad
//! shapes fails the suite.

use std::collections::BTreeSet;
use std::fmt;

const FILE_RULE: &str = "a file read or write on the event loop; move it into a \
                         spawn_worker closure (only stat-class checks may run here)";
const DIALOG_RULE: &str = "a file dialog on the event loop; construct and show it \
                           in a spawn_worker closure";
const NETWORK_RULE: &str = "network on the event loop; the request belongs in a \
                            spawn_worker closure (ureq::Agent construction alone is fine)";
const SLEEP_RULE: &str = "a sleep on the event loop; a wait belongs in a spawn_worker \
                          closure or a Worker-body function";
const POLL_RULE: &str = "a loop that re-checks locked state on a sleep timer; \
                         park on a Condvar instead";

/// The doc-comment contract that a function only ever runs on a worker
/// thread; its sleeps and file calls are as lawful as a spawn_worker
/// closure's (but never its polls).
const WORKER_BODY_MARK: &str = "Worker body";
const AGENT_PREFIX: &str = "Agent";

/// main.rs itself: `spawn_worker`, the app assembly, the module list the
/// guard derives its scope from.
const MAIN_FILE: &str = "src/main.rs";

/// Every controller module the guards read.
const MODULES_UNDER_GUARD: &[&str] = &[
    "browse",
    "conflict",
    "crosshair",
    "detail_tabs",
    "dnd",
    "downloads",
    "frame_probe",
    "installed",
    "launch",
    "pack_store",
    "performance",
    "profiles",
    "repair",
    "source_filter",
    "sources",
    "thumbs",
    "updater",
];

/// main.rs modules the guards do not read, each with the reason that is
/// sound: recorded here rather than silently skipped.
const MODULES_OUTSIDE_THE_GUARDS: &[(&str, &str)] = &[
    (
        "cli",
        "pure argument parsing; main prints its answer before the event loop exists",
    ),
    (
        "describe",
        "pure view-model text; it performs no I/O of any kind",
    ),
    (
        "link_target",
        "pure pasted-link classification; it performs no I/O of any kind",
    ),
    (
        "log",
        "the cfg(windows) diagnostics appender: a Mutex-guarded bounded \
         write, a deliberate exception",
    ),
];

#[derive(Debug, PartialEq)]
struct Finding {
    line: usize,
    excerpt: String,
    rule: &'static str,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {} — {}", self.line, self.excerpt, self.rule)
    }
}

/// The blocking calls a controller makes outside `spawn_worker` regions
/// and outside declared worker bodies: each would run on the event loop.
fn event_loop_blockers(source: &str) -> Vec<Finding> {
    scan(source).blockers
}

/// The polls: sleeps inside loops that re-check locked shared state on a
/// timer instead of parking on a condvar, wherever they run.
fn sleep_polls(source: &str) -> Vec<Finding> {
    let scan = scan(source);
    scan.sleep_sites
        .iter()
        .filter(|site| site.polls_locked_state(&scan))
        .map(|site| finding_at(source, site.at, POLL_RULE))
        .collect()
}

struct SleepSite {
    at: usize,
    loop_group: Option<usize>,
    scope: Vec<usize>,
}

impl SleepSite {
    /// A sleep is a poll when its loop parks on no condvar while the blocks
    /// around it, up to its function, hold or take a lock: shared state
    /// re-checked on a timer. A loop over the outside world (a process, a
    /// clock) locks nothing and is a timer, not a poll.
    fn polls_locked_state(&self, scan: &Scan) -> bool {
        let Some(&loop_group) = self.loop_group.as_ref() else {
            return false;
        };
        let loop_body = span_of(&scan.groups[loop_group]);
        if scan.condvar_parks.iter().any(|at| loop_body.contains(at)) {
            return false;
        }
        self.scope.iter().any(|&group| {
            let span = span_of(&scan.groups[group]);
            scan.lock_calls.iter().any(|at| span.contains(at))
        })
    }
}

fn span_of(group: &Group) -> std::ops::Range<usize> {
    group.open..group.close
}

struct Group {
    open: usize,
    close: usize,
    fn_body: bool,
    loop_body: bool,
    spawn_region: bool,
    worker_body: bool,
    event_loop_region: bool,
}

struct Scan {
    blockers: Vec<Finding>,
    sleep_sites: Vec<SleepSite>,
    lock_calls: Vec<usize>,
    condvar_parks: Vec<usize>,
    groups: Vec<Group>,
    modules: Vec<String>,
}

fn scan(source: &str) -> Scan {
    Walker::new(source).walk()
}

struct DocBlock {
    worker_body: bool,
    end: usize,
}

/// One pass over the source, string-, char-, raw-string- and comment-aware,
/// tracking group nesting so the detector knows at every token whether it
/// sits on the event loop (outside worker regions, or inside an
/// `upgrade_in_event_loop` / `invoke_from_event_loop` callback), inside a
/// `spawn_worker(...)` extent or a declared worker body, or inside a
/// `#[cfg(test)]` item (which is skipped whole: attributes on fields and
/// statements resolve to their `{...}` extent, or dissolve at the next
/// `;`, `,`, `=`).
struct Walker<'s> {
    source: &'s str,
    bytes: &'s [u8],
    pos: usize,
    stack: Vec<usize>,
    scan: Scan,
    pending_fn_depth: Option<usize>,
    pending_fn_is_worker_body: bool,
    pending_loop_depth: Option<usize>,
    pending_spawn: bool,
    pending_event_loop: bool,
    pending_test_item_depth: Option<usize>,
    test_item_groups: Vec<usize>,
    doc: Option<DocBlock>,
}

impl<'s> Walker<'s> {
    fn new(source: &'s str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            stack: Vec::new(),
            scan: Scan {
                blockers: Vec::new(),
                sleep_sites: Vec::new(),
                lock_calls: Vec::new(),
                condvar_parks: Vec::new(),
                groups: Vec::new(),
                modules: Vec::new(),
            },
            pending_fn_depth: None,
            pending_fn_is_worker_body: false,
            pending_loop_depth: None,
            pending_spawn: false,
            pending_event_loop: false,
            pending_test_item_depth: None,
            test_item_groups: Vec::new(),
            doc: None,
        }
    }

    fn walk(mut self) -> Scan {
        while self.pos < self.bytes.len() {
            let byte = self.bytes[self.pos];
            match byte {
                b'/' if self.bytes.get(self.pos + 1) == Some(&b'/') => self.line_comment(),
                b'/' if self.bytes.get(self.pos + 1) == Some(&b'*') => self.block_comment(),
                b'"' => self.pos = string_end(self.bytes, self.pos),
                b'\'' => self.quote(),
                b'#' if self.opens_attribute() => self.attribute(),
                _ if is_word_start(byte) => self.word(),
                b'{' | b'(' | b'[' => self.open_group(byte),
                b'}' | b')' | b']' => self.close_group(),
                b';' | b',' | b'=' => {
                    self.item_terminator(byte);
                    self.pos += 1;
                }
                _ => self.pos += 1,
            }
        }
        self.scan
    }

    fn suppressing(&self) -> bool {
        self.pending_test_item_depth.is_some() || !self.test_item_groups.is_empty()
    }

    fn opens_attribute(&self) -> bool {
        self.bytes.get(self.pos + 1) == Some(&b'[')
            || (self.bytes.get(self.pos + 1) == Some(&b'!')
                && self.bytes.get(self.pos + 2) == Some(&b'['))
    }

    fn line_comment(&mut self) {
        let start = self.pos;
        let mut end = start + 2;
        while end < self.bytes.len() && self.bytes[end] != b'\n' {
            end += 1;
        }
        if !self.suppressing()
            && let Some(text) = self.source[start..end].strip_prefix("///")
        {
            self.extend_doc(text, start, end);
        }
        self.pos = end;
    }

    /// Doc lines attach to the `fn` that follows them with nothing between
    /// but whitespace, visibility and attributes; the `Worker body` mark is
    /// the first line's.
    fn extend_doc(&mut self, text: &str, start: usize, end: usize) {
        let worker_body = text.trim_start().starts_with(WORKER_BODY_MARK);
        let contiguous = match &self.doc {
            Some(block) => {
                let between = &self.source[block.end..start];
                between.matches('\n').count() <= 1 && between.trim().is_empty()
            }
            None => false,
        };
        if contiguous && let Some(block) = self.doc.as_mut() {
            block.worker_body |= worker_body;
            block.end = end;
        } else {
            self.doc = Some(DocBlock { worker_body, end });
        }
    }

    fn block_comment(&mut self) {
        let mut depth = 1;
        let mut pos = self.pos + 2;
        while pos < self.bytes.len() && depth > 0 {
            if self.bytes[pos..].starts_with(b"/*") {
                depth += 1;
                pos += 2;
            } else if self.bytes[pos..].starts_with(b"*/") {
                depth -= 1;
                pos += 2;
            } else {
                pos += 1;
            }
        }
        self.pos = pos;
    }

    /// A quote opens a char literal when an escape or a single char precedes
    /// its partner; otherwise it is a lifetime or a label and only the
    /// quote itself is consumed.
    fn quote(&mut self) {
        let mut chars = self.source[self.pos + 1..].chars();
        let is_char_literal = matches!(
            (chars.next(), chars.next()),
            (Some('\\'), _) | (Some(_), Some('\''))
        );
        if !is_char_literal {
            self.pos += 1;
            return;
        }
        let mut pos = self.pos + 1;
        while pos < self.bytes.len() {
            match self.bytes[pos] {
                b'\\' => pos += 2,
                b'\'' => {
                    pos += 1;
                    break;
                }
                _ => pos += 1,
            }
        }
        self.pos = pos;
    }

    fn attribute(&mut self) {
        let open = if self.bytes[self.pos + 1] == b'[' {
            self.pos + 1
        } else {
            self.pos + 2
        };
        let mut depth = 1;
        let mut pos = open + 1;
        while pos < self.bytes.len() && depth > 0 {
            match self.bytes[pos] {
                b'[' => {
                    depth += 1;
                    pos += 1;
                }
                b']' => {
                    depth -= 1;
                    pos += 1;
                }
                b'"' => pos = string_end(self.bytes, pos),
                _ => pos += 1,
            }
        }
        self.pos = pos;
        let inner_end = pos.saturating_sub(1);
        if inner_end > open {
            let inner = &self.source[open + 1..inner_end];
            if !self.suppressing()
                && inner
                    .bytes()
                    .filter(|b| !b.is_ascii_whitespace())
                    .eq("cfg(test)".bytes())
            {
                self.pending_test_item_depth = Some(self.stack.len());
                self.doc = None;
            }
        }
    }

    fn word(&mut self) {
        let start = self.pos;
        let mut end = start + 1;
        while end < self.bytes.len() && is_word_byte(self.bytes[end]) {
            end += 1;
        }
        let word = &self.source[start..end];
        if (word == "r" || word == "br")
            && let Some(after) = self.raw_string_end(end)
        {
            self.pos = after;
            return;
        }
        if !self.suppressing() {
            self.classify_word(start, end);
        }
        self.pos = end;
    }

    fn raw_string_end(&self, from: usize) -> Option<usize> {
        let mut pos = from;
        while self.bytes.get(pos) == Some(&b'#') {
            pos += 1;
        }
        if self.bytes.get(pos) != Some(&b'"') {
            return None;
        }
        let hashes = &self.source[from..pos];
        let mut scan = pos + 1;
        while scan < self.bytes.len() {
            if self.bytes[scan] == b'"' && self.source[scan + 1..].starts_with(hashes) {
                return Some(scan + 1 + hashes.len());
            }
            scan += 1;
        }
        Some(self.bytes.len())
    }

    fn classify_word(&mut self, start: usize, end: usize) {
        let word = &self.source[start..end];
        let colons = self.bytes.get(end) == Some(&b':') && self.bytes.get(end + 1) == Some(&b':');
        let call = self.byte_after_whitespace(end) == Some(b'(');
        let after_dot = start > 0 && self.bytes[start - 1] == b'.';
        match word {
            "fs" if colons => self.file_call(start, end + 2),
            "File" if colons => {
                let rest = &self.source[end + 2..];
                if rest.starts_with("open(") || rest.starts_with("create(") {
                    self.blocker(start, FILE_RULE);
                }
            }
            "Command" if colons => {
                if self.source[end + 2..].starts_with("new(") {
                    self.blocker(start, FILE_RULE);
                }
            }
            "Registry" if colons => {
                if self.source[end + 2..].starts_with("load(") {
                    self.blocker(start, FILE_RULE);
                }
            }
            "rfd" if colons => self.blocker(start, DIALOG_RULE),
            "ureq" if colons => {
                if !self.source[end + 2..].starts_with(AGENT_PREFIX) {
                    self.blocker(start, NETWORK_RULE);
                }
            }
            "sleep" if call => {
                self.record_sleep(start);
                self.blocker(start, SLEEP_RULE);
            }
            "spawn_worker" if call => self.pending_spawn = true,
            "upgrade_in_event_loop" | "invoke_from_event_loop" if call => {
                self.pending_event_loop = true;
            }
            "loop" | "while" | "for" => self.pending_loop_depth = Some(self.stack.len()),
            "fn" => self.declare_fn(),
            "mod" => self.record_module(end),
            "lock" | "try_lock" if after_dot && call => self.scan.lock_calls.push(start),
            "wait" | "wait_timeout" if after_dot && call => self.scan.condvar_parks.push(start),
            "pub" | "crate" | "unsafe" | "async" => {}
            _ => self.doc = None,
        }
    }

    fn file_call(&mut self, start: usize, after: usize) {
        let call = &self.source[after..];
        let stat_class = call.starts_with("metadata(")
            || call.starts_with("symlink_metadata(")
            || call.starts_with("try_exists(");
        if !stat_class {
            self.blocker(start, FILE_RULE);
        }
    }

    fn on_event_loop(&self) -> bool {
        for &index in self.stack.iter().rev() {
            let group = &self.scan.groups[index];
            if group.event_loop_region {
                return true;
            }
            if group.spawn_region || group.worker_body {
                return false;
            }
        }
        true
    }

    fn blocker(&mut self, at: usize, rule: &'static str) {
        if self.on_event_loop() {
            self.scan.blockers.push(finding_at(self.source, at, rule));
        }
    }

    fn declare_fn(&mut self) {
        let worker_body = self.doc.as_ref().is_some_and(|block| block.worker_body);
        self.doc = None;
        self.pending_fn_depth = Some(self.stack.len());
        self.pending_fn_is_worker_body = worker_body;
    }

    /// `mod name;` declarations, so the guards can be held to main.rs's own
    /// module list; inline `mod name {` bodies do not declare a file.
    fn record_module(&mut self, end: usize) {
        let Some((name_start, name_end)) = self.word_after_whitespace(end) else {
            return;
        };
        let mut pos = name_end;
        while pos < self.bytes.len() && self.bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if self.bytes.get(pos) == Some(&b';') {
            self.scan
                .modules
                .push(self.source[name_start..name_end].to_owned());
        }
    }

    fn record_sleep(&mut self, at: usize) {
        let loop_group = self
            .stack
            .iter()
            .rev()
            .copied()
            .find(|&group| self.scan.groups[group].loop_body);
        let scope = match self
            .stack
            .iter()
            .rposition(|&group| self.scan.groups[group].fn_body)
        {
            Some(fn_at) => self.stack[fn_at..].to_vec(),
            None => self.stack.clone(),
        };
        self.scan.sleep_sites.push(SleepSite {
            at,
            loop_group,
            scope,
        });
    }

    fn byte_after_whitespace(&self, from: usize) -> Option<u8> {
        let mut pos = from;
        while pos < self.bytes.len() && self.bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        self.bytes.get(pos).copied()
    }

    fn word_after_whitespace(&self, from: usize) -> Option<(usize, usize)> {
        let mut pos = from;
        while pos < self.bytes.len() && self.bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= self.bytes.len() || !is_word_start(self.bytes[pos]) {
            return None;
        }
        let start = pos;
        while pos < self.bytes.len() && is_word_byte(self.bytes[pos]) {
            pos += 1;
        }
        Some((start, pos))
    }

    fn open_group(&mut self, kind: u8) {
        let depth = self.stack.len();
        let fn_body = kind == b'{' && self.pending_fn_depth == Some(depth);
        if fn_body {
            self.pending_fn_depth = None;
        }
        let loop_body = kind == b'{' && self.pending_loop_depth == Some(depth);
        if loop_body {
            self.pending_loop_depth = None;
        }
        let worker_body = fn_body && self.pending_fn_is_worker_body;
        if worker_body {
            self.pending_fn_is_worker_body = false;
        }
        let spawn_region = kind == b'(' && self.pending_spawn;
        self.pending_spawn = false;
        let event_loop_region = kind == b'(' && self.pending_event_loop;
        self.pending_event_loop = false;
        if kind == b'{' && self.pending_test_item_depth == Some(depth) {
            self.pending_test_item_depth = None;
            self.test_item_groups.push(self.scan.groups.len());
            self.doc = None;
        }
        let index = self.scan.groups.len();
        self.scan.groups.push(Group {
            open: self.pos,
            close: self.bytes.len(),
            fn_body,
            loop_body,
            spawn_region,
            worker_body,
            event_loop_region,
        });
        self.stack.push(index);
        self.pos += 1;
    }

    fn close_group(&mut self) {
        if let Some(index) = self.stack.pop() {
            let group = &mut self.scan.groups[index];
            group.close = self.pos;
            if self.test_item_groups.contains(&index) {
                self.test_item_groups.retain(|&at| at != index);
                self.doc = None;
            }
        }
        self.pos += 1;
    }

    /// A `#[cfg(test)]` item that resolves to no extent (a field, a const,
    /// a bodiless declaration) dissolves here instead of suppressing the
    /// rest of the file.
    fn item_terminator(&mut self, byte: u8) {
        if self.pending_test_item_depth == Some(self.stack.len()) {
            self.pending_test_item_depth = None;
            self.doc = None;
        }
        if byte == b';' {
            self.pending_fn_depth = None;
            self.pending_loop_depth = None;
        }
    }
}

fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn string_end(bytes: &[u8], open: usize) -> usize {
    let mut pos = open + 1;
    while pos < bytes.len() {
        match bytes[pos] {
            b'\\' => pos += 2,
            b'"' => return pos + 1,
            _ => pos += 1,
        }
    }
    pos
}

fn finding_at(source: &str, at: usize, rule: &'static str) -> Finding {
    let line = 1 + source[..at].matches('\n').count();
    let line_start = source[..at].rfind('\n').map_or(0, |at| at + 1);
    let line_end = source[at..]
        .find('\n')
        .map_or(source.len(), |offset| at + offset);
    Finding {
        line,
        excerpt: source[line_start..line_end].trim().to_owned(),
        rule,
    }
}

/// A controller as it should be: input validated with stat-class calls, all
/// blocking work inside the spawn_worker call's extent, an agent built (no
/// I/O) on the loop, and guarded words that appear only inside strings and
/// comments.
const LAWFUL_CONTROLLER: &str = r##"
use std::path::PathBuf;

fn install_clicked(path: &PathBuf, out: PathBuf) {
    if !path.is_file() {
        return;
    }
    let size = std::fs::metadata(path).map(|m| m.len());
    let agent: ureq::Agent = ureq::Agent::config_builder().build().into();
    spawn_worker(
        "shiv-install",
        move || std::fs::read(path).and_then(|bytes| std::fs::write(&out, bytes)),
        move |result| {
            let _ = result;
        },
    );
    let note = r#"{ "guarded": "fs:: read_to_string( rfd:: ureq:: sleep(" }"#;
    let _ = (size, agent, note);
    // std::thread::sleep( inside a comment is only a comment.
}
"##;

/// The issue's bad commit as a fixture: every blocking family written
/// straight into controller code. Each planted call must be named.
const BLOCKING_CONTROLLER: &str = r#"
fn refresh_clicked(path: &Path, flag: &Path) {
    let text = std::fs::read_to_string(path);
    rfd::FileDialog::new().pick_file();
    ureq::get("https://example.invalid").call();
    std::thread::sleep(std::time::Duration::from_secs(2));
    while std::fs::read_to_string(flag).is_err() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let _ = text;
}
"#;

/// browse.rs's thumb_worker with the condvar park replaced by a timer: the
/// exact regression the worker guard exists for.
const POLLING_QUEUE_WORKER: &str = r#"
fn thumb_worker(shared: &(Mutex<ThumbQueue>, Condvar)) {
    loop {
        let url = {
            let mut q = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(url) = q.pending.pop_back() {
                    break url;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        fetch(&url);
    }
}
"#;

/// thumb_worker as it is: the queue loop parks on the condvar.
const PARKING_QUEUE_WORKER: &str = r#"
fn thumb_worker(shared: &(Mutex<ThumbQueue>, Condvar)) {
    loop {
        let url = {
            let mut q = shared.0.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(url) = q.pending.pop_back() {
                    break url;
                }
                q = shared.1.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
        };
        fetch(&url);
    }
}
"#;

/// watch_game as it is: a declared worker body whose loop waits on the
/// world outside the process, where no condvar exists to park on.
const WORKER_BODY_WATCHING_THE_WORLD: &str = r#"
/// Worker body: wait for the game process, then report it.
fn watch_game(poll: Duration) -> bool {
    loop {
        if game_running() {
            return true;
        }
        std::thread::sleep(poll);
    }
}
"#;

/// The Worker-body contract does not buy back a poll: sleeping over a
/// locked queue is a battery bug even on a worker.
const WORKER_BODY_POLLING_A_QUEUE: &str = r#"
/// Worker body: handle the queue.
fn queue_worker(shared: &Mutex<Vec<String>>) {
    loop {
        match shared.lock().unwrap().pop() {
            Some(url) => handle(&url),
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}
"#;

/// Test code stands outside both guards: it fakes blocking workers on
/// purpose (a gate that simulates a slow disk while the UI is driven).
const TEST_CODE_FAKES_BLOCKERS: &str = r#"
#[cfg(test)]
mod tests {
    #[test]
    fn a_slow_gate() {
        std::fs::write("gate", b"1").unwrap();
        loop {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
fn helper_with_a_dialog() {
    rfd::FileDialog::new().pick_file();
    std::thread::sleep(Duration::from_secs(1));
}
"#;

fn read_controller(path: &str) -> String {
    let full = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("could not read {path}: {e}"))
}

fn controller_files() -> Vec<String> {
    let mut files = vec![MAIN_FILE.to_owned()];
    files.extend(
        MODULES_UNDER_GUARD
            .iter()
            .map(|module| format!("src/{module}.rs")),
    );
    files
}

/// The guard's own proof: pointed at the bad commit, it names every planted
/// blocker — the disk read (twice), the dialog, the request, and both
/// sleeps, loop or no loop.
#[test]
fn the_event_loop_guard_names_every_planted_blocker() {
    let blockers = event_loop_blockers(BLOCKING_CONTROLLER);
    let per_rule = |rule| blockers.iter().filter(|f| f.rule == rule).count();
    assert_eq!(per_rule(FILE_RULE), 2, "{blockers:?}");
    assert_eq!(per_rule(DIALOG_RULE), 1, "{blockers:?}");
    assert_eq!(per_rule(NETWORK_RULE), 1, "{blockers:?}");
    assert_eq!(per_rule(SLEEP_RULE), 2, "{blockers:?}");
    assert_eq!(blockers.len(), 6, "{blockers:?}");
    for planted in ["read_to_string", "FileDialog", "ureq::get", "thread::sleep"] {
        assert!(
            blockers.iter().any(|f| f.excerpt.contains(planted)),
            "the guard missed {planted}: {blockers:?}"
        );
    }
}

/// The good half of the same proof: worker closures, stat-class validation,
/// agent construction, and guarded words inside strings and comments all
/// pass both guards.
#[test]
fn lawful_controller_code_passes_both_guards() {
    assert!(
        event_loop_blockers(LAWFUL_CONTROLLER).is_empty(),
        "{:?}",
        event_loop_blockers(LAWFUL_CONTROLLER)
    );
    assert!(sleep_polls(LAWFUL_CONTROLLER).is_empty());
}

/// The worker guard's own proof: the queue re-checked on a timer is
/// flagged, once, at the sleep.
#[test]
fn a_queue_rechecked_on_a_timer_is_flagged_as_a_poll() {
    let polls = sleep_polls(POLLING_QUEUE_WORKER);
    assert_eq!(polls.len(), 1, "{polls:?}");
    assert!(polls[0].excerpt.contains("sleep"), "{polls:?}");
}

/// The condvar park is not a poll, nor is a declared worker body's wait on
/// the outside world; but the same declaration over a locked queue still
/// fails — the contract exempts a timer, never a poll.
#[test]
fn a_worker_body_may_wait_on_the_world_but_never_poll_a_lock() {
    assert!(sleep_polls(PARKING_QUEUE_WORKER).is_empty());
    assert!(event_loop_blockers(WORKER_BODY_WATCHING_THE_WORLD).is_empty());
    assert!(sleep_polls(WORKER_BODY_WATCHING_THE_WORLD).is_empty());
    assert!(event_loop_blockers(WORKER_BODY_POLLING_A_QUEUE).is_empty());
    assert_eq!(sleep_polls(WORKER_BODY_POLLING_A_QUEUE).len(), 1);
}

/// `#[cfg(test)]` items — a module, a function, and attributes that resolve
/// to no extent — are invisible to both guards.
#[test]
fn cfg_test_items_are_outside_the_guards() {
    assert!(event_loop_blockers(TEST_CODE_FAKES_BLOCKERS).is_empty());
    assert!(sleep_polls(TEST_CODE_FAKES_BLOCKERS).is_empty());
}

/// The guards' file list is main.rs's own module list, minus the recorded
/// exceptions: a new controller module cannot appear outside the guards
/// without this test failing.
#[test]
fn every_module_main_declares_is_under_the_guards() {
    let main_source = read_controller(MAIN_FILE);
    let main_scan = scan(&main_source);
    let declared: BTreeSet<&str> = main_scan.modules.iter().map(String::as_str).collect();
    let under_guard: BTreeSet<_> = MODULES_UNDER_GUARD.iter().copied().collect();
    let outside: BTreeSet<_> = MODULES_OUTSIDE_THE_GUARDS
        .iter()
        .map(|(module, _)| *module)
        .collect();
    let unguarded: Vec<_> = declared.difference(&under_guard).cloned().collect();
    let unexplained: Vec<_> = unguarded
        .iter()
        .filter(|module| !outside.contains(*module))
        .cloned()
        .collect();
    assert!(
        unexplained.is_empty(),
        "main.rs declares modules the guards do not read: {unexplained:?}; \
         add them to MODULES_UNDER_GUARD, or to MODULES_OUTSIDE_THE_GUARDS \
         with a reason"
    );
    let unknown: Vec<_> = under_guard.difference(&declared).cloned().collect();
    assert!(
        unknown.is_empty(),
        "the guards read modules main.rs does not declare: {unknown:?}"
    );
    let stale: Vec<_> = outside.difference(&declared).cloned().collect();
    assert!(
        stale.is_empty(),
        "recorded exceptions that main.rs no longer declares: {stale:?}"
    );
}

/// The deliberately bad commit applied to the real sources, not a fixture:
/// offenses appended after main.rs's and browse.rs's own text (their raw
/// strings, attributes and cfg(test) regions) are still found, so the green
/// scans above are the code being clean, not the scan being blind.
#[test]
fn planted_offenses_in_real_controllers_are_caught() {
    const CANARY: &str = "\nfn shiv_guard_canary(queue: &Mutex<Vec<String>>) {\n    \
                          let _ = std::fs::read_to_string(\"x\");\n    loop {\n        \
                          if queue.lock().unwrap().pop().is_none() {\n            \
                          std::thread::sleep(Duration::from_millis(5));\n        }\n    }\n}\n";
    for file in [MAIN_FILE, "src/browse.rs"] {
        let source = format!("{}{}", read_controller(file), CANARY);
        let blockers = event_loop_blockers(&source);
        assert_eq!(blockers.len(), 2, "{file}: {blockers:?}");
        assert!(
            blockers
                .iter()
                .any(|f| f.rule == FILE_RULE && f.excerpt.contains("read_to_string")),
            "{file}: {blockers:?}"
        );
        assert!(
            blockers
                .iter()
                .any(|f| f.rule == SLEEP_RULE && f.excerpt.contains("sleep")),
            "{file}: {blockers:?}"
        );
        let polls = sleep_polls(&source);
        assert_eq!(polls.len(), 1, "{file}: {polls:?}");
    }
}

/// The event-loop guard over the real sources: no blocking call outside a
/// spawn_worker extent or a declared worker body, in any controller.
#[test]
fn controller_code_blocks_nothing_on_the_event_loop() {
    let mut blockers = Vec::new();
    for file in controller_files() {
        let source = read_controller(&file);
        for finding in event_loop_blockers(&source) {
            blockers.push(format!("{file}: {finding}"));
        }
    }
    assert!(
        blockers.is_empty(),
        "blocking calls on the event loop:\n{}",
        blockers.join("\n")
    );
}

/// The worker guard over the real sources: no loop anywhere in the
/// controllers re-checks locked state on a sleep timer.
#[test]
fn controller_workers_never_poll_locked_state() {
    let mut polls = Vec::new();
    for file in controller_files() {
        let source = read_controller(&file);
        for finding in sleep_polls(&source) {
            polls.push(format!("{file}: {finding}"));
        }
    }
    assert!(
        polls.is_empty(),
        "workers polling locked state on a timer:\n{}",
        polls.join("\n")
    );
}

#[test]
fn guards_catch_blockers_inside_event_loop_callbacks_and_registry_io() {
    let bad = r#"
        fn handler(weak: Weak<App>) {
            spawn_worker("w", || {
                let _ = fs::read("ok_in_worker");
            }, move |_| {
                let _ = weak.upgrade_in_event_loop(move |_| {
                    let _ = fs::read("bad_in_event_loop");
                    let _ = Registry::load(&paths);
                    let _ = File::open("bad");
                    let _ = Command::new("bad");
                    spawn_worker("nested", || {
                        let _ = fs::read("ok_in_nested_worker");
                    }, |_| {});
                });
            });
        }
    "#;
    let blockers = event_loop_blockers(bad);
    assert_eq!(blockers.len(), 4, "{blockers:?}");
    assert!(
        blockers
            .iter()
            .any(|f| f.excerpt.contains("bad_in_event_loop"))
    );
    assert!(
        blockers
            .iter()
            .any(|f| f.excerpt.contains("Registry::load"))
    );
    assert!(blockers.iter().any(|f| f.excerpt.contains("File::open")));
    assert!(blockers.iter().any(|f| f.excerpt.contains("Command::new")));
}
