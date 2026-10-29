//! Grok subscription voice. Mints a realtime credential from the local Grok Bot
//! session and keeps microphone and speaker audio on this machine.

mod audio;
mod auth;
mod protocol;
mod session;

pub use audio::{DesktopDuplex, Duplex};
pub use auth::{signed_in, Credentials, GrokBotCredentials, Minted};
pub use session::{run, Call, ToolExec, ToolSpec, TranscriptSink};

#[cfg(test)]
mod tests;
