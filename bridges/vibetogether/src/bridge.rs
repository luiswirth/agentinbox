//! The bridge: one receive of the agent kept open always, its items delivered into the inbox,
//! each received once it is there and read once it has left `new/`.

use std::{
  collections::BTreeMap,
  convert::Infallible,
  ffi::{OsStr, OsString},
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
const MAX_BACKOFF: Duration = Duration::from_secs(60);

const JOIN: &str = "application/vnd.vibetogether.join";
const LEAVE: &str = "application/vnd.vibetogether.leave";

pub struct Bridge {
  pub inbox: Inbox,
  pub client: Client,
}

/// The agent's positions in its stream, as the bridge knows them; `None` leaves the stored one.
#[derive(Default)]
struct Stream {
  /// The last item delivered into the inbox.
  received: Option<u64>,
  /// The last item that, with every item before it, has left `new/`.
  read: Option<u64>,
  /// The items delivered and not yet read, by their number, each with its file name in `new/`.
  pending: BTreeMap<u64, OsString>,
}

impl Bridge {
  /// Runs until the server refuses the agent or the inbox fails, and returns why.
  pub fn run(self) -> Result<Infallible, String> {
    self.inbox.create().map_err(|e| e.to_string())?;
    let mut stream = Stream::default();
    let (tx, rx) = mpsc::channel();
    let mut generation = 0;
    // The read position and state of the receive out, while one is.
    let mut out: Option<(Option<u64>, State)> = None;
    let mut next = Instant::now();
    let mut backoff: Option<Duration> = None;

    loop {
      // A receive this bridge replaced ends with 409, and what it answered comes again to the next.
      if let Ok((of, result)) = rx.recv_timeout(TICK)
        && of == generation
      {
        out = None;
        match result {
          Ok(answer) => {
            self.accept(&mut stream, answer)?;
            backoff = None;
            next = Instant::now();
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
      let issue = match out {
        Some(last) => last != (stream.read, state),
        None => Instant::now() >= next,
      };
      if issue {
        generation += 1;
        let (client, tx, received, read, of) = (
          self.client.clone(),
          tx.clone(),
          stream.received,
          stream.read,
          generation,
        );
        thread::spawn(move || {
          let _ = tx.send((of, client.receive(received, read, state, WAIT.as_secs())));
        });
        out = Some((read, state));
      }
    }
  }

  /// Takes in an answer: delivers each item, and rings the bell for a ping or a session's item.
  fn accept(&self, stream: &mut Stream, answer: Answer) -> Result<(), String> {
    let mut ring = false;
    for item in &answer.items {
      let (content, ext) = render(item, self.client.server());
      let path = self
        .inbox
        .deliver(content.as_bytes(), &item.entry.from, &ext)
        .map_err(|e| e.to_string())?;
      ring |= item.reach != Reach::Fyi;
      stream
        .pending
        .insert(item.n, path.file_name().unwrap_or_default().to_owned());
    }
    stream.received = Some(answer.items.last().map_or(answer.received, |item| item.n));
    if ring {
      self.inbox.ring().map_err(|e| e.to_string())?;
    }
    Ok(())
  }
}

/// Moves the read position over every pending item, oldest first, that is no longer `untaken`, and stops at the first
/// that is: an item is read only once it and every item before it have left `new/`.
fn advance(stream: &mut Stream, untaken: impl Fn(&OsStr) -> bool) {
  while let Some(entry) = stream.pending.first_entry() {
    if untaken(entry.get()) {
      break;
    }
    stream.read = Some(*entry.key());
    entry.remove();
  }
}

/// An item as the agent reads it, with its extension: a first line that says where it is from, then its text.
/// A payload without inline text is described with where to fetch it, never fetched.
pub fn render(item: &Item, server: &str) -> (String, String) {
  let entry = &item.entry;
  let header = format!(
    "Vibe Together item {} ({}), session {} ({}):",
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

#[cfg(test)]
mod tests {
  use std::fs;

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

  #[test]
  fn read_stops_at_the_first_untaken_item() {
    let mut stream = Stream {
      read: Some(4),
      pending: (5..=9)
        .map(|n| (n, OsString::from(n.to_string())))
        .collect(),
      ..Stream::default()
    };
    let untaken = ["7", "9"];
    advance(&mut stream, |name| {
      untaken.contains(&name.to_str().unwrap())
    });
    assert_eq!(stream.read, Some(6));
    assert_eq!(
      stream.pending.keys().copied().collect::<Vec<_>>(),
      [7, 8, 9]
    );

    advance(&mut stream, |name| name == "9");
    assert_eq!(stream.read, Some(8));
    advance(&mut stream, |_| false);
    assert_eq!(stream.read, Some(9));
    assert!(stream.pending.is_empty());
  }

  #[test]
  fn render_says_where_an_item_is_from() {
    let (text, ext) = render(
      &item(42, Reach::Ping, "text/markdown", Some("hi")),
      "https://vt",
    );
    assert_eq!(ext, "md");
    assert_eq!(
      text,
      "Vibe Together item 42 (ping), session general (main) (s_abc):\nhi\n"
    );
    let (join, _) = render(&item(3, Reach::Session, JOIN, None), "https://vt");
    assert_eq!(
      join,
      "Vibe Together item 3 (session), session general (main) (s_abc): luiswirth/lead joined.\n"
    );
    let (file, _) = render(&item(4, Reach::Fyi, "image/png", None), "https://vt");
    assert!(file.contains("image/png, named plot.png, 5 bytes"));
    assert!(file.contains("GET https://vt/api/sessions/s_abc/entries/104"));
  }

  /// Delivered items are received at once, and read once the agent took them; an fyi alone rings not.
  #[test]
  fn received_on_delivery_read_on_take() {
    let root = std::env::temp_dir().join(format!(
      "agentinbox-vibetogether-test-{}",
      std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let bridge = Bridge {
      inbox: Inbox::new(&root),
      client: Client::new("https://vt", String::new(), String::new(), WAIT),
    };
    let inbox = &bridge.inbox;
    inbox.create().unwrap();
    let answer = |ns: &[u64], reach, received| Answer {
      items: ns
        .iter()
        .map(|&n| item(n, reach, "text/plain", Some(&n.to_string())))
        .collect(),
      received,
    };
    let mut stream = Stream::default();
    bridge
      .accept(&mut stream, answer(&[], Reach::Fyi, 7))
      .unwrap();
    assert_eq!((stream.received, stream.read), (Some(7), None));
    bridge
      .accept(&mut stream, answer(&[8, 9], Reach::Fyi, 7))
      .unwrap();
    assert_eq!(stream.received, Some(9));
    assert!(!inbox.wake_file().exists());
    advance(&mut stream, |name| inbox.new_dir().join(name).exists());
    assert_eq!(stream.read, None);

    inbox.take().unwrap();
    bridge
      .accept(&mut stream, answer(&[10], Reach::Ping, 9))
      .unwrap();
    assert!(inbox.wake_file().exists());
    advance(&mut stream, |name| inbox.new_dir().join(name).exists());
    assert_eq!((stream.received, stream.read), (Some(10), Some(9)));
    let _ = fs::remove_dir_all(&root);
  }
}
