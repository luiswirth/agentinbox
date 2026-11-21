//! The command line: the awake lock across processes, and the readings a take runs.

use std::{
  fs,
  path::{Path, PathBuf},
  process::{Command, Stdio},
  thread,
  time::{Duration, Instant},
};

use agentinbox::Inbox;

fn home(name: &str) -> PathBuf {
  let home = std::env::temp_dir().join(format!("agentinbox-cli-{}-{name}", std::process::id()));
  let _ = fs::remove_dir_all(&home);
  fs::create_dir_all(&home).unwrap();
  home
}

fn agentinbox(home: &Path, args: &[&str]) -> Command {
  let mut command = Command::new(env!("CARGO_BIN_EXE_agentinbox"));
  command
    .args(args)
    .env_clear()
    .env("PATH", std::env::var_os("PATH").unwrap())
    .env("HOME", home)
    .env("AGENT_INBOX", home.join("inbox"));
  command
}

fn until(what: &str, check: impl Fn() -> bool) {
  let start = Instant::now();
  while !check() {
    assert!(start.elapsed() < Duration::from_secs(10), "never {what}");
    thread::sleep(Duration::from_millis(20));
  }
}

/// `awake` holds the lock across processes for as long as its stdin is open, and the system
/// releases it when the pipe closes, also when the holder is killed.
#[test]
fn awake_is_held_while_stdin_is_open() {
  let home = home("awake");
  let inbox = Inbox::new(home.join("inbox"));
  let hold = || {
    agentinbox(&home, &["awake"])
      .stdin(Stdio::piped())
      .spawn()
      .unwrap()
  };

  let mut holder = hold();
  until("awake", || inbox.awake().unwrap());
  drop(holder.stdin.take());
  assert!(holder.wait().unwrap().success());
  until("asleep", || !inbox.awake().unwrap());

  let mut holder = hold();
  until("awake", || inbox.awake().unwrap());
  holder.kill().unwrap();
  holder.wait().unwrap();
  until("asleep", || !inbox.awake().unwrap());
  fs::remove_dir_all(&home).unwrap();
}

/// A take runs each reading first, so that what it delivers is taken in the same step, and a
/// failing reading is passed over.
#[test]
fn a_take_runs_the_readings_first() {
  let home = home("readings");
  fs::create_dir_all(home.join(".config/agentinbox")).unwrap();
  fs::write(
    home.join(".config/agentinbox/readings"),
    "# the time\necho noon | agentinbox deliver --from clock\n\nexit 3\n",
  )
  .unwrap();
  let mut take = agentinbox(&home, &["take"]);
  // The reading finds this very binary as `agentinbox`.
  let bin = Path::new(env!("CARGO_BIN_EXE_agentinbox"))
    .parent()
    .unwrap();
  let path = std::env::join_paths(
    std::iter::once(bin.to_path_buf())
      .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
  )
  .unwrap();
  let out = take.env("PATH", path).output().unwrap();
  assert!(out.status.success());
  let stdout = String::from_utf8(out.stdout).unwrap();
  assert!(stdout.starts_with("<inbox-item from=\"clock\""), "{stdout}");
  assert!(stdout.contains("\nnoon\n"), "{stdout}");
  let stderr = String::from_utf8(out.stderr).unwrap();
  assert!(stderr.contains("the reading `exit 3` failed"), "{stderr}");
  fs::remove_dir_all(&home).unwrap();
}

/// The PATH with this very binary first, so that what a test runs finds it as `agentinbox`.
fn path_with_binary() -> std::ffi::OsString {
  let bin = Path::new(env!("CARGO_BIN_EXE_agentinbox"))
    .parent()
    .unwrap();
  std::env::join_paths(
    std::iter::once(bin.to_path_buf())
      .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
  )
  .unwrap()
}

/// Stops the waker when the test ends, also when it fails, as a service manager does.
struct Running(std::process::Child);
impl Drop for Running {
  fn drop(&mut self) {
    terminate(self.0.id());
    let _ = self.0.wait();
  }
}

fn terminate(pid: u32) {
  // SAFETY: a signal to a process this test started.
  unsafe { libc::kill(pid as i32, libc::SIGTERM) };
}

fn alive(pid: i32) -> bool {
  // SAFETY: signal 0 only tests for the process.
  unsafe { libc::kill(pid, 0) == 0 }
}

fn waker(home: &Path) -> Running {
  Running(
    agentinbox(home, &["waker", home.join("inboxes").to_str().unwrap()])
      .env("PATH", path_with_binary())
      .spawn()
      .unwrap(),
  )
}

fn deliver(home: &Path, inbox: &Path, text: &str) {
  let mut deliver = agentinbox(home, &["deliver", "--from", "luis", "--wake"])
    .env("AGENT_INBOX", inbox)
    .stdin(Stdio::piped())
    .spawn()
    .unwrap();
  use std::io::Write;
  deliver
    .stdin
    .take()
    .unwrap()
    .write_all(text.as_bytes())
    .unwrap();
  assert!(deliver.wait().unwrap().success());
}

/// The waker answers a ring of an offline agent by running its `resume`, and leaves a ring to a
/// mind that is asleep until it is gone.
#[test]
fn the_waker_resumes_an_offline_agent_and_leaves_a_sleeper_its_rings() {
  let home = home("waker");
  let root = home.join("inboxes/agent");
  let inbox = Inbox::new(&root);
  inbox.create().unwrap();
  let log = home.join("log");
  fs::write(
    inbox.resume_file(),
    format!("agentinbox take >>'{}'", log.display()),
  )
  .unwrap();
  let _waker = waker(&home);

  deliver(&home, &root, "first");
  until("resumed", || {
    fs::read_to_string(&log).is_ok_and(|log| log.contains("first"))
  });

  let mut sleeper = agentinbox(&home, &["asleep"])
    .env("AGENT_INBOX", &root)
    .stdin(Stdio::piped())
    .spawn()
    .unwrap();
  until("asleep", || inbox.asleep().unwrap());
  deliver(&home, &root, "second");
  thread::sleep(Duration::from_millis(500));
  assert!(!fs::read_to_string(&log).unwrap().contains("second"));
  assert!(inbox.wake_file().exists());

  drop(sleeper.stdin.take());
  assert!(sleeper.wait().unwrap().success());
  until("resumed once the sleeper left", || {
    fs::read_to_string(&log).unwrap().contains("second")
  });
  fs::remove_dir_all(&home).unwrap();
}

/// A run that takes nothing in ends the answer: a broken resume never loops.
#[test]
fn a_resume_that_takes_nothing_in_runs_once() {
  let home = home("broken");
  let root = home.join("inboxes/agent");
  let inbox = Inbox::new(&root);
  inbox.create().unwrap();
  let count = home.join("count");
  fs::write(
    inbox.resume_file(),
    format!("echo run >>'{}'", count.display()),
  )
  .unwrap();
  let _waker = waker(&home);
  deliver(&home, &root, "hello");
  until("run", || count.exists());
  thread::sleep(Duration::from_millis(500));
  assert_eq!(fs::read_to_string(&count).unwrap(), "run\n");
  fs::remove_dir_all(&home).unwrap();
}

/// `asleep --pid` holds the lock until that process ends.
#[test]
fn asleep_is_held_while_the_process_lives() {
  let home = home("asleep-pid");
  let inbox = Inbox::new(home.join("inbox"));
  let mut process = Command::new("sleep").arg("100").spawn().unwrap();
  let mut holder = agentinbox(&home, &["asleep", "--pid", &process.id().to_string()])
    .spawn()
    .unwrap();
  until("asleep", || inbox.asleep().unwrap());
  process.kill().unwrap();
  process.wait().unwrap();
  assert!(holder.wait().unwrap().success());
  assert!(!inbox.asleep().unwrap());
  fs::remove_dir_all(&home).unwrap();
}

fn set_alarm(home: &Path, root: &Path, args: &[&str]) {
  let mut all = vec!["alarm", "set"];
  all.extend_from_slice(args);
  let status = agentinbox(home, &all)
    .env("AGENT_INBOX", root)
    .current_dir(home)
    .status()
    .unwrap();
  assert!(status.success());
}

fn told(inbox: &Inbox) -> Vec<String> {
  let mut told: Vec<String> = fs::read_dir(inbox.new_dir())
    .map(|dir| {
      dir
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().contains(".alarm."))
        .map(|p| fs::read_to_string(p).unwrap())
        .collect()
    })
    .unwrap_or_default();
  told.sort();
  told
}

/// An alarm set while no waker runs goes off once it starts, rings, tells its note and is gone.
#[test]
fn an_alarm_goes_off_once_and_outlasts_the_waker() {
  let home = home("alarm-once");
  let root = home.join("inboxes/agent");
  let inbox = Inbox::new(&root);
  set_alarm(
    &home,
    &root,
    &[
      "--in", "1s", "--note", "stand up", "--name", "up", "--by", "luis",
    ],
  );
  let list = agentinbox(&home, &["alarm", "list"])
    .env("AGENT_INBOX", &root)
    .output()
    .unwrap();
  assert!(
    String::from_utf8(list.stdout)
      .unwrap()
      .starts_with("up, set by luis: agentinbox until ")
  );
  let _waker = waker(&home);
  until("gone off", || !told(&inbox).is_empty());
  assert_eq!(told(&inbox), ["stand up\n\n(The alarm `up`, set by luis.)"]);
  assert!(inbox.wake_file().exists());
  until("dismissed", || inbox.alarm_names().unwrap().is_empty());
  fs::remove_dir_all(&home).unwrap();
}

/// A repeating alarm goes off again and again, until dismissing it stops its command.
#[test]
fn dismissing_a_repeating_alarm_stops_its_command() {
  let home = home("alarm-repeat");
  let root = home.join("inboxes/agent");
  let inbox = Inbox::new(&root);
  let _waker = waker(&home);
  set_alarm(
    &home,
    &root,
    &[
      "--repeat",
      "--name",
      "tick",
      "sleep 0.1; echo $$ >pid; sleep 100 & wait; true",
    ],
  );
  let pid = || {
    fs::read_to_string(home.join("pid"))
      .ok()
      .and_then(|p| p.trim().parse::<i32>().ok())
  };
  until("running", || pid().is_some());
  let first = pid().unwrap();
  assert!(alive(first));
  let dismiss = agentinbox(&home, &["alarm", "dismiss", "tick"])
    .env("AGENT_INBOX", &root)
    .status()
    .unwrap();
  assert!(dismiss.success());
  until("stopped", || !alive(first));
  thread::sleep(Duration::from_millis(300));
  assert!(told(&inbox).is_empty());
  assert!(inbox.alarm_names().unwrap().is_empty());
  fs::remove_dir_all(&home).unwrap();
}

/// A repeating alarm runs again after each time it goes off.
#[test]
fn a_repeating_alarm_repeats() {
  let home = home("alarm-repeats");
  let root = home.join("inboxes/agent");
  let inbox = Inbox::new(&root);
  let _waker = waker(&home);
  set_alarm(
    &home,
    &root,
    &["--repeat", "--name", "tick", "--note", "tick", "sleep 0.2"],
  );
  until("gone off three times", || told(&inbox).len() >= 3);
  assert_eq!(inbox.alarm_names().unwrap(), ["tick"]);
  fs::remove_dir_all(&home).unwrap();
}

/// A command that fails dismisses its alarm and says so, once.
#[test]
fn a_failing_alarm_is_told_and_dismissed() {
  let home = home("alarm-fails");
  let root = home.join("inboxes/agent");
  let inbox = Inbox::new(&root);
  let _waker = waker(&home);
  set_alarm(&home, &root, &["--repeat", "--name", "broken", "exit 3"]);
  until("told", || !told(&inbox).is_empty());
  until("dismissed", || inbox.alarm_names().unwrap().is_empty());
  thread::sleep(Duration::from_millis(300));
  let told = told(&inbox);
  assert_eq!(told.len(), 1);
  assert!(told[0].starts_with("The alarm `broken` is dismissed: its command failed"));
  fs::remove_dir_all(&home).unwrap();
}

/// Stopping the waker stops the alarms' commands and keeps the alarms, which run again as it starts.
#[test]
fn stopping_the_waker_keeps_the_alarms() {
  let home = home("alarm-restart");
  let root = home.join("inboxes/agent");
  let inbox = Inbox::new(&root);
  let waker_ = waker(&home);
  set_alarm(
    &home,
    &root,
    &["--name", "later", "echo $$ >>pids; sleep 100 & wait"],
  );
  let pids = || {
    fs::read_to_string(home.join("pids"))
      .unwrap_or_default()
      .lines()
      .map(|p| p.parse::<i32>().unwrap())
      .collect::<Vec<_>>()
  };
  until("running", || pids().len() == 1);
  drop(waker_);
  let first = pids()[0];
  until("stopped", || !alive(first));
  assert_eq!(inbox.alarm_names().unwrap(), ["later"]);
  assert!(told(&inbox).is_empty());
  let _waker = waker(&home);
  until("running again", || pids().len() == 2);
  fs::remove_dir_all(&home).unwrap();
}

/// `until` returns at once for a time passed, and waits for one to come.
#[test]
fn until_waits_for_the_time() {
  let home = home("until");
  let start = Instant::now();
  assert!(
    agentinbox(&home, &["until", "2000-01-01T00:00:00Z"])
      .status()
      .unwrap()
      .success()
  );
  assert!(start.elapsed() < Duration::from_secs(1));
  let soon = jiff::Timestamp::now() + jiff::SignedDuration::from_millis(700);
  assert!(
    agentinbox(&home, &["until", &soon.to_string()])
      .status()
      .unwrap()
      .success()
  );
  assert!(start.elapsed() >= Duration::from_millis(700));
  fs::remove_dir_all(&home).unwrap();
}
