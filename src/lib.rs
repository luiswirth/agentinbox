//! MindIO: the boundary between an agent's mind and its world, as IO is in hardware: input,
//! output and waking, where hardware has interrupts.
//!
//! An agent's IO is one directory on its machine, by convention `~/.agents/mindio/<agent>/`,
//! named after the agent (`MindIo`):
//!
//! - `inbox/`, the input (`Inbox`): what reaches the agent from outside, which the mind takes in at
//!   its next step.
//! - `outbox/`, the output (`Outbox`): what the agent says to someone outside, which a subscriber
//!   carries.
//! - `wake`, the file that wakes the agent: present from a wake until a mind answers it.
//! - `awake`, locked while a mind is in a turn, and `asleep`, locked while a mind waits and wakes
//!   itself; with neither held, the agent is offline.
//! - `resume`, the command that brings the agent's mind back, which the machine's waker runs.
//! - `waking`, the agent's rules on what wakes it (`MindIo::wakes`).
//! - `alarms/`, the alarms (`Alarm`), which the waker runs.
//! - `status`, the agent's status (`Status`), what others see of it.
//!
//! The inbox and the outbox are one pair, each a directory of items: `new/` holds the items not
//! yet taken, `cur/` the taken ones, `tmp/` the ones being written. An item is one file, named
//! `<time>.<name>.<ext>`, such as `20261115T153012.123456Z.luis.md`, with the time in UTC to the
//! microsecond, so that the names sort by time, and the name, the sender's in the inbox and the
//! recipient's in the outbox, encoded one to one: a `/` as `_`, and every other byte but ASCII
//! letters, digits and `-` as `%XX`, such as `luis_lead` for `luis/lead`.
//!
//! Other machines' agents are seen through the peers (`peers`).
//!
//! What it creates is its owner's alone: its directories 0700, its files 0600.

use std::{
  ffi::OsString,
  fs, io,
  io::{Read, Write},
  os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
  path::{Path, PathBuf},
  sync::{
    atomic::{AtomicU64, Ordering},
    mpsc,
  },
};

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};
use notify::{RecursiveMode, Watcher};

mod alarm;
pub mod peers;
mod status;
pub use alarm::Alarm;
pub use status::{Presence, Status, all};

const TIME_FORMAT: &str = "%Y%m%dT%H%M%S.%6fZ";
const TIME_LEN: usize = "20261115T153012.123456Z".len();

/// The waking rules every agent's own come after: every event wakes, and no reading does.
pub const DEFAULT_WAKING: &str = "kind=event wake\nkind=reading quiet\n";

/// What an inbox item is, by how it came: the two kinds of providers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
  /// Something that happened, delivered as it happens, such as a message.
  Event,
  /// A state, delivered as the agent looks, by a reading the take runs, such as the time.
  Reading,
}

impl Kind {
  pub fn name(self) -> &'static str {
    match self {
      Self::Event => "event",
      Self::Reading => "reading",
    }
  }
}

/// An agent's IO: its directory, named after it.
pub struct MindIo {
  pub root: PathBuf,
}

/// The input: what reaches the agent from outside.
pub struct Inbox {
  pub root: PathBuf,
}

/// The output: what the agent says to someone outside.
pub struct Outbox {
  pub root: PathBuf,
}

pub struct Item {
  /// Where the item lies now, in `cur/`.
  pub path: PathBuf,
  pub time: Timestamp,
  /// `None` for a file whose name is not in the item format, such as one copied in by hand.
  pub sender: Option<String>,
  pub content: Content,
}

pub enum Content {
  Text(String),
  /// Content that is not UTF-8, left in the file.
  File {
    mime: String,
  },
}

/// An outbox item a subscriber took.
pub struct Outgoing {
  /// Where it lies now, in `cur/`.
  pub path: PathBuf,
  pub time: Timestamp,
  pub recipient: String,
  pub ext: String,
}

impl MindIo {
  pub fn new(root: impl Into<PathBuf>) -> Self {
    Self { root: root.into() }
  }

  /// The agent's name: its directory's.
  pub fn name(&self) -> String {
    self
      .root
      .file_name()
      .map(|name| name.to_string_lossy().into_owned())
      .unwrap_or_default()
  }

  pub fn inbox(&self) -> Inbox {
    Inbox {
      root: self.root.join("inbox"),
    }
  }
  pub fn outbox(&self) -> Outbox {
    Outbox {
      root: self.root.join("outbox"),
    }
  }
  pub fn wake_file(&self) -> PathBuf {
    self.root.join("wake")
  }
  pub fn awake_file(&self) -> PathBuf {
    self.root.join("awake")
  }
  pub fn asleep_file(&self) -> PathBuf {
    self.root.join("asleep")
  }
  pub fn resume_file(&self) -> PathBuf {
    self.root.join("resume")
  }
  pub fn waking_file(&self) -> PathBuf {
    self.root.join("waking")
  }

  /// Creates the agent's directory, its inbox and its outbox where they are missing, its owner's
  /// alone.
  pub fn create(&self) -> io::Result<()> {
    private_dir(&self.root)?;
    create_queue(&self.inbox().root)?;
    create_queue(&self.outbox().root)
  }

  /// Delivers an item into the inbox, from `sender`, and wakes the agent where its waking rules
  /// say so (`wakes`), matched by `from`, `kind` and the provider's own `fields`.
  pub fn deliver(
    &self,
    content: impl Read,
    sender: &str,
    ext: &str,
    kind: Kind,
    fields: &[(&str, &str)],
  ) -> io::Result<PathBuf> {
    self.create()?;
    let path = self.inbox().deliver(content, sender, ext)?;
    let mut all = vec![("from", sender), ("kind", kind.name())];
    all.extend_from_slice(fields);
    if self.wakes(&all)? {
      self.wake()?;
    }
    Ok(path)
  }

  /// The command that brings the agent's mind back, as whoever started it wrote it, if any.
  pub fn resume(&self) -> io::Result<Option<String>> {
    let path = self.resume_file();
    match fs::read_to_string(&path) {
      Ok(command) => Ok(Some(command.trim().to_string()).filter(|c| !c.is_empty())),
      Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
      Err(e) => Err(at(e, &path)),
    }
  }

  /// Whether an item wakes the agent, by its fields, such as `from=luis kind=event`: the agent
  /// decides, by its waking rules, the file `waking`, read after the defaults
  /// (`DEFAULT_WAKING`). Each line is a rule, fields to match and then `wake` or `quiet`, such as
  /// `from=lead quiet`, or `quiet` alone for everything; a field matches where the item has it
  /// with that value, and the last rule that matches decides. `#` starts a comment. A line that
  /// is no rule is an error, so no rule is lost unseen.
  pub fn wakes(&self, fields: &[(&str, &str)]) -> io::Result<bool> {
    let path = self.waking_file();
    let own = match fs::read_to_string(&path) {
      Ok(rules) => rules,
      Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
      Err(e) => return Err(at(e, &path)),
    };
    let mut wakes = false;
    for (source, rules) in [
      ("the defaults", DEFAULT_WAKING),
      (&*path.to_string_lossy(), &own),
    ] {
      for (n, line) in rules.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default();
        let mut words: Vec<_> = line.split_whitespace().collect();
        let Some(verdict) = words.pop() else {
          continue;
        };
        let invalid = |what: &str| {
          io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{source}:{}: {what}", n + 1),
          )
        };
        let verdict = match verdict {
          "wake" => true,
          "quiet" => false,
          _ => return Err(invalid("a rule ends in `wake` or `quiet`")),
        };
        let mut matches = true;
        for word in words {
          let (key, value) = word
            .split_once('=')
            .ok_or_else(|| invalid("a field is `<name>=<value>`"))?;
          matches &= fields.iter().any(|&(k, v)| k == key && v == value);
        }
        if matches {
          wakes = verdict;
        }
      }
    }
    Ok(wakes)
  }

  /// Wakes the agent: creates `wake`, which stays until a mind answers it.
  pub fn wake(&self) -> io::Result<()> {
    self.create()?;
    let wake = self.wake_file();
    fs::OpenOptions::new()
      .create(true)
      .append(true)
      .mode(0o600)
      .open(&wake)
      .map_err(|e| at(e, &wake))?;
    Ok(())
  }

  /// Blocks until the agent is woken, then answers the wake by removing `wake`.
  /// Returns at once where it was woken since the last wait.
  pub fn wait(&self) -> io::Result<()> {
    self.until(|io| io.answer_wake())
  }

  /// Blocks until the agent is woken, leaving `wake` in place for whoever answers it.
  pub fn until_woken(&self) -> io::Result<()> {
    self.until(|io| Ok(io.wake_file().exists()))
  }

  /// Answers a wake by removing `wake`: whether there was one, so that of concurrent answerers
  /// exactly one does.
  pub fn answer_wake(&self) -> io::Result<bool> {
    let wake = self.wake_file();
    match fs::remove_file(&wake) {
      Ok(()) => Ok(true),
      Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
      Err(e) => Err(at(e, &wake)),
    }
  }

  /// Takes the exclusive lock on `awake`, waiting while another holds it:
  /// the agent is awake until the returned file is dropped or the process ends,
  /// and then the system releases the lock, so it never goes stale.
  pub fn hold_awake(&self) -> io::Result<fs::File> {
    self.create()?;
    let (file, path) = self.lock_file(self.awake_file())?;
    file.lock().map_err(|e| at(e, &path))?;
    Ok(file)
  }

  /// Takes a shared lock on `asleep`: a mind of the agent waits and will wake itself, such as an
  /// interactive session that keeps a `wait` running, until the returned file is dropped or the
  /// process ends. Several may hold it at once.
  pub fn hold_asleep(&self) -> io::Result<fs::File> {
    self.create()?;
    let (file, path) = self.lock_file(self.asleep_file())?;
    file.lock_shared().map_err(|e| at(e, &path))?;
    Ok(file)
  }

  /// Blocks while any mind holds `asleep`: once it returns, none did a moment ago. It fails
  /// where the directory is gone, and never makes it again.
  pub fn until_no_sleeper(&self) -> io::Result<()> {
    let (file, path) = self.lock_file(self.asleep_file())?;
    file.lock().map_err(|e| at(e, &path))
  }

  fn lock_file(&self, path: PathBuf) -> io::Result<(fs::File, PathBuf)> {
    let file = fs::OpenOptions::new()
      .create(true)
      .append(true)
      .mode(0o600)
      .open(&path)
      .map_err(|e| at(e, &path))?;
    Ok((file, path))
  }

  /// Whether the agent is awake: whether any open file holds the lock on `awake`, in this process
  /// or another. It tests by taking the lock for a moment, during which a `hold_awake` waits.
  pub fn awake(&self) -> io::Result<bool> {
    held(&self.awake_file())
  }

  /// Whether a mind of the agent is asleep: whether any open file holds the lock on `asleep`.
  pub fn asleep(&self) -> io::Result<bool> {
    held(&self.asleep_file())
  }

  /// Blocks until `done` holds, testing it at once and at every change in the directory itself.
  fn until(&self, done: impl Fn(&Self) -> io::Result<bool>) -> io::Result<()> {
    self.create()?;
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx).map_err(io::Error::other)?;
    watcher
      .watch(&self.root, RecursiveMode::NonRecursive)
      .map_err(io::Error::other)?;
    while !done(self)? {
      while !changes(
        &rx
          .recv()
          .map_err(io::Error::other)?
          .map_err(io::Error::other)?,
      ) {}
    }
    Ok(())
  }
}

impl Inbox {
  pub fn new_dir(&self) -> PathBuf {
    self.root.join("new")
  }
  pub fn cur_dir(&self) -> PathBuf {
    self.root.join("cur")
  }
  pub fn tmp_dir(&self) -> PathBuf {
    self.root.join("tmp")
  }

  /// Writes the item into `tmp/`, then links it into `new/` under a name no other item has,
  /// so that `new/` only ever holds complete items and no delivery overwrites another. It wakes
  /// no one: `MindIo::deliver` also wakes.
  pub fn deliver(&self, content: impl Read, sender: &str, ext: &str) -> io::Result<PathBuf> {
    create_queue(&self.root)?;
    write_item(&self.root, content, sender, ext)
  }

  /// Moves every item in `new/` to `cur/`, in order, and returns them.
  /// The move comes first, so that of several concurrent takers exactly one gets each item.
  pub fn take(&self) -> io::Result<Vec<Item>> {
    create_queue(&self.root)?;
    let new = self.new_dir();
    let mut names: Vec<_> = fs::read_dir(&new)
      .map_err(|e| at(e, &new))?
      .map(|entry| entry.map(|entry| entry.file_name()))
      .collect::<Result<_, _>>()
      .map_err(|e| at(e, &new))?;
    names.retain(|name| !name.to_string_lossy().starts_with('.'));
    names.sort();

    let mut items = Vec::new();
    for name in names {
      let path = self.cur_dir().join(&name);
      match fs::rename(new.join(&name), &path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
        Err(e) => return Err(at(e, &path)),
      }
      items.push(read_item(path)?);
    }
    Ok(items)
  }

  /// The oldest item in `new/`, not yet taken in, by its name.
  pub fn oldest_untaken(&self) -> io::Result<Option<OsString>> {
    create_queue(&self.root)?;
    let new = self.new_dir();
    let mut oldest: Option<OsString> = None;
    for entry in fs::read_dir(&new).map_err(|e| at(e, &new))? {
      let name = entry.map_err(|e| at(e, &new))?.file_name();
      if !name.to_string_lossy().starts_with('.') && oldest.as_ref().is_none_or(|o| name < *o) {
        oldest = Some(name);
      }
    }
    Ok(oldest)
  }
}

impl Outbox {
  pub fn new_dir(&self) -> PathBuf {
    self.root.join("new")
  }
  pub fn cur_dir(&self) -> PathBuf {
    self.root.join("cur")
  }

  /// Writes an item for `recipient`, such as `telegram:luis`, as `Inbox::deliver` writes one.
  /// Answering an inbox item is writing an item for the name it came from.
  pub fn send(&self, content: impl Read, recipient: &str, ext: &str) -> io::Result<PathBuf> {
    if recipient.is_empty() {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "a recipient is a name, such as telegram:luis",
      ));
    }
    create_queue(&self.root)?;
    write_item(&self.root, content, recipient, ext)
  }

  /// Takes every item in `new/` whose recipient `serves` accepts, oldest first, by moving it into
  /// `cur/` first, so that of several subscribers exactly one carries each; an item none serves
  /// stays in `new/`. Only a regular file is an item: a link is never followed to what it points
  /// at.
  pub fn take(&self, serves: impl Fn(&str) -> bool) -> io::Result<Vec<Outgoing>> {
    create_queue(&self.root)?;
    let new = self.new_dir();
    let mut names: Vec<(String, Timestamp, String, String)> = fs::read_dir(&new)
      .map_err(|e| at(e, &new))?
      .filter_map(|entry| entry.ok())
      .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
      .filter_map(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        let (time, recipient, ext) = parse_name(&name)?;
        Some((name, time, recipient, ext))
      })
      .filter(|(_, _, recipient, _)| serves(recipient))
      .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let mut taken = Vec::new();
    for (name, time, recipient, ext) in names {
      let path = self.cur_dir().join(&name);
      match fs::rename(new.join(&name), &path) {
        Ok(()) => taken.push(Outgoing {
          path,
          time,
          recipient,
          ext,
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(at(e, &path)),
      }
    }
    Ok(taken)
  }
}

/// Whether an event changes a file: opening and closing one, as testing a lock or reading
/// `resume` does, changes none, and waking at them would make every look at the directory wake
/// the watcher again.
pub fn changes(event: &notify::Event) -> bool {
  !event.kind.is_access()
}

/// Whether any open file holds a lock on `path`, tested by taking it exclusively for a moment.
fn held(path: &Path) -> io::Result<bool> {
  let file = match fs::File::open(path) {
    Ok(file) => file,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
    Err(e) => return Err(at(e, path)),
  };
  match file.try_lock() {
    Ok(()) => Ok(false),
    Err(fs::TryLockError::WouldBlock) => Ok(true),
    Err(fs::TryLockError::Error(e)) => Err(at(e, path)),
  }
}

/// Creates a directory where it is missing, its owner's alone; one made before it was private
/// becomes so.
fn private_dir(dir: &Path) -> io::Result<()> {
  fs::DirBuilder::new()
    .recursive(true)
    .mode(0o700)
    .create(dir)
    .map_err(|e| at(e, dir))?;
  fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| at(e, dir))
}

/// Creates an inbox's or an outbox's directories where they are missing.
fn create_queue(root: &Path) -> io::Result<()> {
  for dir in ["new", "cur", "tmp"].map(|d| root.join(d)) {
    private_dir(&dir)?;
  }
  Ok(())
}

/// Writes an item into the inbox or outbox at `root`: into `tmp/`, then linked into `new/` under
/// a name no other item has, so that `new/` only ever holds complete items and none overwrites
/// another.
fn write_item(root: &Path, mut content: impl Read, who: &str, ext: &str) -> io::Result<PathBuf> {
  let sender = encode(who);
  let ext = ext.trim_start_matches('.');
  let valid = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+');
  if ext.is_empty() || !ext.bytes().all(valid) {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      format!("invalid extension `{ext}`"),
    ));
  }

  static COUNT: AtomicU64 = AtomicU64::new(0);
  let unique = format!(
    "{}-{}",
    std::process::id(),
    COUNT.fetch_add(1, Ordering::Relaxed)
  );
  let tmp = root.join("tmp").join(format!(
    "{}.{unique}.{sender}.{ext}",
    format_time(Timestamp::now())
  ));
  let mut file = fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .mode(0o600)
    .open(&tmp)
    .map_err(|e| at(e, &tmp))?;
  io::copy(&mut content, &mut file).map_err(|e| at(e, &tmp))?;
  file.sync_all().map_err(|e| at(e, &tmp))?;
  drop(file);

  let mut last = String::new();
  loop {
    let time = format_time(Timestamp::now());
    if time == last {
      continue;
    }
    let path = root.join("new").join(format!("{time}.{sender}.{ext}"));
    match fs::hard_link(&tmp, &path) {
      Ok(()) => {
        fs::remove_file(&tmp).map_err(|e| at(e, &tmp))?;
        return Ok(path);
      }
      Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = time,
      Err(e) => return Err(at(e, &path)),
    }
  }
}

/// The extension of a type given as a MIME type, such as `image/png`, or as an extension, such as `png`.
pub fn extension(ty: &str) -> Option<String> {
  let ext = if ty.contains('/') {
    mime2ext::mime2ext(ty)?
  } else {
    ty.trim_start_matches('.')
  };
  Some(ext.to_string()).filter(|ext| !ext.is_empty() && !ext.contains('/'))
}

fn read_item(path: PathBuf) -> io::Result<Item> {
  let name = path
    .file_name()
    .unwrap_or_default()
    .to_string_lossy()
    .into_owned();
  let (time, sender) = match parse_name(&name) {
    Some((time, sender, _)) => (time, Some(sender)),
    None => {
      let modified = fs::metadata(&path)
        .and_then(|m| m.modified())
        .map_err(|e| at(e, &path))?;
      (Timestamp::try_from(modified).unwrap_or_default(), None)
    }
  };
  let bytes = fs::read(&path).map_err(|e| at(e, &path))?;
  let content = match String::from_utf8(bytes) {
    Ok(text) => Content::Text(text),
    Err(_) => {
      let mime = mime_guess::from_path(&path)
        .first_raw()
        .filter(|mime| !mime.starts_with("text/"))
        .unwrap_or("application/octet-stream");
      Content::File {
        mime: mime.to_string(),
      }
    }
  };
  Ok(Item {
    path,
    time,
    sender,
    content,
  })
}

/// An item's name read: its time, its sender or recipient, and its extension.
fn parse_name(name: &str) -> Option<(Timestamp, String, String)> {
  let time = name.get(..TIME_LEN)?;
  let rest = name.get(TIME_LEN..)?.strip_prefix('.')?;
  let (sender, ext) = rest.split_once('.')?;
  let time = DateTime::strptime("%Y%m%dT%H%M%S.%fZ", time)
    .ok()?
    .to_zoned(TimeZone::UTC)
    .ok()?
    .timestamp();
  Some((time, decode(sender), ext.to_string()))
}

fn format_time(time: Timestamp) -> String {
  time.strftime(TIME_FORMAT).to_string()
}

/// A sender as it stands in a file name, one to one: no `.`, no `/`.
fn encode(sender: &str) -> String {
  sender
    .bytes()
    .map(|b| match b {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' => (b as char).to_string(),
      b'/' => "_".to_string(),
      b => format!("%{b:02X}"),
    })
    .collect()
}

/// The sender a file name holds, as `encode` wrote it; anything else is kept as it is.
fn decode(name: &str) -> String {
  let bytes = name.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    let hex = bytes
      .get(i + 1..i + 3)
      .and_then(|h| std::str::from_utf8(h).ok())
      .and_then(|h| u8::from_str_radix(h, 16).ok());
    match (bytes[i], hex) {
      (b'%', Some(byte)) => {
        out.push(byte);
        i += 3;
      }
      (b'_', _) => {
        out.push(b'/');
        i += 1;
      }
      (byte, _) => {
        out.push(byte);
        i += 1;
      }
    }
  }
  String::from_utf8_lossy(&out).into_owned()
}

/// `text` as an attribute's value, on one line: no `"`, `<`, `&` or control character of its own.
fn attribute(text: &str) -> String {
  text
    .chars()
    .map(|c| match c {
      '&' => "&amp;".to_string(),
      '"' => "&quot;".to_string(),
      '<' => "&lt;".to_string(),
      c if c.is_control() => format!("&#{};", c as u32),
      c => c.to_string(),
    })
    .collect()
}

/// `text` as an item's body, which can neither end the item nor open another: every `inbox-item`,
/// in any case and however it is spaced or prefixed, becomes `inbox\-item`.
fn body(text: &str) -> String {
  const TAG: &str = "inbox-item";
  let lower = text.to_ascii_lowercase();
  let mut out = String::with_capacity(text.len());
  let mut last = 0;
  // Every mention of the tag, however it is spaced or prefixed, so that no reader takes one for a
  // boundary: `inbox-item` becomes `inbox\-item`.
  for (i, _) in lower.match_indices(TAG) {
    out.push_str(&text[last..i + 5]);
    out.push('\\');
    last = i + 5;
  }
  out.push_str(&text[last..]);
  out
}

fn at(e: io::Error, path: &Path) -> io::Error {
  io::Error::new(e.kind(), format!("{}: {e}", path.display()))
}

impl Item {
  /// The item as a model reads it: text inline, any other content as its path and type. Nothing
  /// an item holds ends it or adds to its header.
  pub fn render(&self, out: &mut impl Write) -> io::Result<()> {
    let time = self
      .time
      .to_zoned(TimeZone::system())
      .strftime("%Y-%m-%d %H:%M:%S %:z");
    let from = attribute(self.sender.as_deref().unwrap_or("unknown"));
    match &self.content {
      Content::Text(text) => {
        writeln!(out, "<inbox-item from=\"{from}\" time=\"{time}\">")?;
        writeln!(out, "{}", body(text.strip_suffix('\n').unwrap_or(text)))?;
        writeln!(out, "</inbox-item>")
      }
      Content::File { mime } => {
        writeln!(
          out,
          "<inbox-item from=\"{from}\" time=\"{time}\" type=\"{}\" path=\"{}\"/>",
          attribute(mime),
          attribute(&self.path.display().to_string())
        )
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn an_outbox_item_goes_to_the_one_subscriber_that_serves_it() {
    let dir = std::env::temp_dir().join(format!("mindio-outbox-{}", std::process::id()));
    let outbox = MindIo::new(&dir).outbox();
    let sent = outbox.send(&b"hi"[..], "telegram:luis", "md").unwrap();
    assert!(sent.starts_with(outbox.new_dir()));
    assert!(sent.to_string_lossy().ends_with(".telegram%3Aluis.md"));
    outbox
      .send(&b"elsewhere"[..], "discord/luis", "txt")
      .unwrap();
    let taken = outbox.take(|r| r.starts_with("telegram:")).unwrap();
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].recipient, "telegram:luis");
    assert_eq!(taken[0].ext, "md");
    assert_eq!(fs::read(&taken[0].path).unwrap(), b"hi");
    assert!(
      outbox
        .take(|r| r.starts_with("telegram/"))
        .unwrap()
        .is_empty()
    );
    assert_eq!(fs::read_dir(outbox.new_dir()).unwrap().count(), 1);
    fs::remove_dir_all(&dir).unwrap();
  }

  #[test]
  fn the_agent_decides_what_wakes_it_over_the_defaults() {
    let dir = std::env::temp_dir().join(format!("mindio-waking-{}", std::process::id()));
    let mindio = MindIo::new(&dir);
    mindio.create().unwrap();
    let hive = [("from", "luis"), ("kind", "event"), ("session", "hive")];
    let clock = [("from", "clock"), ("kind", "reading")];
    assert!(mindio.wakes(&hive).unwrap());
    assert!(!mindio.wakes(&clock).unwrap());
    fs::write(
      mindio.waking_file(),
      "# listen in the hive\nsession=hive quiet\n\nfrom=clock wake # loud\n",
    )
    .unwrap();
    assert!(!mindio.wakes(&hive).unwrap());
    assert!(mindio.wakes(&clock).unwrap());
    fs::write(mindio.waking_file(), "quiet\nfrom=luis wake\n").unwrap();
    assert!(mindio.wakes(&hive).unwrap());
    assert!(
      !mindio
        .wakes(&[("from", "lead"), ("kind", "event")])
        .unwrap()
    );
    fs::write(mindio.waking_file(), "session=hive loud\n").unwrap();
    assert!(mindio.wakes(&hive).is_err());
    fs::remove_dir_all(&dir).unwrap();
  }
}
