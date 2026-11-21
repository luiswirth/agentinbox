//! Alarms: a command that waits, and a note, which the machine's waker delivers into the inbox
//! once the command ends. An alarm is the directory `alarms/<name>/` of the inbox, so it
//! outlasts restarts, and dismissing it is removing it.
//!
//! Its files: `command`, run with `sh -c` in the directory `dir`, `note`, `by`, who set it, and
//! `repeat`, present where it is set again after each time it goes off.
//!
//! The command only waits, and since it runs again after a restart, it waits for something that
//! holds then too, such as a time (`agentinbox until`). One that fails dismisses its alarm, so
//! that a broken command never wakes the agent again and again.

use std::{
  fs, io,
  os::unix::fs::{DirBuilderExt, OpenOptionsExt},
  path::PathBuf,
};

use crate::{Inbox, at};

pub struct Alarm {
  pub name: String,
  pub command: String,
  /// Where the command runs.
  pub dir: PathBuf,
  pub note: String,
  /// Who set it.
  pub by: String,
  pub repeat: bool,
}

impl Alarm {
  /// What the agent is told as it goes off.
  pub fn message(&self) -> String {
    let Self { name, by, note, .. } = self;
    let note = note.trim_end();
    if note.is_empty() {
      format!("The alarm `{name}` went off, set by {by}.")
    } else {
      format!("{note}\n\n(The alarm `{name}`, set by {by}.)")
    }
  }
}

impl Inbox {
  pub fn alarms_dir(&self) -> PathBuf {
    self.root.join("alarms")
  }

  /// Sets an alarm, under a name no other alarm of the inbox has: written in `tmp/`, then renamed
  /// into place, so that the waker only ever sees whole alarms, and sees the rename in `tmp/`,
  /// which it watches from the inbox's start, where `alarms/` may be new.
  pub fn set_alarm(&self, alarm: &Alarm) -> io::Result<()> {
    let name = &alarm.name;
    let path = self.alarm_path(name)?;
    self.create()?;
    let alarms = self.alarms_dir();
    fs::DirBuilder::new()
      .mode(0o700)
      .create(&alarms)
      .or_else(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => Ok(()),
        _ => Err(at(e, &alarms)),
      })?;
    if path.exists() {
      return Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("there is an alarm `{name}` already"),
      ));
    }
    let tmp = self
      .tmp_dir()
      .join(format!(".alarm.{name}.{}", std::process::id()));
    fs::DirBuilder::new()
      .recursive(true)
      .mode(0o700)
      .create(&tmp)
      .map_err(|e| at(e, &tmp))?;
    let dir = alarm.dir.to_string_lossy();
    let mut files = vec![
      ("command", alarm.command.as_str()),
      ("dir", &dir),
      ("note", alarm.note.as_str()),
      ("by", alarm.by.as_str()),
    ];
    if alarm.repeat {
      files.push(("repeat", ""));
    }
    for (file, content) in files {
      let file = tmp.join(file);
      fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .and_then(|mut f| io::Write::write_all(&mut f, content.as_bytes()))
        .map_err(|e| at(e, &file))?;
    }
    fs::rename(&tmp, &path).map_err(|e| {
      let _ = fs::remove_dir_all(&tmp);
      at(e, &path)
    })
  }

  /// Where the alarm of that name lies: a name is any but one with a `/` or a `.` first.
  pub fn alarm_path(&self, name: &str) -> io::Result<PathBuf> {
    if name.is_empty() || name.starts_with('.') || name.contains('/') {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid alarm name `{name}`: no `/`, and no `.` first"),
      ));
    }
    Ok(self.alarms_dir().join(name))
  }

  /// The alarm of that name, if it is set.
  pub fn alarm(&self, name: &str) -> io::Result<Option<Alarm>> {
    let path = self.alarm_path(name)?;
    let read = |file: &str| fs::read_to_string(path.join(file));
    let command = match read("command") {
      Ok(command) => command,
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
      Err(e) => return Err(at(e, &path)),
    };
    let optional = |file: &str| match read(file) {
      Ok(text) => Ok(text),
      Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
      Err(e) => Err(at(e, &path.join(file))),
    };
    Ok(Some(Alarm {
      name: name.to_string(),
      command,
      dir: PathBuf::from(optional("dir")?),
      note: optional("note")?,
      by: Some(optional("by")?)
        .filter(|by| !by.is_empty())
        .unwrap_or_else(|| "unknown".to_string()),
      repeat: path.join("repeat").exists(),
    }))
  }

  /// The names of the alarms set, in order.
  pub fn alarm_names(&self) -> io::Result<Vec<String>> {
    let alarms = self.alarms_dir();
    let entries = match fs::read_dir(&alarms) {
      Ok(entries) => entries,
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
      Err(e) => return Err(at(e, &alarms)),
    };
    let mut names = Vec::new();
    for entry in entries {
      let entry = entry.map_err(|e| at(e, &alarms))?;
      let name = entry.file_name().to_string_lossy().into_owned();
      if !name.starts_with('.') && entry.file_type().is_ok_and(|t| t.is_dir()) {
        names.push(name);
      }
    }
    names.sort();
    Ok(names)
  }

  /// The alarms set, in the order of their names.
  pub fn alarms(&self) -> io::Result<Vec<Alarm>> {
    let mut alarms = Vec::new();
    for name in self.alarm_names()? {
      alarms.extend(self.alarm(&name)?);
    }
    Ok(alarms)
  }

  /// Dismisses the alarm: whether it was set, so that of concurrent dismissers exactly one does.
  /// It is renamed into `tmp/` first, so that it goes at once and whole.
  pub fn dismiss_alarm(&self, name: &str) -> io::Result<bool> {
    let path = self.alarm_path(name)?;
    let aside = self
      .tmp_dir()
      .join(format!(".alarm.{name}.dismissed.{}", std::process::id()));
    match fs::rename(&path, &aside) {
      Ok(()) => fs::remove_dir_all(&aside).map_err(|e| at(e, &aside))?,
      Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
      Err(e) => return Err(at(e, &path)),
    }
    Ok(true)
  }
}
