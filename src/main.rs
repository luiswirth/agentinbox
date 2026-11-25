//! `mindio`: an agent's IO on its machine, from the command line.

use std::{
  env, fs,
  io::{self, Write},
  path::{Path, PathBuf},
  process::{Command as Process, ExitCode, Stdio},
  time::Duration,
};

use jiff::{Span, Timestamp, Zoned, civil::DateTime, tz::TimeZone};
use mindio::{Alarm, Kind, MindIo, extension, peers};

mod waker;
use clap::{Parser, Subcommand};

/// MindIO: the boundary between an agent's mind and its world: input, output and waking.
///
/// An agent's IO is a directory on its machine, made at first use. Its input is `inbox/`, what
/// reaches the agent from outside, which anything on the machine may deliver into; the mind takes
/// it in at its next step. Its output is `outbox/`, what the agent says to someone outside, which
/// a subscriber carries. Each holds `new/`, the items not yet taken, `cur/`, the taken ones, and
/// `tmp/`, the ones being written; an item is one file, `<time>.<name>.<ext>`, with the time in
/// UTC. A wake, the file `wake`, starts a turn of an idle agent; what wakes it, the agent decides
/// itself, by its waking rules (see `deliver`).
///
/// A mind in a turn holds a lock on `awake`, and a mind that waits and wakes itself one on
/// `asleep`; with neither held the agent is offline, and the machine's waker answers its wake by
/// running its `resume`. Anything can deliver without this command, by writing a file into
/// `inbox/tmp/` and linking it into `inbox/new/`, and wake with `touch wake`.
#[derive(Parser)]
#[command(version)]
struct Cli {
  /// The agent's IO, by convention `~/.agents/mindio/<agent>/`.
  #[arg(long, global = true, env = "MINDIO")]
  mindio: Option<PathBuf>,
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand)]
enum Command {
  /// Delivers one item, read from stdin, into the inbox, and wakes the agent where its waking
  /// rules say so.
  ///
  /// The agent decides what wakes it, by the file `waking`: each line is a rule, fields to match
  /// and then `wake` or `quiet`, such as `from=lead quiet`, or `quiet` alone for everything. An
  /// item's fields are `from`, `kind` and the `--field`s its provider gives. The rules come after
  /// the defaults, `kind=event wake` and `kind=reading quiet`, and the last rule that matches
  /// decides.
  Deliver {
    /// Who sends it, such as an agent's or a person's name.
    #[arg(long, default_value = "unknown")]
    from: String,
    /// Its type, as a MIME type (`image/png`) or an extension (`png`).
    #[arg(long = "type", default_value = "txt")]
    ty: String,
    /// What it is: an `event`, something that happened, or a `reading`, a state delivered as the
    /// agent looks; `take` runs each reading with `MINDIO_KIND=reading`.
    #[arg(long, env = "MINDIO_KIND", default_value = "event", value_parser = ["event", "reading"])]
    kind: String,
    /// A fact about it, as `<name>=<value>`, which waking rules can match, such as
    /// `session=hive`.
    #[arg(long = "field", value_parser = field)]
    fields: Vec<(String, String)>,
  },
  /// Wakes the agent, delivering nothing.
  Wake,
  /// Takes the readings, then prints every new inbox item, oldest first, and moves it to `cur/`.
  ///
  /// A reading is a command that delivers a state as the agent looks, such as the time; the
  /// readings are the lines of `mindio/readings` in the user's config directory, each run with
  /// `sh -c` and `MINDIO`, and one that fails is reported on stderr and passed over. Prints
  /// nothing where nothing is new. A harness runs it at each step and gives its output to the
  /// model: each item as `<inbox-item from=".." time="..">` with its text, or, where it is no
  /// text, its path and type.
  Take,
  /// Blocks until the agent is woken, then answers the wake.
  ///
  /// It returns at once where the agent was woken while no one waited. Run it as a background
  /// command, or from a watcher that starts the agent. It takes nothing in: run `take` after it.
  Wait,
  /// Holds the asleep lock until stdin closes, or until the process `--pid` ends: a mind of the
  /// agent waits meanwhile and will wake itself, so the waker leaves its wakes to it.
  ///
  /// An interactive harness holds it for as long as its session is open, such as pi's extension
  /// with a pipe, or Claude Code's start hook with `--pid`, which detaches it.
  Asleep {
    /// The process whose end releases the lock, such as the harness's.
    #[arg(long)]
    pid: Option<i32>,
  },
  /// Holds the awake lock until stdin closes: a mind of the agent is in a turn meanwhile.
  ///
  /// A harness starts it with a pipe at a turn's start and closes the pipe at its end; if the
  /// harness dies, the pipe closes and the lock goes with it.
  Awake,
  /// The machine's waker: watches every agent's IO in a directory and answers each one's wakes.
  ///
  /// A wake is answered once the agent is offline, holding neither `awake` nor `asleep`: it runs
  /// the file `resume`, a command run with `sh -c` and `MINDIO`, while inbox items are untaken,
  /// holding the awake lock meanwhile. A wake while a mind is awake or asleep is that mind's, and
  /// only what it leaves untaken is answered; a wake with nothing new, or of an agent with no
  /// `resume`, runs nothing. It stops answering a wake after a run that failed or took nothing
  /// in. It also runs every agent's alarms, and runs until stopped.
  Waker {
    /// The directory of the agents.
    #[arg(default_value_os_t = default_dir())]
    dir: PathBuf,
  },
  /// The alarms: a command that waits, and a note, delivered once the command ends.
  #[command(subcommand)]
  Alarm(AlarmCommand),
  /// Sets the agent's status, one line on what it is doing and where, which others see; prints
  /// it where no text is given, and clears it where the text is empty.
  ///
  /// It lasts until the next. Anyone may set any agent's, with `--mindio`.
  Status {
    /// The status.
    text: Option<String>,
  },
  /// Lists the agents of this machine and of its peers: each one's presence and status.
  ///
  /// Awake is in a turn, asleep a mind that waits and wakes itself, offline neither, which the
  /// waker brings back as it is woken. A peer is another machine, a line `<name> <command>` of
  /// `mindio/peers` in the user's config directory, whose command lists its agents, such as over
  /// ssh with `mindio agents --here --porcelain`; its agents are `<peer>:<agent>`, and a peer out
  /// of reach is listed as such.
  Agents {
    /// The directory of the agents.
    #[arg(default_value_os_t = default_dir())]
    dir: PathBuf,
    /// Only this machine's agents.
    #[arg(long)]
    here: bool,
    /// One line per agent for programs: name, presence, the time its status was set in seconds
    /// since the epoch, and the status, separated by tabs.
    #[arg(long)]
    porcelain: bool,
  },
  /// Blocks until a time, such as `2026-11-22 09:00` in the system's time zone, or
  /// `2026-11-22T08:00:00Z`; at once where it has passed.
  ///
  /// It checks the clock at least every 30 seconds, so a machine that slept wakes it in time.
  Until {
    /// The time.
    time: String,
  },
  /// Prints what an agent needs to know of its IO, for its harness to give it as a session starts.
  Brief,
  /// Writes one item, read from stdin, into the outbox, for a recipient, such as `telegram:luis`.
  ///
  /// A subscriber, such as messagent for `telegram:`, carries the items for the recipients it
  /// serves; an item none serves stays in `new/`. Answering an inbox item is writing an item for
  /// the name it came from.
  Send {
    /// Whom it is for: the name an inbox item came from, to answer it.
    recipient: String,
    /// Its type, as a MIME type (`image/png`) or an extension (`png`).
    #[arg(long = "type", default_value = "md")]
    ty: String,
  },
}

#[derive(Subcommand)]
enum AlarmCommand {
  /// Sets an alarm: the machine's waker runs its command, and once the command ends, delivers the
  /// note into the inbox, from `alarm`, as an event with the fields `alarm` and `by`.
  ///
  /// The command only waits, since it runs again whenever the waker starts again; a condition to
  /// poll is a loop, such as `until check; do sleep 60; done`. It runs with `sh -c`, in the
  /// directory this is run in, with `MINDIO`. An alarm is dismissed once it went off, a repeating
  /// one only once its command fails, and a failing command is told as such. A command killed by
  /// a signal keeps its alarm, which runs again as the waker starts again. Anyone may set an alarm
  /// for any agent, with `--mindio`. A command that is no wait, such as a long build, runs in the
  /// background and delivers its own end:
  /// `setsid sh -c 'cargo build >log 2>&1; echo "ended: $?" | mindio deliver' &`.
  Set {
    /// The command that waits.
    #[arg(required_unless_present_any = ["at", "within"], conflicts_with_all = ["at", "within"])]
    command: Option<String>,
    /// Waits until a time, as `until` takes it.
    #[arg(long, conflicts_with = "within")]
    at: Option<String>,
    /// Waits for a span of time, such as `90m` or `1h 30m`, from now.
    #[arg(long = "in")]
    within: Option<String>,
    /// What the agent is told.
    #[arg(long, default_value = "")]
    note: String,
    /// Its name, by default the time it is set.
    #[arg(long)]
    name: Option<String>,
    /// Who sets it, by default the agent named by `MINDIO`.
    #[arg(long)]
    by: Option<String>,
    /// Runs the command again after each time it goes off, until dismissed or it fails.
    #[arg(long, conflicts_with_all = ["at", "within"])]
    repeat: bool,
  },
  /// Lists the alarms set.
  List,
  /// Dismisses an alarm, stopping its command.
  Dismiss {
    /// Its name.
    name: String,
  },
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  match run(cli) {
    Ok(()) => ExitCode::SUCCESS,
    Err(error) => {
      eprintln!("mindio: {error}");
      ExitCode::FAILURE
    }
  }
}

fn run(cli: Cli) -> io::Result<()> {
  let ext = |ty: &str| {
    extension(ty)
      .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("unknown type `{ty}`")))
  };
  match &cli.command {
    Command::Waker { dir } => return waker::run(dir),
    Command::Agents {
      dir,
      here,
      porcelain,
    } => return agents(dir, *here, *porcelain),
    Command::Until { time } => return until(time_from(time)?),
    _ => {}
  }
  let mindio = MindIo::new(cli.mindio.ok_or_else(|| {
    io::Error::new(
      io::ErrorKind::InvalidInput,
      "no agent: set MINDIO or pass --mindio",
    )
  })?);
  match cli.command {
    Command::Deliver {
      from,
      ty,
      kind,
      fields,
    } => {
      let kind = if kind == "reading" {
        Kind::Reading
      } else {
        Kind::Event
      };
      let fields: Vec<(&str, &str)> = fields
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
      mindio.deliver(io::stdin().lock(), &from, &ext(&ty)?, kind, &fields)?;
    }
    Command::Wake => mindio.wake()?,
    Command::Brief => print!("{}", brief(&mindio)),
    Command::Take => {
      readings(&mindio)?;
      let mut out = io::stdout().lock();
      for (i, item) in mindio.inbox().take()?.iter().enumerate() {
        if i > 0 {
          writeln!(out)?;
        }
        item.render(&mut out)?;
      }
    }
    Command::Wait => mindio.wait()?,
    Command::Awake => {
      let _lock = mindio.hold_awake()?;
      io::copy(&mut io::stdin().lock(), &mut io::sink())?;
    }
    Command::Asleep { pid } => {
      let _lock = mindio.hold_asleep()?;
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
    Command::Alarm(command) => alarm(&mindio, command)?,
    Command::Status { text: Some(text) } => mindio.set_status(&text)?,
    Command::Status { text: None } => {
      if let Some(status) = mindio.status()? {
        println!("{}", status.text);
      }
    }
    Command::Send { recipient, ty } => {
      let path = mindio
        .outbox()
        .send(io::stdin().lock(), &recipient, &ext(&ty)?)?;
      println!("{}", path.display());
    }
    Command::Waker { .. } | Command::Until { .. } | Command::Agents { .. } => {
      unreachable!("handled above")
    }
  }
  Ok(())
}

/// What an agent needs to know of its IO.
fn brief(mindio: &MindIo) -> String {
  let root = mindio.root.display();
  format!(
    "Your IO is MindIO, the directory {root}, named by MINDIO in your commands: its input is \
     `inbox/`, which is given to you as inbox items at each step; its output is `outbox/`, where \
     `mindio send <recipient>` answers an item by the name it came from; and it wakes you.\n\
     \n\
     What wakes you is yours to decide, in {root}/waking: one rule per line, fields to match and \
     then `wake` or `quiet`, such as `from=messagent quiet`, the last rule that matches deciding. \
     They come after the defaults, `kind=event wake` and `kind=reading quiet`: every event \
     wakes you, and no reading, such as the time. An item's fields are `from`, `kind` and the \
     ones its provider documents. Never being woken, or too rarely, is the worst case, so quiet \
     only what you know you don't need; a needless wake costs little, since you can end that \
     turn at once.\n\
     \n\
     `mindio --help` tells the rest, such as alarms and your status.\n"
  )
}

/// A field as `--field` takes it, `<name>=<value>`.
fn field(text: &str) -> Result<(String, String), String> {
  match text.split_once('=') {
    Some((name, value)) if !name.is_empty() && !name.contains(char::is_whitespace) => {
      Ok((name.to_string(), value.to_string()))
    }
    _ => Err(format!("`{text}` is no field: `<name>=<value>`")),
  }
}

/// A time as the user reads it, in the system's time zone, to the minute.
fn local(time: std::time::SystemTime) -> String {
  Timestamp::try_from(time)
    .unwrap_or_default()
    .to_zoned(TimeZone::system())
    .strftime("%Y-%m-%d %H:%M")
    .to_string()
}

fn invalid(what: String) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidInput, what)
}

fn alarm(mindio: &MindIo, command: AlarmCommand) -> io::Result<()> {
  match command {
    AlarmCommand::Set {
      command,
      at,
      within,
      note,
      name,
      by,
      repeat,
    } => {
      let command = match (command, at, within) {
        (Some(command), ..) => command,
        (_, Some(at), _) => until_command(time_from(&at)?),
        (_, _, Some(within)) => {
          let span: Span = within
            .parse()
            .map_err(|e| invalid(format!("`{within}` is no span of time: {e}")))?;
          let time = Zoned::now()
            .checked_add(span)
            .map_err(|e| invalid(format!("`{within}` from now: {e}")))?;
          until_command(time.timestamp())
        }
        _ => unreachable!("clap requires one"),
      };
      let by = by.unwrap_or_else(|| {
        env::var_os("MINDIO")
          .and_then(|own| PathBuf::from(own).file_name().map(|n| n.to_owned()))
          .map(|n| n.to_string_lossy().into_owned())
          .unwrap_or_else(|| "unknown".to_string())
      });
      let name =
        name.unwrap_or_else(|| Timestamp::now().strftime("%Y%m%dT%H%M%S.%6fZ").to_string());
      mindio.set_alarm(&Alarm {
        name: name.clone(),
        command,
        dir: env::current_dir()?,
        note,
        by,
        repeat,
      })?;
      println!("{name}");
    }
    AlarmCommand::List => {
      let mut out = io::stdout().lock();
      for alarm in mindio.alarms()? {
        let repeat = if alarm.repeat { ", repeating" } else { "" };
        writeln!(
          out,
          "{}, set by {}{repeat}: {}",
          alarm.name, alarm.by, alarm.command
        )?;
        for line in alarm.note.lines() {
          writeln!(out, "  {line}")?;
        }
      }
    }
    AlarmCommand::Dismiss { name } => {
      if !mindio.dismiss_alarm(&name)? {
        return Err(invalid(format!("there is no alarm `{name}`")));
      }
    }
  }
  Ok(())
}

/// The command that waits until the time: absolute, so that it waits for the same time when it
/// runs again after a restart.
fn until_command(time: Timestamp) -> String {
  format!("mindio until {time}")
}

/// A time, as an instant such as `2026-11-22T08:00:00Z`, or a civil time in the system's time
/// zone, such as `2026-11-22 09:00`.
fn time_from(text: &str) -> io::Result<Timestamp> {
  if let Ok(time) = text.parse::<Timestamp>() {
    return Ok(time);
  }
  text
    .parse::<DateTime>()
    .and_then(|civil| civil.to_zoned(TimeZone::system()))
    .map(|zoned| zoned.timestamp())
    .map_err(|e| invalid(format!("`{text}` is no time: {e}")))
}

/// Blocks until the time, by the clock, which a machine's sleep does not stop.
fn until(time: Timestamp) -> io::Result<()> {
  loop {
    let left = Timestamp::now().duration_until(time);
    if left.is_negative() || left.is_zero() {
      return Ok(());
    }
    let left = left.unsigned_abs().min(std::time::Duration::from_secs(30));
    std::thread::sleep(left);
  }
}

/// The agents' directory by convention, `~/.agents/mindio/`.
fn default_dir() -> PathBuf {
  env::var_os("HOME")
    .map(PathBuf::from)
    .unwrap_or_default()
    .join(".agents/mindio")
}

/// The file that lists the readings: `mindio/readings` in the user's config directory.
fn readings_file() -> Option<PathBuf> {
  peers::config_dir().map(|config| config.join("mindio/readings"))
}

/// Lists the agents of this machine, and of its peers, asked now, unless `here`.
fn agents(dir: &Path, here: bool, porcelain: bool) -> io::Result<()> {
  let mut out = io::stdout().lock();
  let line = |out: &mut io::StdoutLock, agent: &peers::Agent| -> io::Result<()> {
    if porcelain {
      return writeln!(out, "{}", agent.porcelain());
    }
    match &agent.status {
      Some((text, set)) => writeln!(
        out,
        "{} ({}, set {}): {text}",
        agent.name,
        agent.presence,
        local(*set)
      ),
      None => writeln!(out, "{} ({})", agent.name, agent.presence),
    }
  };
  for agent in peers::here(dir)? {
    line(&mut out, &agent)?;
  }
  if here {
    return Ok(());
  }
  for peer in peers::peers(Duration::ZERO)? {
    if peer.reachable {
      for agent in &peer.agents {
        line(&mut out, agent)?;
      }
    } else {
      eprintln!("mindio: the peer {} is out of reach", peer.name);
    }
  }
  Ok(())
}

/// Runs each reading, so that what it delivers is taken in the same step. A reading that fails,
/// such as for an expired login, is reported and passed over: it never stops the take.
fn readings(mindio: &MindIo) -> io::Result<()> {
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
      .env("MINDIO", &mindio.root)
      .env("MINDIO_KIND", "reading")
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .status();
    match status {
      Ok(status) if status.success() => {}
      Ok(status) => eprintln!("mindio: the reading `{reading}` failed: {status}"),
      Err(error) => eprintln!("mindio: the reading `{reading}` failed: {error}"),
    }
  }
  Ok(())
}
