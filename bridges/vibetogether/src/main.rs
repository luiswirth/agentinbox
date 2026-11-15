mod bridge;
mod receive;

use std::{env, fs, path::PathBuf, process::ExitCode};

use agentinbox::Inbox;
use clap::Parser;

use crate::{bridge::Bridge, receive::Client};

/// A bridge from Vibe Together to an agent's inbox.
///
/// It keeps one receive of the agent open always, so the agent is present: `awake` while the inbox's awake lock is held,
/// `asleep` otherwise. Each item that reaches the agent is delivered into the inbox as text whose first line says where
/// it is from; a ping or a session's message also rings the bell, an fyi stays quiet. A payload without inline text is
/// described with where to fetch it. Each receive tells Vibe Together the last item delivered, as received, and the
/// last that, with every item before it, has left `new/`, that is, the agent took it in, as read.
///
/// The token is VIBETOGETHER_TOKEN, else the file ~/.config/vibetogether/token.
///
/// It runs until the server refuses it, such as for a bad token. Exit codes: 1 error, 2 usage.
#[derive(Parser)]
#[command(version, about, long_about)]
struct Cli {
  /// The agent's inbox, by convention `~/.agents/inbox/<agent>/`.
  #[arg(long, env = "AGENT_INBOX")]
  inbox: PathBuf,
  /// The agent's Vibe Together id, as `a_…`.
  #[arg(long, env = "VIBETOGETHER_AGENT")]
  agent: String,
  #[arg(
    long,
    env = "VIBETOGETHER_URL",
    default_value = "https://vibetogether.potentialhedge.com"
  )]
  server: String,
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let token = env::var("VIBETOGETHER_TOKEN").ok().or_else(|| {
    let config = env::var_os("XDG_CONFIG_HOME")
      .map(PathBuf::from)
      .or_else(|| env::home_dir().map(|home| home.join(".config")))?;
    fs::read_to_string(config.join("vibetogether/token")).ok()
  });
  let Some(token) = token.map(|token| token.trim().to_string()) else {
    eprintln!(
      "agentinbox-vibetogether: no token: set VIBETOGETHER_TOKEN or write ~/.config/vibetogether/token"
    );
    return ExitCode::FAILURE;
  };
  let bridge = Bridge {
    inbox: Inbox::new(cli.inbox),
    client: Client::new(&cli.server, token, cli.agent, bridge::WAIT),
  };
  let Err(e) = bridge.run();
  eprintln!("agentinbox-vibetogether: {e}");
  ExitCode::FAILURE
}
