//! The TUI's way to Jev: the key from `TYPESAFE_API_KEY` and a blocking
//! ureq POST for [`deltoids_jev`].

use std::time::Duration;

use deltoids_jev::{Post, Reply};
use ureq::Agent;

const KEY_VAR: &str = "TYPESAFE_API_KEY";
const TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) fn key_from_env() -> Option<String> {
    std::env::var(KEY_VAR)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

pub(crate) fn post(post: Post<'_>) -> Result<Reply, String> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(post.url)
        .header("Authorization", post.authorization)
        .header("Content-Type", "application/json")
        .send(post.body)
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("unreadable response: {error}"))?;
    Ok(Reply { status, body })
}
