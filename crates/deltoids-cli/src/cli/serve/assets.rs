//! Static web-app assets, embedded into the binary at compile time.
//!
//! The app has no build step: three hand-written files served verbatim by
//! [`super::router`]. Keeping them embedded means `deltoids serve` ships as
//! one self-contained binary with no runtime file dependencies.

pub const INDEX_HTML: &str = include_str!("assets/index.html");
pub const APP_JS: &str = include_str!("assets/app.js");
pub const STYLE_CSS: &str = include_str!("assets/style.css");
