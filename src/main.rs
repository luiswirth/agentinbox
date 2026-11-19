//! `agentinbox`: the agent's inbox and outbox on its machine, from the command line.

use std::{
  env, fs,
  io::{self, Write},
  path::PathBuf,
  process::{Command as Process, ExitCode, Stdio},
};

use agentinbox::{Inbox, Outbox, extension};

mod waker;
use clap::{Parser, Subcommand};

/// The agent's inbox: its perception of everything outside it.
///
/// A directory, made at first use, that anything on the machine may deliver into. `new/` holds
/// the items not yet taken in, `cur/` the taken ones, `tmp/` the ones being written; the file
/// `wake` is the doorbell, and the agent is awake while a lock on the file `awake` is held. Each
/// item is one file, named `<time>.<sender>.<ext>`, with the time in UTC. Delivering is quiet:
/// the agent sees the item at its next step; ringing the bell also wakes an agent that is idle.
/// A mind that waits and wakes itself holds a lock on `asleep`; with neither lock held, the
/// agent is offline, and the machine's waker answers a ring by running the inbox's `resume`.
/// Anything can deliver without this command, by writing a file into `tmp/` and renaming it
/// into `new/`, and ring with `touch wake`.
#[derive(Parser)]
#[command(version)]
struct Cli {
  /// The agent's inbox, by convention `~/.agents/inbox/<agent>/`.
  #[arg(long, global = true, env = "AGENT_INBOX")]
  inbox: Option<PathBuf>,
  /// The agent's outbox, by convention `~/.agents/outbox/<agent>/`.
  #[arg(long, global = true, env = "AGENT_OUTBOX")]
  outbox: Option<PathBuf>,
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Delivers one item, read from stdin, into `new/`.
  ///
  /// The inbox's file `do-not-disturb` decides whether it rings, by `from`: each line is a rule,
  /// fields to match and then `ring` or `quiet`, such as `from=lead quiet`, or `quiet` alone for
  /// everything. The last rule that matches decides; where none does, the sender's proposal
  /// stands.
  Deliver {
    /// Who sends it, such as an agent's or a person's name.
    #[arg(long, default_value = "unknown")]
    from: String,
    /// Its type, as a MIME type (`image/png`) or an extension (`png`).
    #[arg(long = "type", default_value = "txt")]
    ty: String,
    /// Proposes to ring the bell, to wake the agent.
    #[arg(long)]
    wake: bool,
  },
  /// Rings the bell: wakes the agent without delivering anything.
  Ring,
  /// Takes the readings, then prints every new item, oldest first, and moves it to `cur/`.
  ///
  /// A reading is a command that delivers a state as the agent looks, such as the time; the
  /// readings are the lines of `agentinbox/readings` in the user's config directory, each run
  /// with `sh -c` and `AGENT_INBOX`, and one that fails is reported on stderr and passed over.
  /// Prints nothing where nothing is new. A harness runs it at each step and gives its output to
  /// the model: each item as `<inbox-item from=".." time="..">` with its text, or, where it is
  /// no text, its path and type.
  Take,
  /// Blocks until the bell rings, then silences it.
  ///
  /// It returns at once where the bell rang while no one waited. It wakes the agent: run it as a
  /// background command, or from a watcher that starts the agent. It takes nothing in: run
  /// `take` after it.
  Wait,
  /// Holds the asleep lock until stdin closes, or until the process `--pid` ends: a mind of
  /// the agent waits meanwhile and will wake itself, so the waker leaves its rings to it.
  ///
  /// An interactive harness holds it for as long as its session is open, such as pi's
  /// extension with a pipe, or Claude Code's start hook with `--pid`, which detaches it.
  Asleep {
    /// The process whose end releases the lock, such as the harness's.
    #[arg(long)]
    pid: Option<i32>,
  },
  /// The machine's waker: watches every inbox in a directory and answers each one's bell.
  ///
  /// A ring is answered once the agent is offline, holding neither `awake` nor `asleep`: it runs
  /// the inbox's file `resume`, a command run with `sh -c` and `AGENT_INBOX`, while items are
  /// untaken, holding the awake lock meanwhile. A ring while a mind is awake or asleep is that
  /// mind's, and only what it leaves untaken is answered; a ring with nothing new, or in an
  /// inbox with no `resume`, runs nothing. It stops answering a ring after a run that failed or
  /// took nothing in. It also keeps each line of an inbox's file `providers` running, each with
  /// `sh -c` and `AGENT_INBOX`, again after a pause where one ends, until the line or the inbox
  /// is gone. It runs until stopped.
  Waker {
    /// The directory of the inboxes.
    #[arg(default_value_os_t = default_inboxes())]
    dir: PathBuf,
  },
  /// Holds the awake lock until stdin closes: the agent is awake meanwhile.
  ///
  /// A harness starts it with a pipe at a turn's start and closes the pipe at its end; if the
  /// harness dies, the pipe closes and the lock goes with it.
  Awake,
  /// Writes one item, read from stdin, into the outbox, for a recipient, such as
  /// `telegram:luis`.
  ///
  /// The outbox is the inbox's dual: what the agent says to someone outside. A subscriber, such
  /// as messagent for `telegram:`, carries the items for the recipients it serves; an item none
  /// serves stays in `new/`. Answering an inbox item is writing an item for the name it came
  /// from.
  Send {
    /// Whom it is for: the name an inbox item came from, to answer it.
    recipient: String,
    /// Its type, as a MIME type (`image/png`) or an extension (`png`).
    #[arg(long = "type", default_value = "md")]
    ty: String,
  },
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  match run(cli) {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("agentinbox: {error}");
      ExitCode::FAILURE
    }
  }
}

fn run(cli: Cli) -> io::Result<()> {
  let missing = |what: &str, var: &str| {
    io::Error::new(
      io::ErrorKind::InvalidInput,
      format!("no {what}: set {var} or pass --{what}"),
    )
  };
  let ext = |ty: &str| {
    extension(ty)
      .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("unknown type `{ty}`")))
  };
  if let Command::Waker { dir } = &cli.command {
    return waker::run(dir);
  }
  if let Command::Send { recipient, ty } = &cli.command {
    let root = cli
      .outbox
      .ok_or_else(|| missing("outbox", "AGENT_OUTBOX"))?;
    let path = Outbox::new(root).send(io::stdin().lock(), recipient, &ext(ty)?)?;
    println!("{}", path.display());
    return Ok(());
  }
  let inbox = Inbox::new(cli.inbox.ok_or_else(|| missing("inbox", "AGENT_INBOX"))?);
  match cli.command {
    Command::Deliver { from, ty, wake } => {
      inbox.deliver(io::stdin().lock(), &from, &ext(&ty)?)?;
      if inbox.rings(&[("from", &from)], wake)? {
        inbox.ring()?;
      }
    }
    Command::Ring => inbox.ring()?,
    Command::Take => {
      readings(&inbox)?;
      let mut out = io::stdout().lock();
      for (i, item) in inbox.take()?.iter().enumerate() {
        if i > 0 {
          writeln!(out)?;
        }
        item.render(&mut out)?;
      }
    }
    Command::Wait => inbox.wait()?,
    Command::Awake => {
      let _lock = inbox.hold_awake()?;
      io::copy(&mut io::stdin().lock(), &mut io::sink())?;
    }
    Command::Asleep { pid } => {
      let _lock = inbox.hold_asleep()?;
      match pid {
        Some(pid) => {
          waker::detach();
          waker::until_ended(pid)?;
        }
        None => {
          io::copy(&mut io::stdin().lock(), &mut io::sink())?;
        }
      }
    }
    Command::Send { .. } | Command::Waker { .. } => unreachable!("handled above"),
  }
  Ok(())
}

/// The inboxes' directory by convention, `~/.agents/inbox/`.
fn default_inboxes() -> PathBuf {
  env::var_os("HOME")
    .map(PathBuf::from)
    .unwrap_or_default()
    .join(".agents/inbox")
}

/// The file that lists the readings: `agentinbox/readings` in the user's config directory.
fn readings_file() -> Option<PathBuf> {
  env::var_os("XDG_CONFIG_HOME")
    .map(PathBuf::from)
    .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    .map(|config| config.join("agentinbox/readings"))
}

/// Runs each reading, so that what it delivers is taken in the same step. A reading that fails,
/// such as for an expired login, is reported and passed over: it never stops the take.
fn readings(inbox: &Inbox) -> io::Result<()> {
  let Some(path) = readings_file() else {
    return Ok(());
  };
  let list = match fs::read_to_string(&path) {
    Ok(list) => list,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
    Err(error) => {
      return Err(io::Error::new(
        error.kind(),
        format!("{}: {error}", path.display()),
      ));
    }
  };
  for reading in list
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty() && !line.starts_with('#'))
  {
    let status = Process::new("sh")
      .args(["-c", reading])
      .env("AGENT_INBOX", &inbox.root)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .status();
    match status {
      Ok(status) if status.success() => {}
      Ok(status) => eprintln!("agentinbox: the reading `{reading}` failed: {status}"),
      Err(error) => eprintln!("agentinbox: the reading `{reading}` failed: {error}"),
    }
  }
  Ok(())
}
