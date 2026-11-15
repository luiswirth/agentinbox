use std::{
  io::{self, Write},
  path::PathBuf,
  process::ExitCode,
};

use agentinbox::{Inbox, extension};
use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};

/// An agent's inbox: anything delivers content to an agent, and the agent's harness takes it in.
///
/// The inbox is a directory, created on first use: `new/` holds the items not yet taken in, `cur/` the taken ones,
/// `tmp/` the ones being written, the file `wake` is the doorbell, and the agent is awake while a lock on the file
/// `awake` is held. Each item is one file, named `<time>.<sender>.<ext>`, with the time in UTC. Delivering is quiet: the agent sees the item at its next step.
/// Ringing the bell also wakes an agent that is idle.
///
/// Anything can also deliver without this command, by writing a file into `tmp/` and renaming it into `new/`,
/// and ring with `touch wake`.
///
/// Exit codes: 0 ok, 1 error, 2 usage.
#[derive(Parser)]
#[command(version, about, long_about)]
struct Cli {
  /// The inbox directory, by convention `~/.agents/inbox/<agent>/`.
  #[arg(long, env = "AGENT_INBOX", global = true)]
  inbox: Option<PathBuf>,

  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Deliver one item, read from stdin, into `new/`.
  Deliver {
    /// Who sends it, such as an agent's or a person's name.
    #[arg(long, default_value = "unknown")]
    from: String,
    /// Its type, as a MIME type (`image/png`) or an extension (`png`).
    #[arg(long, default_value = "txt")]
    r#type: String,
    /// Also ring the bell, to wake the agent.
    #[arg(long)]
    wake: bool,
  },
  /// Ring the bell: wake the agent without delivering anything.
  Ring,
  /// Print every new item, oldest first, and move it to `cur/`. Prints nothing if nothing is new.
  ///
  /// A harness runs this at each step and gives its output to the model.
  /// Each item is printed as `<inbox-item from=".." time="..">` with its text,
  /// or, if it is not text, such as an image, as its path and type.
  Take,
  /// Block until the bell rings, then silence it. Returns at once if it rang while no one waited.
  ///
  /// Wakes the agent: run it as a background command, or from a watcher that starts the agent.
  /// It does not take anything in; run `take` after it.
  Wait,
  /// Hold the awake lock until stdin closes: the agent is awake meanwhile, such as while a turn runs.
  ///
  /// A harness starts it with a pipe at the start of a turn and closes the pipe at its end.
  /// If the harness dies, the pipe closes and the lock goes with it. Waits while another holds the lock.
  Awake,
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let Some(root) = cli.inbox else {
    Cli::command()
      .error(
        ErrorKind::MissingRequiredArgument,
        "no inbox: set AGENT_INBOX or pass --inbox",
      )
      .exit();
  };
  let inbox = Inbox::new(root);
  match run(&inbox, cli.command) {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("agentinbox: {e}");
      ExitCode::FAILURE
    }
  }
}

fn run(inbox: &Inbox, command: Command) -> io::Result<()> {
  match command {
    Command::Deliver { from, r#type, wake } => {
      let Some(ext) = extension(&r#type) else {
        Cli::command()
          .error(
            ErrorKind::InvalidValue,
            format!("unknown type `{}`", r#type),
          )
          .exit();
      };
      inbox.deliver(io::stdin().lock(), &from, &ext)?;
      if wake {
        inbox.ring()?;
      }
    }
    Command::Ring => inbox.ring()?,
    Command::Take => {
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
  }
  Ok(())
}
