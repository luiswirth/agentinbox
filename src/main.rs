//! `agentinbox`: the agent's inbox and outbox on its machine, from the command line.

use std::{
  env, fs,
  io::{self, Write},
  path::PathBuf,
  process::{Command as Process, ExitCode, Stdio},
};

use agentinbox::{Inbox, Outbox, extension};
use clap::{Parser, Subcommand};

/// The agent's inbox: its perception of everything outside it.
///
/// A directory, made at first use, that anything on the machine may deliver into. `new/` holds
/// the items not yet taken in, `cur/` the taken ones, `tmp/` the ones being written; the file
/// `wake` is the doorbell, and the agent is awake while a lock on the file `awake` is held. Each
/// item is one file, named `<time>.<sender>.<ext>`, with the time in UTC. Delivering is quiet:
/// the agent sees the item at its next step; ringing the bell also wakes an agent that is idle.
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
  /// Answers the bell by running a command, such as a harness's one-shot run.
  ///
  /// Once no mind of the agent is awake, it runs the command while items are untaken, holding
  /// the awake lock meanwhile. A ring while a mind is awake is that mind's, and only what it
  /// leaves untaken is answered; a ring with nothing new runs nothing. It stops answering a ring
  /// after a run that failed or took nothing in. The command gets `AGENT_INBOX`. It runs until
  /// stopped.
  Wake {
    /// The command and its arguments, after `--`.
    #[arg(trailing_var_arg = true, required = true, value_name = "COMMAND")]
    command: Vec<String>,
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
    Command::Wake { command } => wake(&inbox, &command)?,
    Command::Send { .. } => unreachable!("handled above"),
  }
  Ok(())
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

/// Answers the bell: once no mind of the agent is awake, takes the ring and, while something is
/// untaken, runs `command` as the agent's turn, holding the awake lock. A ring that comes while a
/// mind is awake is that mind's: it stays for the mind's own wait, and only what is left once
/// the mind sleeps is answered. It stops answering a ring after a run that failed, or that took
/// nothing in, so that a broken command doesn't run in a loop.
fn wake(inbox: &Inbox, command: &[String]) -> io::Result<()> {
  loop {
    inbox.rung()?;
    let _turn = inbox.hold_awake()?;
    if !inbox.unring()? {
      continue;
    }
    while let Some(oldest) = inbox.oldest_untaken()? {
      let status = Process::new(&command[0])
        .args(&command[1..])
        .env("AGENT_INBOX", &inbox.root)
        .status()?;
      if !status.success() {
        eprintln!("agentinbox: the woken command failed: {status}");
        break;
      }
      if inbox.new_dir().join(&oldest).exists() {
        eprintln!("agentinbox: the woken command took nothing in");
        break;
      }
    }
  }
}
