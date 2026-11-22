//! The peers: other machines of the user's, whose agents are seen beside this machine's, since
//! inboxes are per machine. The inbox knows no machine: a peer is a name and a command that
//! lists its agents, a line `<name> <command>` of `agentinbox/peers` in the user's config
//! directory, such as `lwirth-macbook ssh -o ConnectTimeout=3 lwirth-macbook agentinbox agents
//! --here --porcelain`, run with `sh -c`.
//!
//! Its agents are named `<peer>:<agent>`. What a peer listed last is kept in the user's cache
//! directory, `agentinbox/peers/<name>`, so that a look at every step, as the sense `others`
//! takes, asks a peer at most once in a while; a peer out of reach keeps what it listed last,
//! and is asked again only once that while has passed.

use std::{
  env,
  fs::{self, TryLockError},
  io::{self, Read},
  os::unix::fs::{DirBuilderExt, OpenOptionsExt},
  path::{Path, PathBuf},
  process::{Command, Stdio},
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{Inbox, at, inboxes};

/// How long a peer's command may take before it counts as out of reach.
const TIMEOUT: Duration = Duration::from_secs(10);

/// An agent as a listing tells it, of this machine or a peer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
  /// Its name: its inbox's, prefixed with `<peer>:` for a peer's.
  pub name: String,
  /// Awake, asleep or offline.
  pub presence: String,
  /// Its status and when it was set, if it set one.
  pub status: Option<(String, SystemTime)>,
}

/// A peer and what it listed last.
pub struct Peer {
  pub name: String,
  /// Its agents, named `<peer>:<agent>`, as it listed them last.
  pub agents: Vec<Agent>,
  /// When it listed them, if it ever did.
  pub listed: Option<SystemTime>,
  /// Whether it answered the last time it was asked.
  pub reachable: bool,
}

impl Agent {
  /// The agent of an inbox, as it is a moment ago.
  pub fn of(inbox: &Inbox) -> io::Result<Self> {
    Ok(Self {
      name: inbox.name(),
      presence: inbox.presence()?.name().to_string(),
      status: inbox.status()?.map(|status| (status.text, status.set)),
    })
  }

  /// The agent as one line of a listing for a machine: name, presence, the time its status was
  /// set in seconds since the epoch, and the status, separated by tabs, the last two empty
  /// without a status. A status is one line, so it holds no tab.
  pub fn porcelain(&self) -> String {
    let (status, set) = match &self.status {
      Some((text, set)) => (
        text.as_str(),
        set
          .duration_since(UNIX_EPOCH)
          .map(|d| d.as_secs().to_string())
          .unwrap_or_default(),
      ),
      None => ("", String::new()),
    };
    format!("{}\t{}\t{set}\t{status}", self.name, self.presence)
  }

  /// The agent from a line of [`Agent::porcelain`].
  pub fn from_porcelain(line: &str) -> Option<Self> {
    let mut fields = line.splitn(4, '\t');
    let name = fields.next()?.to_string();
    let presence = fields.next()?.to_string();
    let set = fields.next()?;
    let text = fields.next().unwrap_or("").trim_end();
    if name.is_empty() {
      return None;
    }
    let status = (!text.is_empty()).then(|| {
      let set = set
        .parse()
        .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap_or(UNIX_EPOCH);
      (text.to_string(), set)
    });
    Some(Self {
      name,
      presence,
      status,
    })
  }
}

/// This machine's agents, one per inbox in the directory, such as `~/.agents/inbox/`.
pub fn here(dir: &Path) -> io::Result<Vec<Agent>> {
  inboxes(dir)?.iter().map(Agent::of).collect()
}

/// The user's config directory.
pub fn config_dir() -> Option<PathBuf> {
  env::var_os("XDG_CONFIG_HOME")
    .map(PathBuf::from)
    .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

fn cache_dir() -> Option<PathBuf> {
  env::var_os("XDG_CACHE_HOME")
    .map(PathBuf::from)
    .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    .map(|cache| cache.join("agentinbox/peers"))
}

/// The peers, each with its agents: asked anew where it was last asked longer than `max_age`
/// ago, else as it listed them last. `Duration::ZERO` asks every peer now.
pub fn peers(max_age: Duration) -> io::Result<Vec<Peer>> {
  let Some(path) = config_dir().map(|config| config.join("agentinbox/peers")) else {
    return Ok(Vec::new());
  };
  let list = match fs::read_to_string(&path) {
    Ok(list) => list,
    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(e) => return Err(at(e, &path)),
  };
  let Some(cache) = cache_dir() else {
    return Ok(Vec::new());
  };
  let mut peers = Vec::new();
  for line in list
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty() && !line.starts_with('#'))
  {
    let Some((name, command)) = line.split_once(char::is_whitespace) else {
      eprintln!("agentinbox: the peer `{line}` has no command");
      continue;
    };
    peers.push(peer(&cache, name, command.trim(), max_age)?);
  }
  Ok(peers)
}

fn peer(cache: &Path, name: &str, command: &str, max_age: Duration) -> io::Result<Peer> {
  fs::DirBuilder::new()
    .recursive(true)
    .mode(0o700)
    .create(cache)
    .map_err(|e| at(e, cache))?;
  let listing = cache.join(name);
  let unreachable = cache.join(format!("{name}.unreachable"));
  let modified = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok();
  let asked = modified(&listing).max(modified(&unreachable));
  let due = asked.is_none_or(|asked| asked.elapsed().unwrap_or_default() >= max_age);
  if due {
    // Of the looks at once, such as parallel tool calls, one asks, and the rest take what it
    // listed last.
    let lock_path = cache.join(format!("{name}.lock"));
    let lock = fs::OpenOptions::new()
      .write(true)
      .create(true)
      .truncate(false)
      .mode(0o600)
      .open(&lock_path)
      .map_err(|e| at(e, &lock_path))?;
    match lock.try_lock() {
      Ok(()) => match list(command) {
        Some(text) => {
          let tmp = cache.join(format!(".{name}.{}", std::process::id()));
          write(&tmp, text.as_bytes())?;
          fs::rename(&tmp, &listing).map_err(|e| at(e, &listing))?;
          let _ = fs::remove_file(&unreachable);
        }
        None => {
          write(&unreachable, b"")?;
        }
      },
      Err(TryLockError::WouldBlock) => {}
      Err(TryLockError::Error(e)) => return Err(at(e, &lock_path)),
    }
  }
  let agents = fs::read_to_string(&listing)
    .unwrap_or_default()
    .lines()
    .filter_map(Agent::from_porcelain)
    .map(|agent| Agent {
      name: format!("{name}:{}", agent.name),
      ..agent
    })
    .collect();
  Ok(Peer {
    name: name.to_string(),
    agents,
    listed: modified(&listing),
    reachable: !unreachable.exists(),
  })
}

/// Writes a file of the cache, its owner's alone.
fn write(path: &Path, content: &[u8]) -> io::Result<()> {
  fs::OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(true)
    .mode(0o600)
    .open(path)
    .and_then(|mut file| io::Write::write_all(&mut file, content))
    .map_err(|e| at(e, path))
}

/// What the peer's command prints, if it succeeds within [`TIMEOUT`].
fn list(command: &str) -> Option<String> {
  let mut child = Command::new("sh")
    .args(["-c", command])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .ok()?;
  let mut stdout = child.stdout.take()?;
  let reader = thread::spawn(move || {
    let mut text = String::new();
    stdout.read_to_string(&mut text).map(|_| text)
  });
  let start = SystemTime::now();
  loop {
    match child.try_wait() {
      Ok(Some(status)) => {
        let text = reader.join().ok()?.ok()?;
        return status.success().then_some(text);
      }
      Ok(None) if start.elapsed().unwrap_or_default() < TIMEOUT => {
        thread::sleep(Duration::from_millis(20));
      }
      _ => {
        let _ = child.kill();
        let _ = child.wait();
        return None;
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::Presence;

  #[test]
  fn porcelain_round_trips() {
    let set = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    for agent in [
      Agent {
        name: "lead".into(),
        presence: Presence::Asleep.name().into(),
        status: Some(("writing the digest, in ~/dev".into(), set)),
      },
      Agent {
        name: "pi-1".into(),
        presence: Presence::Offline.name().into(),
        status: None,
      },
    ] {
      assert_eq!(Agent::from_porcelain(&agent.porcelain()), Some(agent));
    }
  }
}
