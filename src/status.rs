//! What others see of an agent: its status, one line on what it is doing and where, which it
//! sets itself and which lasts until the next, the inbox's file `status`; and its presence, from
//! the inbox's locks. Anyone may set any agent's status, as anyone may deliver into its inbox.

use std::{
  fs, io,
  os::unix::fs::OpenOptionsExt,
  path::{Path, PathBuf},
  time::SystemTime,
};

use crate::{Inbox, at};

pub struct Status {
  /// One line, never empty.
  pub text: String,
  /// When it was set.
  pub set: SystemTime,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presence {
  /// In a turn: a lock on `awake` is held.
  Awake,
  /// A mind waits and will wake itself: a lock on `asleep` is held.
  Asleep,
  /// Neither: the waker brings it back as its bell rings.
  Offline,
}

impl Presence {
  pub fn name(self) -> &'static str {
    match self {
      Self::Awake => "awake",
      Self::Asleep => "asleep",
      Self::Offline => "offline",
    }
  }
}

impl Inbox {
  pub fn status_file(&self) -> PathBuf {
    self.root.join("status")
  }

  /// The agent's name: its inbox's.
  pub fn name(&self) -> String {
    self
      .root
      .file_name()
      .map(|name| name.to_string_lossy().into_owned())
      .unwrap_or_default()
  }

  /// The agent's status, if it set one.
  pub fn status(&self) -> io::Result<Option<Status>> {
    let path = self.status_file();
    let read = fs::read_to_string(&path).and_then(|text| {
      let set = fs::metadata(&path)?.modified()?;
      Ok((text, set))
    });
    match read {
      Ok((text, set)) => Ok(
        Some(one_line(&text))
          .filter(|text| !text.is_empty())
          .map(|text| Status { text, set }),
      ),
      Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
      Err(e) => Err(at(e, &path)),
    }
  }

  /// Sets the agent's status, as one line, at once and whole; an empty one clears it.
  pub fn set_status(&self, text: &str) -> io::Result<()> {
    let path = self.status_file();
    let text = one_line(text);
    if text.is_empty() {
      return match fs::remove_file(&path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(at(e, &path)),
        _ => Ok(()),
      };
    }
    self.create()?;
    let tmp = self
      .tmp_dir()
      .join(format!(".status.{}", std::process::id()));
    fs::OpenOptions::new()
      .write(true)
      .create(true)
      .truncate(true)
      .mode(0o600)
      .open(&tmp)
      .and_then(|mut file| io::Write::write_all(&mut file, format!("{text}\n").as_bytes()))
      .map_err(|e| at(e, &tmp))?;
    fs::rename(&tmp, &path).map_err(|e| at(e, &path))
  }

  /// Whether the agent is awake, asleep or offline, from the locks a moment ago.
  pub fn presence(&self) -> io::Result<Presence> {
    Ok(if self.awake()? {
      Presence::Awake
    } else if self.asleep()? {
      Presence::Asleep
    } else {
      Presence::Offline
    })
  }
}

/// The inboxes in a directory, such as the machine's `~/.agents/inbox/`, by name.
pub fn inboxes(dir: &Path) -> io::Result<Vec<Inbox>> {
  let entries = match fs::read_dir(dir) {
    Ok(entries) => entries,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(e) => return Err(at(e, dir)),
  };
  let mut inboxes = Vec::new();
  for entry in entries {
    let entry = entry.map_err(|e| at(e, dir))?;
    if !entry.file_name().to_string_lossy().starts_with('.')
      && entry.file_type().is_ok_and(|t| t.is_dir())
    {
      inboxes.push(Inbox::new(entry.path()));
    }
  }
  inboxes.sort_by(|a, b| a.root.cmp(&b.root));
  Ok(inboxes)
}

/// The text on one line, its runs of whitespace one space each.
fn one_line(text: &str) -> String {
  text.split_whitespace().collect::<Vec<_>>().join(" ")
}
