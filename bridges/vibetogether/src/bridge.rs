//! The bridge: one receive of the agent kept open always, its items delivered into the inbox,
//! and each acknowledged once it has left `new/`.

use std::{
  collections::{BTreeMap, HashMap},
  convert::Infallible,
  ffi::{OsStr, OsString},
  fs, io,
  io::Read,
  sync::mpsc,
  thread,
  time::{Duration, Instant},
};

use agentinbox::{Inbox, extension};

use crate::receive::{Answer, Client, Failure, Item, Reach, State};

/// How long a receive waits for an item before it is issued again.
pub const WAIT: Duration = Duration::from_secs(240);
/// How often the bridge looks at the awake lock and at `new/`.
const TICK: Duration = Duration::from_secs(1);
/// How often the bridge receives while items it delivered are not taken yet.
/// Vibe Together answers at once while unacknowledged items are left, so no receive can wait meanwhile;
/// this keeps the agent present, well within the grace Vibe Together gives after a receive.
const PENDING_INTERVAL: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// The start of the first line of every item the bridge delivers, followed by the item's number in the agent's stream.
const HEADER: &str = "Vibe Together item ";
const JOIN: &str = "application/vnd.vibetogether.join";
const LEAVE: &str = "application/vnd.vibetogether.leave";

pub struct Bridge {
  pub inbox: Inbox,
  pub client: Client,
}

/// Where the bridge stands.
#[derive(Default)]
struct Stream {
  /// The read position the bridge may acknowledge: every item up to it has left `new/`. `None` before the first answer.
  position: Option<u64>,
  /// The items delivered and not yet acknowledged, by their number, each with its file name in `new/`.
  pending: BTreeMap<u64, OsString>,
  /// The bridge's items found in `new/` at the start, by their number:
  /// delivered before a restart and not acknowledged, so Vibe Together sends them again.
  recognised: HashMap<u64, OsString>,
}

impl Bridge {
  /// Runs until the server refuses the agent or the inbox fails, and returns why.
  pub fn run(self) -> Result<Infallible, String> {
    self.inbox.create().map_err(|e| e.to_string())?;
    let mut stream = Stream {
      recognised: recognise(&self.inbox).map_err(|e| e.to_string())?,
      ..Stream::default()
    };
    let (tx, rx) = mpsc::channel();
    let mut generation = 0;
    // The last request issued, and whether it is still out.
    let mut last: Option<(Option<u64>, State)> = None;
    let mut out = false;
    let mut next = Instant::now();
    let mut backoff: Option<Duration> = None;

    loop {
      let timeout = if out {
        TICK
      } else {
        next.saturating_duration_since(Instant::now()).min(TICK)
      };
      for (of, result) in rx.recv_timeout(timeout).into_iter().chain(rx.try_iter()) {
        if of != generation {
          // A receive this bridge replaced: it ends with 409, and an answer it gave still counts.
          if let Ok(answer) = result {
            self.accept(&mut stream, answer)?;
          }
          continue;
        }
        out = false;
        match result {
          Ok(answer) => {
            self.accept(&mut stream, answer)?;
            backoff = None;
            next = Instant::now()
              + if stream.pending.is_empty() {
                Duration::ZERO
              } else {
                PENDING_INTERVAL
              };
          }
          Err(Failure::Retry(reason)) => {
            let delay = backoff.map_or(TICK, |delay| (delay * 2).min(MAX_BACKOFF));
            eprintln!("agentinbox-vibetogether: receive failed, again in {delay:?}: {reason}");
            next = Instant::now() + delay;
            backoff = Some(delay);
          }
          Err(Failure::Refused(reason)) => return Err(format!("refused: {reason}")),
        }
      }

      advance(&mut stream, |name| self.inbox.new_dir().join(name).exists());
      let state = match self.inbox.awake().map_err(|e| e.to_string())? {
        true => State::Awake,
        false => State::Asleep,
      };
      let changed = last.is_none_or(|(position, last)| last != state || stream.position > position);
      let issue = if out {
        changed
      } else {
        Instant::now() >= next || (changed && backoff.is_none())
      };
      if issue {
        generation += 1;
        let wait = if stream.pending.is_empty() {
          WAIT
        } else {
          Duration::ZERO
        };
        let (client, tx, position, of) =
          (self.client.clone(), tx.clone(), stream.position, generation);
        thread::spawn(move || {
          let _ = tx.send((of, client.receive(position, state, wait.as_secs())));
        });
        last = Some((position, state));
        out = true;
      }
    }
  }

  /// Takes in an answer: delivers each item not delivered before, and rings the bell for a ping or a session's item.
  fn accept(&self, stream: &mut Stream, answer: Answer) -> Result<(), String> {
    let position = stream.position.max(Some(answer.position));
    stream.position = position;
    stream.pending.retain(|&n, _| Some(n) > position);
    let mut ring = false;
    for item in answer.items {
      if Some(item.n) <= position || stream.pending.contains_key(&item.n) {
        continue;
      }
      let name = match stream.recognised.remove(&item.n) {
        Some(name) => name,
        None => {
          let (content, ext) = render(&item, self.client.server());
          let path = self
            .inbox
            .deliver(content.as_bytes(), &item.entry.from, &ext)
            .map_err(|e| e.to_string())?;
          ring |= item.reach != Reach::Fyi;
          path.file_name().unwrap_or_default().to_owned()
        }
      };
      stream.pending.insert(item.n, name);
    }
    if ring {
      self.inbox.ring().map_err(|e| e.to_string())?;
    }
    Ok(())
  }
}

/// Moves the position over every pending item, oldest first, that is no longer `untaken`, and stops at the first that is:
/// an item is acknowledged only once it and every item before it have left `new/`.
fn advance(stream: &mut Stream, untaken: impl Fn(&OsStr) -> bool) {
  while let Some(entry) = stream.pending.first_entry() {
    if untaken(entry.get()) {
      break;
    }
    stream.position = Some(*entry.key());
    entry.remove();
  }
}

/// An item as the agent reads it, with its extension: a first line that says where it is from, then its text.
/// A payload without inline text is described with where to fetch it, never fetched.
pub fn render(item: &Item, server: &str) -> (String, String) {
  let entry = &item.entry;
  let header = format!(
    "{HEADER}{} ({}), session {} ({}):",
    item.n,
    item.reach.as_str(),
    item.session.name,
    item.session.id,
  );
  let text = |text: String| (text, "txt".to_string());
  match (entry.ty.as_str(), &entry.text) {
    (JOIN, _) => text(format!("{header} {} joined.\n", entry.from)),
    (LEAVE, _) => text(format!("{header} {} left.\n", entry.from)),
    (ty, Some(body)) => {
      let ext = Some(ty)
        .filter(|ty| ty.starts_with("text/"))
        .and_then(extension)
        .unwrap_or_else(|| "txt".to_string());
      (format!("{header}\n{}\n", body.trim_end_matches('\n')), ext)
    }
    (ty, None) => {
      let filename = entry
        .filename
        .as_ref()
        .map(|name| format!(", named {name}"))
        .unwrap_or_default();
      text(format!(
        "{header}\nA payload of type {ty}{filename}, {} bytes, not fetched: it is at GET {server}/api/sessions/{}/entries/{}.\n",
        entry.size, item.session.id, entry.n,
      ))
    }
  }
}

/// The number in the agent's stream of an item the bridge delivered, from the item's first line.
fn number(first_line: &str) -> Option<u64> {
  first_line
    .strip_prefix(HEADER)?
    .split_once(" (")?
    .0
    .parse()
    .ok()
}

/// The bridge's items in `new/`, by their number, each with its file name.
fn recognise(inbox: &Inbox) -> io::Result<HashMap<u64, OsString>> {
  let mut found = HashMap::new();
  for entry in fs::read_dir(inbox.new_dir())? {
    let entry = entry?;
    let mut head = [0; 256];
    let Ok(len) = fs::File::open(entry.path()).and_then(|mut file| file.read(&mut head)) else {
      continue;
    };
    if let Some(n) = number(
      String::from_utf8_lossy(&head[..len])
        .lines()
        .next()
        .unwrap_or_default(),
    ) {
      found.insert(n, entry.file_name());
    }
  }
  Ok(found)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn item(n: u64, reach: Reach, ty: &str, text: Option<&str>) -> Item {
    serde_json::from_value(serde_json::json!({
      "n": n,
      "reach": reach.as_str(),
      "session": {"id": "s_abc", "name": "general (main)"},
      "entry": {"n": n + 100, "from": "luiswirth/lead",
                "type": ty, "size": 5, "filename": "plot.png", "text": text},
    }))
    .unwrap()
  }

  fn inbox(name: &str) -> Inbox {
    let root = std::env::temp_dir().join(format!(
      "agentinbox-vibetogether-test-{}-{name}",
      std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    Inbox::new(root)
  }

  #[test]
  fn position_stops_at_the_first_untaken_item() {
    let mut stream = Stream {
      position: Some(4),
      pending: (5..=9)
        .map(|n| (n, OsString::from(n.to_string())))
        .collect(),
      ..Stream::default()
    };
    let untaken = ["7", "9"];
    advance(&mut stream, |name| {
      untaken.contains(&name.to_str().unwrap())
    });
    assert_eq!(stream.position, Some(6));
    assert_eq!(
      stream.pending.keys().copied().collect::<Vec<_>>(),
      [7, 8, 9]
    );

    advance(&mut stream, |name| name == "9");
    assert_eq!(stream.position, Some(8));
    advance(&mut stream, |_| false);
    assert_eq!(stream.position, Some(9));
    assert!(stream.pending.is_empty());
  }

  #[test]
  fn header_carries_the_number() {
    let (text, ext) = render(
      &item(42, Reach::Ping, "text/markdown", Some("hi")),
      "https://vt",
    );
    assert_eq!(ext, "md");
    assert_eq!(
      text,
      "Vibe Together item 42 (ping), session general (main) (s_abc):\nhi\n"
    );
    assert_eq!(number(text.lines().next().unwrap()), Some(42));
    assert_eq!(number("Vibe Together item x (ping)"), None);
    assert_eq!(number("hello"), None);

    let (join, _) = render(&item(3, Reach::Session, JOIN, None), "https://vt");
    assert_eq!(
      join,
      "Vibe Together item 3 (session), session general (main) (s_abc): luiswirth/lead joined.\n"
    );
    let (file, _) = render(&item(4, Reach::Fyi, "image/png", None), "https://vt");
    assert!(file.contains("image/png, named plot.png, 5 bytes"));
    assert!(file.contains("GET https://vt/api/sessions/s_abc/entries/104"));
  }

  /// After a restart, the items Vibe Together sends again that are still in `new/` are not delivered again.
  #[test]
  fn restart_delivers_nothing_twice() {
    let inbox = inbox("restart");
    let bridge = || Bridge {
      inbox: Inbox::new(&inbox.root),
      client: Client::new("https://vt", String::new(), String::new(), WAIT),
    };
    let answer = |ns: &[u64], position| Answer {
      items: ns
        .iter()
        .map(|&n| item(n, Reach::Fyi, "text/plain", Some(&n.to_string())))
        .collect(),
      position,
    };
    let mut stream = Stream::default();
    bridge().accept(&mut stream, answer(&[1, 2], 0)).unwrap();
    assert!(!inbox.wake_file().exists());
    // The agent takes the first and the bridge acknowledges it; then the second arrives, and the bridge dies.
    inbox.take().unwrap();
    advance(&mut stream, |name| inbox.new_dir().join(name).exists());
    assert_eq!(stream.position, Some(2));
    bridge().accept(&mut stream, answer(&[3], 2)).unwrap();

    let mut stream = Stream {
      recognised: recognise(&inbox).unwrap(),
      ..Stream::default()
    };
    let mut again = answer(&[3], 2);
    again
      .items
      .push(item(4, Reach::Ping, "text/plain", Some("4")));
    bridge().accept(&mut stream, again).unwrap();
    let texts: Vec<_> = inbox
      .take()
      .unwrap()
      .iter()
      .map(|item| match &item.content {
        agentinbox::Content::Text(text) => text.lines().nth(1).unwrap().to_string(),
        agentinbox::Content::File { .. } => panic!("not text"),
      })
      .collect();
    assert_eq!(texts, ["3", "4"]);
    assert!(inbox.wake_file().exists());
    advance(&mut stream, |name| inbox.new_dir().join(name).exists());
    assert_eq!(stream.position, Some(4));
  }
}
