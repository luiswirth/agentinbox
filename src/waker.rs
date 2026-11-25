//! The machine's waker: one process that watches every agent's IO in a directory, answers each
//! agent's wake by running its `resume` while the agent is offline, and runs its alarms.

use std::{
  collections::{HashMap, HashSet},
  ffi::OsString,
  fs, io,
  os::unix::{
    fs::{DirBuilderExt, MetadataExt},
    process::{CommandExt, ExitStatusExt},
  },
  path::Path,
  process::{Command, ExitStatus, Stdio},
  sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
  },
  thread,
};

use mindio::{Alarm, Kind, MindIo};
use notify::{RecursiveMode, Watcher};

/// One agent the waker keeps, by the nudges to its answerer and its alarms' keeper.
struct Kept {
  nudge: Sender<()>,
  alarms: Sender<()>,
}

pub fn run(dir: &Path) -> io::Result<()> {
  stop_on_signal()?;
  fs::DirBuilder::new()
    .recursive(true)
    .mode(0o700)
    .create(dir)?;
  let dir = dir.canonicalize()?;
  let (tx, rx) = mpsc::channel::<Option<OsString>>();
  let watched = dir.clone();
  let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
    match event {
      Ok(event) if !mindio::changes(&event) => {}
      Ok(event) => {
        for path in event.paths {
          let agent = path
            .strip_prefix(&watched)
            .ok()
            .and_then(|rest| rest.components().next())
            .map(|c| c.as_os_str().to_owned());
          let _ = tx.send(agent);
        }
      }
      // An overflow or the like: look at every agent again.
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

/// Brings one agent in line with its directory: starts keeping it and nudges its answerer; drops
/// it where it is gone.
fn keep(dir: &Path, name: &OsString, kept: &mut HashMap<OsString, Kept>) {
  let root = dir.join(name);
  if name.to_string_lossy().starts_with('.') || !root.is_dir() {
    kept.remove(name);
    return;
  }
  let entry = kept.entry(name.clone()).or_insert_with(|| {
    let (nudge, nudges) = mpsc::channel();
    let mindio = MindIo::new(&root);
    thread::spawn(move || {
      if let Err(error) = answer(&mindio, &nudges) {
        eprintln!("mindio: {}: {error}", mindio.root.display());
      }
    });
    let (alarms, alarm_nudges) = mpsc::channel();
    let mindio = MindIo::new(&root);
    thread::spawn(move || keep_alarms(&mindio, &alarm_nudges));
    Kept { nudge, alarms }
  });
  let _ = entry.nudge.send(());
  let _ = entry.alarms.send(());
}

/// Answers the agent's wakes, nudged at each change in its directory, until the directory is
/// gone: once the agent is offline, answers the wake and, while something is untaken, runs
/// `resume` as the agent's turn, holding the awake lock. A wake that comes while a mind is awake
/// or asleep is that mind's, and only what it leaves untaken is answered. It stops answering a
/// wake after a run that failed, or that took nothing in, so that a broken command doesn't run in
/// a loop.
fn answer(mindio: &MindIo, nudges: &Receiver<()>) -> io::Result<()> {
  let inbox = mindio.inbox();
  loop {
    while !mindio.wake_file().exists() || mindio.resume()?.is_none() {
      if nudges.recv().is_err() {
        return Ok(());
      }
    }
    // A lock's release changes no file, so the wait for it blocks on the lock itself.
    mindio.until_no_sleeper()?;
    let _turn = mindio.hold_awake()?;
    if mindio.asleep()? || mindio.answer_wake()?.is_none() {
      continue;
    }
    while let Some(oldest) = inbox.oldest_untaken()? {
      let Some(resume) = mindio.resume()? else {
        break;
      };
      let mut command = Command::new("sh");
      command
        .args(["-c", &resume])
        .env("MINDIO", &mindio.root)
        .stdin(Stdio::null());
      let status = unblock_signals(&mut command).status()?;
      if !status.success() {
        eprintln!(
          "mindio: {}: the resume failed: {status}",
          mindio.root.display()
        );
        break;
      }
      if inbox.new_dir().join(&oldest).exists() {
        eprintln!(
          "mindio: {}: the resume took nothing in",
          mindio.root.display()
        );
        break;
      }
    }
  }
}

/// The process groups of the alarms' commands that run, which the waker stops as it is stopped.
/// Its lock is taken before any alarm's, and held while a command starts, so that none starts
/// unseen.
static RUNNING: Mutex<Option<HashSet<i32>>> = Mutex::new(None);
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Stops the alarms' commands, each its own process group, as the waker is stopped by a signal,
/// and ends the waker. Called before any thread starts, so that every thread blocks the signals
/// and only this one takes them; a child starts with none blocked.
fn stop_on_signal() -> io::Result<()> {
  // SAFETY: an empty set, filled with valid signals, blocked in this thread before any other.
  let set = unsafe {
    let mut set: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut set);
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
      libc::sigaddset(&mut set, signal);
    }
    let error = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    if error != 0 {
      return Err(io::Error::from_raw_os_error(error));
    }
    set
  };
  thread::spawn(move || {
    let mut signal = 0;
    // SAFETY: the set made above, and room for the signal taken.
    unsafe { libc::sigwait(&set, &mut signal) };
    STOPPING.store(true, Ordering::SeqCst);
    let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    for &group in running.iter().flatten() {
      // SAFETY: a signal to a process group this waker started.
      unsafe { libc::kill(-group, libc::SIGTERM) };
    }
    std::process::exit(0);
  });
  Ok(())
}

/// Starts the command with no signal blocked, as the waker's threads block those it stops on,
/// and a child inherits the mask.
fn unblock_signals(command: &mut Command) -> &mut Command {
  // SAFETY: between fork and exec only async-signal-safe calls, on a set on the stack.
  unsafe {
    command.pre_exec(|| {
      let mut set: libc::sigset_t = std::mem::zeroed();
      libc::sigemptyset(&mut set);
      match libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut()) {
        0 => Ok(()),
        error => Err(io::Error::from_raw_os_error(error)),
      }
    })
  }
}

/// One alarm the keeper runs, by the directory it was set as.
struct Runner {
  ino: u64,
  state: Arc<Mutex<RunnerState>>,
}

#[derive(Default)]
struct RunnerState {
  /// The process group of its command, while it runs.
  group: Option<i32>,
  /// Dismissed, or set anew: its command is stopped, and it tells nothing.
  stopped: bool,
}

fn stop(state: &Mutex<RunnerState>) {
  let _running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
  let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
  state.stopped = true;
  if let Some(group) = state.group {
    // SAFETY: a signal to a process group this waker started.
    unsafe { libc::kill(-group, libc::SIGTERM) };
  }
}

/// Keeps the agent's alarms running, nudged at each change in its directory, until it is gone:
/// runs each alarm set, and stops each one dismissed or set anew under its name.
fn keep_alarms(mindio: &MindIo, nudges: &Receiver<()>) {
  let mut runners: HashMap<String, Runner> = HashMap::new();
  loop {
    match alarms_set(mindio) {
      Ok(set) => {
        runners.retain(|name, runner| {
          let kept = set.get(name) == Some(&runner.ino);
          if !kept {
            stop(&runner.state);
          }
          kept
        });
        for (name, ino) in set {
          runners.entry(name.clone()).or_insert_with(|| {
            let state = Arc::new(Mutex::new(RunnerState::default()));
            let mindio = MindIo::new(&mindio.root);
            let runs = state.clone();
            thread::spawn(move || run_alarm(&mindio, &name, &runs));
            Runner { ino, state }
          });
        }
      }
      Err(error) => eprintln!("mindio: {}: {error}", mindio.root.display()),
    }
    if nudges.recv().is_err() {
      for runner in runners.values() {
        stop(&runner.state);
      }
      return;
    }
  }
}

/// The alarms set, each by the inode of its directory, which tells an alarm set anew under the
/// same name.
fn alarms_set(mindio: &MindIo) -> io::Result<HashMap<String, u64>> {
  let mut set = HashMap::new();
  for name in mindio.alarm_names()? {
    if let Ok(metadata) = fs::metadata(mindio.alarm_path(&name)?) {
      set.insert(name, metadata.ino());
    }
  }
  Ok(set)
}

/// Runs the alarm's command, in a process group of its own, and tells the agent as it ends: as
/// it goes off where it succeeded, then again where it repeats, or else dismissed; where it
/// failed, that it is dismissed. A command killed by a signal from outside, such as the service
/// manager's as it stops the waker, has not failed: the alarm stays, and runs again as the waker
/// starts again.
fn run_alarm(mindio: &MindIo, name: &str, state: &Mutex<RunnerState>) {
  if let Err(error) = run_alarm_until_done(mindio, name, state) {
    eprintln!(
      "mindio: {}: the alarm `{name}`: {error}",
      mindio.root.display()
    );
  }
}

fn run_alarm_until_done(mindio: &MindIo, name: &str, state: &Mutex<RunnerState>) -> io::Result<()> {
  loop {
    let Some(alarm) = mindio.alarm(name)? else {
      return Ok(());
    };
    let mut command = Command::new("sh");
    command
      .args(["-c", &alarm.command])
      .env("MINDIO", &mindio.root)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .process_group(0);
    unblock_signals(&mut command);
    if !alarm.dir.as_os_str().is_empty() {
      command.current_dir(&alarm.dir);
    }
    let child = {
      let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
      let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
      if state.stopped || STOPPING.load(Ordering::SeqCst) {
        return Ok(());
      }
      let child = command.spawn();
      if let Ok(child) = &child {
        let group = child.id() as i32;
        state.group = Some(group);
        running.get_or_insert_default().insert(group);
      }
      child
    };
    let status = child.and_then(|mut child| {
      let status = child.wait();
      let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
      let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
      if let Some(group) = state.group.take() {
        running.get_or_insert_default().remove(&group);
      }
      status
    });
    {
      let state = state.lock().unwrap_or_else(|e| e.into_inner());
      if state.stopped || STOPPING.load(Ordering::SeqCst) || !mindio.alarm_path(name)?.exists() {
        return Ok(());
      }
    }
    match status {
      Ok(status) if status.signal().is_some() => {
        eprintln!(
          "mindio: {}: the alarm `{name}` was stopped ({status}): it runs again as the waker starts again",
          mindio.root.display()
        );
        return Ok(());
      }
      Ok(status) if status.success() => {
        tell(mindio, &alarm, &alarm.message())?;
        if !alarm.repeat {
          mindio.dismiss_alarm(name)?;
          return Ok(());
        }
      }
      failed => {
        tell(mindio, &alarm, &failure(&alarm, failed))?;
        mindio.dismiss_alarm(name)?;
        return Ok(());
      }
    }
  }
}

fn failure(alarm: &Alarm, failed: io::Result<ExitStatus>) -> String {
  let why = match failed {
    Ok(status) => status.to_string(),
    Err(error) => error.to_string(),
  };
  format!(
    "The alarm `{}` is dismissed: its command failed ({why}).\n\n{}",
    alarm.name, alarm.command
  )
}

/// Delivers what the alarm tells, from `alarm`, as an event.
fn tell(mindio: &MindIo, alarm: &Alarm, text: &str) -> io::Result<()> {
  let fields = [("alarm", alarm.name.as_str()), ("by", &alarm.by)];
  mindio.deliver(text.as_bytes(), "alarm", "txt", Kind::Event, &fields)?;
  Ok(())
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
