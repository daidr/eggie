pub use eggie_completion::{CompletionItem, PromptInput};
mod events;
pub use events::*;
mod messages;
pub use messages::*;
mod input;
pub use input::*;
mod session;
pub use session::*;
mod cells;
pub use cells::*;
mod snapshot;
use eggie_domain::{ProjectId, SessionId};
use serde::{Deserialize, Serialize};
pub use snapshot::*;
use std::path::PathBuf;
use std::sync::Arc;

pub const PROTOCOL_VERSION: u32 = 7;

mod ports;
pub use ports::*;

mod paste;
pub use paste::*;

/// Fixed-point scale used by [`TerminalScrollDelta`]. Keeping scroll deltas integral preserves
/// sub-pixel trackpad motion without introducing non-reflexive floating-point values into the
/// wire protocol.
pub const TERMINAL_SCROLL_DELTA_SCALE: i32 = 1024;

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::de::{SeqAccess, Visitor};
    use serde::{Deserializer, Serialize, Serializer};
    use std::fmt;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&STANDARD.encode(bytes))
        } else {
            serde_bytes::Bytes::new(bytes).serialize(serializer)
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BytesVisitor;

        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("base64 text or a binary byte string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                STANDARD.decode(value).map_err(E::custom)
            }

            fn visit_borrowed_bytes<E>(self, value: &'de [u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(value.to_vec())
            }

            fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(value)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut bytes = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
                while let Some(byte) = sequence.next_element()? {
                    bytes.push(byte);
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_any(BytesVisitor)
    }
}

pub fn encode_line<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    Ok(encoded)
}

#[cfg(test)]
mod tests;

fn is_false(value: &bool) -> bool {
    !*value
}
