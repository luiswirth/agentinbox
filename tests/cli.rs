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
