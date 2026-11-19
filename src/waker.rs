//! The machine's waker: one process that watches every inbox in a directory, answers each one's
//! bell by running its `resume` while the agent is offline.

use std::{
  collections::HashMap,
  ffi::OsString,
  fs, io,
  os::unix::fs::DirBuilderExt,
  path::Path,
  process::{Command, Stdio},
  sync::mpsc::{self, Receiver, Sender},
  thread,
};

use agentinbox::Inbox;
use notify::{RecursiveMode, Watcher};

/// One inbox the waker keeps, by the nudge to its answerer.
struct Kept {
  nudge: Sender<()>,
}

pub fn run(dir: &Path) -> io::Result<()> {
  fs::DirBuilder::new()
    .recursive(true)
    .mode(0o700)
    .create(dir)?;
  let dir = dir.canonicalize()?;
  let (tx, rx) = mpsc::channel::<Option<OsString>>();
  let watched = dir.clone();
  let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
    match event {
      Ok(event) => {
        for path in event.paths {
          let inbox = path
            .strip_prefix(&watched)
            .ok()
            .and_then(|rest| rest.components().next())
            .map(|c| c.as_os_str().to_owned());
          let _ = tx.send(inbox);
        }
      }
      // An overflow or the like: look at every inbox again.
      Err(_) => {
        let _ = tx.send(None);
      }
    }
  })
  .map_err(io::Error::other)?;
  watcher
    .watch(&dir, RecursiveMode::Recursive)
    .map_err(io::Error::other)?;

  let mut kept: HashMap<OsString, Kept> = HashMap::new();
  for entry in fs::read_dir(&dir)? {
    keep(&dir, &entry?.file_name(), &mut kept);
  }
  for changed in rx {
    match changed {
      Some(name) => keep(&dir, &name, &mut kept),
      None => {
        let mut names: Vec<OsString> = kept.keys().cloned().collect();
        for entry in fs::read_dir(&dir)? {
          names.push(entry?.file_name());
        }
        for name in names {
          keep(&dir, &name, &mut kept);
        }
      }
    }
  }
  Ok(())
}

/// Brings one inbox in line with its directory: starts keeping it and nudges its answerer;
/// drops it where it is gone.
fn keep(dir: &Path, name: &OsString, kept: &mut HashMap<OsString, Kept>) {
  let root = dir.join(name);
  if name.to_string_lossy().starts_with('.') || !root.is_dir() {
    kept.remove(name);
    return;
  }
  let entry = kept.entry(name.clone()).or_insert_with(|| {
    let (nudge, nudges) = mpsc::channel();
    let inbox = Inbox::new(&root);
    thread::spawn(move || {
      if let Err(error) = answer(&inbox, &nudges) {
        eprintln!("agentinbox: {}: {error}", inbox.root.display());
      }
    });
    Kept { nudge }
  });
  let _ = entry.nudge.send(());
}

/// Answers the inbox's bell, nudged at each change in it, until the inbox is gone: once the
/// agent is offline, takes the ring and, while something is untaken, runs `resume` as the
/// agent's turn, holding the awake lock. A ring that comes while a mind is awake or asleep is
/// that mind's, and only what it leaves untaken is answered. It stops answering a ring after a
/// run that failed, or that took nothing in, so that a broken command doesn't run in a loop.
fn answer(inbox: &Inbox, nudges: &Receiver<()>) -> io::Result<()> {
  loop {
    while !inbox.wake_file().exists() || inbox.resume()?.is_none() {
      if nudges.recv().is_err() {
        return Ok(());
      }
    }
    // A lock's release changes no file, so the wait for it blocks on the lock itself.
    inbox.until_no_sleeper()?;
    let _turn = inbox.hold_awake()?;
    if inbox.asleep()? || !inbox.unring()? {
      continue;
    }
    while let Some(oldest) = inbox.oldest_untaken()? {
      let Some(resume) = inbox.resume()? else {
        break;
      };
      let status = Command::new("sh")
        .args(["-c", &resume])
        .env("AGENT_INBOX", &inbox.root)
        .stdin(Stdio::null())
        .status()?;
      if !status.success() {
        eprintln!(
          "agentinbox: {}: the resume failed: {status}",
          inbox.root.display()
        );
        break;
      }
      if inbox.new_dir().join(&oldest).exists() {
        eprintln!(
          "agentinbox: {}: the resume took nothing in",
          inbox.root.display()
        );
        break;
      }
    }
  }
}

/// Blocks until the process `pid` has ended, at once where there is none.
pub fn until_ended(pid: i32) -> io::Result<()> {
  #[cfg(target_os = "linux")]
  {
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if fd < 0 {
      let error = io::Error::last_os_error();
      return match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(()),
        _ => Err(error),
      };
    }
    let mut poll = libc::pollfd {
      fd,
      events: libc::POLLIN,
      revents: 0,
    };
    loop {
      // SAFETY: one valid pollfd, waited on without a timeout.
      let n = unsafe { libc::poll(&mut poll, 1, -1) };
      if n >= 0 {
        break;
      }
      let error = io::Error::last_os_error();
      if error.kind() != io::ErrorKind::Interrupted {
        return Err(error);
      }
    }
    // SAFETY: the descriptor opened above, closed once.
    unsafe { libc::close(fd) };
    Ok(())
  }
  #[cfg(target_os = "macos")]
  {
    // SAFETY: kqueue returns a new descriptor or -1.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
      return Err(io::Error::last_os_error());
    }
    let change = libc::kevent {
      ident: pid as usize,
      filter: libc::EVFILT_PROC,
      flags: libc::EV_ADD | libc::EV_ONESHOT,
      fflags: libc::NOTE_EXIT,
      data: 0,
      udata: std::ptr::null_mut(),
    };
    // SAFETY: zeroes are a valid kevent to receive into.
    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
    let result = loop {
      // SAFETY: one change and room for one event, waited on without a timeout.
      let n = unsafe { libc::kevent(kq, &change, 1, &mut event, 1, std::ptr::null()) };
      if n > 0 && event.flags & libc::EV_ERROR != 0 {
        break match event.data as i32 {
          libc::ESRCH => Ok(()),
          code => Err(io::Error::from_raw_os_error(code)),
        };
      }
      if n >= 0 {
        break Ok(());
      }
      let error = io::Error::last_os_error();
      match error.raw_os_error() {
        Some(libc::EINTR) => continue,
        Some(libc::ESRCH) => break Ok(()),
        _ => break Err(error),
      }
    };
    // SAFETY: the descriptor opened above, closed once.
    unsafe { libc::close(kq) };
    result
  }
}

/// Detaches from the session of whoever started this process, so that it outlives it.
pub fn detach() {
  // SAFETY: setsid takes nothing; it fails harmlessly where the process leads a group already.
  unsafe { libc::setsid() };
}
