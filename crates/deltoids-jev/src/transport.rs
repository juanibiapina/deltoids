//! Sending one Jev request: the POST itself is the caller's, as
//! [`Post`] in and [`Reply`] out; this retries with backoff while
//! TypeSafe reports rate limits or overload and says what a reply means.

use std::thread;
use std::time::Duration;

use super::{JevRequest, SendError};

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const ATTEMPTS: u32 = 4;
#[cfg(not(test))]
const BACKOFF: Duration = Duration::from_millis(500);
#[cfg(test)]
const BACKOFF: Duration = Duration::from_millis(1);

/// An HTTP POST of a JSON `body` to `url`, with `authorization` as the
/// `Authorization` header. A transport sets `Content-Type:
/// application/json` and a timeout of its choice; 60 seconds suits Jev.
#[derive(Debug, Clone, Copy)]
pub struct Post<'a> {
    pub url: &'a str,
    pub authorization: &'a str,
    pub body: &'a str,
}

/// What the server answered, whatever its status. A transport returns
/// `Err` only when there is no answer, as on a network failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

pub(crate) fn send(
    key: &str,
    request: &JevRequest,
    post: &(dyn Fn(Post<'_>) -> Result<Reply, String> + Sync),
) -> Result<String, SendError> {
    let authorization = format!("Bearer {key}");
    let mut last_error = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            thread::sleep(BACKOFF * (1 << attempt));
        }
        let reply = post(Post {
            url: ENDPOINT,
            authorization: &authorization,
            body: &request.body,
        });
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => {
                last_error = format!("Jev request failed: {error}");
                continue;
            }
        };
        if matches!(reply.status, 429 | 529) {
            last_error = format!("Jev is busy ({})", reply.status);
            continue;
        }
        return outcome(reply);
    }
    Err(SendError::Failed(last_error))
}

pub(crate) fn outcome(reply: Reply) -> Result<String, SendError> {
    let Reply { status, body } = reply;
    match status {
        200..=299 => Ok(body),
        400 if body.contains("max_tokens_exceeded") => Err(SendError::TooLarge),
        401 => Err(SendError::Failed("Jev rejected the API key".to_string())),
        _ => Err(SendError::Failed(format!(
            "Jev returned {status}: {}",
            first_line(&body)
        ))),
    }
}

fn first_line(body: &str) -> &str {
    let line = body.lines().next().unwrap_or("");
    match line.char_indices().nth(120) {
        Some((end, _)) => &line[..end],
        None => line,
    }
}
