//! Items that stream: linked as they start, whole once their writer lets go, and followed by
//! readers until then.

use std::{
  fs,
  io::{BufRead, BufReader, Read, Write},
  path::Path,
  process::{Command, Stdio},
  sync::mpsc,
  thread,
  time::{Duration, Instant},
};

use mindio::{Content, Kind, MindIo, Verdict, follow, whole};

fn agent(name: &str) -> MindIo {
  let root = std::env::temp_dir().join(format!("mindio-stream-{}-{name}", std::process::id()));
  let _ = fs::remove_dir_all(&root);
  MindIo::new(root)
}

fn only(dir: &Path) -> std::path::PathBuf {
  let mut entries: Vec<_> = fs::read_dir(dir)
    .unwrap()
    .map(|e| e.unwrap().path())
    .collect();
  assert_eq!(entries.len(), 1);
  entries.pop().unwrap()
}

#[test]
fn an_open_item_is_in_new_and_whole_once_its_writer_lets_go() {
  let agent = agent("open");
  let mut writer = agent.outbox().open("voice", "md").unwrap();
  writer.write_all(b"hel").unwrap();
  let path = only(&agent.outbox().new_dir());
  assert_eq!(path, writer.path);
  assert!(!whole(&path).unwrap());
  assert_eq!(
    fs::read_dir(agent.outbox().root.join("tmp"))
      .unwrap()
      .count(),
    0
  );
  drop(writer);
  assert!(whole(&path).unwrap());
  assert_eq!(fs::read(&path).unwrap(), b"hel");
}

#[test]
fn a_follower_reads_each_piece_as_it_comes_and_ends_with_the_item() {
  let agent = agent("follow");
  let mut writer = agent.outbox().open("voice", "md").unwrap();
  let taken = agent.outbox().take(|_| true).unwrap();
  let (tx, rx) = mpsc::channel();
  let path = taken[0].path.clone();
  let reader = thread::spawn(move || {
    let mut item = follow(&path).unwrap();
    let mut buf = [0; 64];
    loop {
      let n = item.read(&mut buf).unwrap();
      tx.send(buf[..n].to_vec()).unwrap();
      if n == 0 {
        return;
      }
    }
  });
  for piece in ["one ", "two ", "three"] {
    writer.write_all(piece.as_bytes()).unwrap();
    let got = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(got, piece.as_bytes());
  }
  assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
  writer.finish().unwrap();
  assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_empty());
  reader.join().unwrap();
}

#[test]
fn a_follower_keeps_following_an_item_that_moves() {
  let agent = agent("moves");
  let mut writer = agent.inbox().open("voice", "txt").unwrap();
  let path = writer.path.clone();
  let reader = thread::spawn(move || {
    let mut text = String::new();
    follow(&path).unwrap().read_to_string(&mut text).unwrap();
    text
  });
  thread::sleep(Duration::from_millis(100));
  let name = writer.path.file_name().unwrap().to_owned();
  fs::rename(&writer.path, agent.inbox().cur_dir().join(name)).unwrap();
  writer.write_all(b"after the move").unwrap();
  drop(writer);
  assert_eq!(reader.join().unwrap(), "after the move");
}

#[test]
fn a_take_waits_for_an_open_item_to_be_whole() {
  let agent = agent("take");
  let mut writer = agent.inbox().open("voice", "txt").unwrap();
  writer.write_all(b"half").unwrap();
  let start = Instant::now();
  let finisher = thread::spawn(move || {
    thread::sleep(Duration::from_millis(300));
    writer.write_all(b" and whole").unwrap();
  });
  let items = agent.inbox().take().unwrap();
  assert!(start.elapsed() >= Duration::from_millis(300));
  match &items[0].content {
    Content::Text(text) => assert_eq!(text, "half and whole"),
    Content::File { .. } => panic!("not text"),
  }
  finisher.join().unwrap();
}

#[test]
fn an_interrupt_wakes_at_the_first_byte_and_is_answered_with_the_wake() {
  let agent = agent("interrupt");
  agent.create().unwrap();
  fs::write(agent.waking_file(), "from=voice interrupt\n").unwrap();
  let writer = agent.open("voice", "txt", Kind::Event, &[]).unwrap();
  assert!(agent.interrupt_file().exists());
  assert_eq!(agent.wait().unwrap(), Verdict::Interrupt);
  assert!(!agent.interrupt_file().exists() && !agent.wake_file().exists());
  drop(writer);
  agent.wake().unwrap();
  assert_eq!(agent.wait().unwrap(), Verdict::Wake);
}

/// `mindio send` streams stdin into the item, printing its path as it starts, and `mindio follow`
/// prints the item as it grows.
#[test]
fn the_command_line_streams_both_ways() {
  let agent = agent("cli");
  let mindio = |args: &[&str]| {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mindio"));
    command.args(args).env("MINDIO", &agent.root);
    command
  };
  let mut send = mindio(&["send", "voice"])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .spawn()
    .unwrap();
  let mut input = send.stdin.take().unwrap();
  input.write_all(b"first\n").unwrap();
  let mut path = String::new();
  BufReader::new(send.stdout.take().unwrap())
    .read_line(&mut path)
    .unwrap();
  let path = path.trim_end().to_string();
  let mut follow = mindio(&["follow", &path])
    .stdout(Stdio::piped())
    .spawn()
    .unwrap();
  let mut lines = BufReader::new(follow.stdout.take().unwrap()).lines();
  assert_eq!(lines.next().unwrap().unwrap(), "first");
  input.write_all(b"second\n").unwrap();
  assert_eq!(lines.next().unwrap().unwrap(), "second");
  drop(input);
  assert!(send.wait().unwrap().success());
  assert!(lines.next().is_none());
  assert!(follow.wait().unwrap().success());
}
