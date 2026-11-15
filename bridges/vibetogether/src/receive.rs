//! Vibe Together's receive, `GET /api/receive`, as far as the bridge uses it.

use std::time::Duration;

use serde::Deserialize;

/// What a receive answers.
#[derive(Deserialize, Debug, Clone)]
pub struct Answer {
  /// The items after the received position, oldest first.
  pub items: Vec<Item>,
  /// The received position stored now.
  pub received: u64,
}

/// An item of the agent's stream, numbered on its own.
#[derive(Deserialize, Debug, Clone)]
pub struct Item {
  pub n: u64,
  pub reach: Reach,
  pub session: Session,
  pub entry: Entry,
}

/// Why an item reached the agent.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Reach {
  /// The message names the agent to wake it.
  Ping,
  /// The message names the agent quietly.
  Fyi,
  /// It was said in a session the agent is in, a join or leave included.
  Session,
}

impl Reach {
  pub fn as_str(self) -> &'static str {
    match self {
      Reach::Ping => "ping",
      Reach::Fyi => "fyi",
      Reach::Session => "session",
    }
  }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Session {
  pub id: String,
  pub name: String,
}

/// An entry of a session's log.
#[derive(Deserialize, Debug, Clone)]
pub struct Entry {
  /// Its number in the session's log.
  pub n: u64,
  /// The name it was sent under, such as `luiswirth/lead`.
  pub from: String,
  /// The payload's MIME type.
  #[serde(rename = "type")]
  pub ty: String,
  pub size: u64,
  pub filename: Option<String>,
  /// The payload, where it is short text; anything else is fetched.
  pub text: Option<String>,
}

/// What the agent's own side holds while its receive waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
  Awake,
  Asleep,
}

impl State {
  pub fn as_str(self) -> &'static str {
    match self {
      State::Awake => "awake",
      State::Asleep => "asleep",
    }
  }
}

/// Why a receive gave no answer.
#[derive(Debug)]
pub enum Failure {
  /// The network or the server failed, or a newer receive of the agent ended this one (409): try again.
  Retry(String),
  /// The server refused the request, such as for a bad token or an unknown agent: trying again will not help.
  Refused(String),
}

#[derive(Clone)]
pub struct Client {
  http: ureq::Agent,
  server: String,
  token: String,
  agent: String,
}

impl Client {
  /// A client that receives as `agent`, with requests waiting at most `wait` before they count as failed.
  pub fn new(server: &str, token: String, agent: String, wait: Duration) -> Self {
    let config = ureq::Agent::config_builder()
      .http_status_as_error(false)
      .timeout_global(Some(wait + Duration::from_secs(60)))
      .build();
    Self {
      http: config.into(),
      server: server.trim_end_matches('/').to_string(),
      token,
      agent,
    }
  }

  pub fn server(&self) -> &str {
    &self.server
  }

  /// Receives what comes after `received`, or after the stored received position if it is `None`, and stores `read`
  /// where it is given. It returns as soon as there is an item, or after `wait` seconds.
  pub fn receive(
    &self,
    received: Option<u64>,
    read: Option<u64>,
    state: State,
    wait: u64,
  ) -> Result<Answer, Failure> {
    let mut request = self
      .http
      .get(format!("{}/api/receive", self.server))
      .header("Authorization", format!("Bearer {}", self.token))
      .query("agent", &self.agent)
      .query("state", state.as_str())
      .query("wait", wait.to_string());
    for (name, position) in [("received", received), ("read", read)] {
      if let Some(position) = position {
        request = request.query(name, position.to_string());
      }
    }
    let mut response = request.call().map_err(|e| Failure::Retry(e.to_string()))?;
    let status = response.status().as_u16();
    if status == 200 {
      return response
        .body_mut()
        .read_json()
        .map_err(|e| Failure::Retry(format!("unreadable answer: {e}")));
    }
    let body = response.body_mut().read_to_string().unwrap_or_default();
    let reason = format!("{status} {}", body.trim());
    Err(match status {
      408 | 409 | 429 | 500.. => Failure::Retry(reason),
      _ => Failure::Refused(reason),
    })
  }
}
