//! Blocking transport for Jev requests: one POST per request, retried
//! with backoff while TypeSafe reports rate limits or overload.

use std::thread;
use std::time::Duration;

use ureq::Agent;

use super::{JevRequest, SendError};

const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const ATTEMPTS: u32 = 4;
const TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) const KEY_VAR: &str = "TYPESAFE_API_KEY";

pub(crate) fn key_from_env() -> Option<String> {
    std::env::var(KEY_VAR)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

pub(crate) fn send(key: &str, request: &JevRequest) -> Result<String, SendError> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut last_error = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(500 << attempt));
        }
        let response = agent
            .post(ENDPOINT)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .send(request.body.as_str());
        let mut response = match response {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("Jev request failed: {error}");
                continue;
            }
        };
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| SendError::Failed(format!("Jev response unreadable: {error}")))?;
        if matches!(status, 429 | 529) {
            last_error = format!("Jev is busy ({status})");
            continue;
        }
        return outcome(status, body);
    }
    Err(SendError::Failed(last_error))
}

pub(crate) fn outcome(status: u16, body: String) -> Result<String, SendError> {
    match status {
        200..=299 => Ok(body),
        400 if body.contains("max_tokens_exceeded") => Err(SendError::TooLarge),
        401 => Err(SendError::Failed(format!(
            "Jev rejected the key in {KEY_VAR}"
        ))),
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
