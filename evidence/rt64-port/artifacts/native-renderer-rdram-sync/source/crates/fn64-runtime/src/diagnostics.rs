//! Process-lifetime diagnostic configuration used on runtime hot paths.
//!
//! Environment variables are a launch-time interface. Reading them for every
//! guest queue operation takes the process environment lock and allocates even
//! when diagnostics are disabled, so the parsed setting is immutable after its
//! first use.

use std::sync::OnceLock;

/// Optional diagnostics for message delivery across the ABI/runtime seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugSendDiagnostics {
    Disabled,
    Enabled { message_words: Option<usize> },
}

impl DebugSendDiagnostics {
    #[inline]
    pub const fn enabled(self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    #[inline]
    pub const fn message_words(self) -> Option<usize> {
        match self {
            Self::Disabled => None,
            Self::Enabled { message_words } => message_words,
        }
    }
}

/// Return the launch-time `FN64_DEBUG_SEND` configuration.
///
/// `FN64_DEBUG_SEND_WORDS` is parsed only when the parent diagnostic is
/// enabled, preserving the diagnostic's existing opt-in behavior.
#[inline]
pub fn debug_send_diagnostics() -> DebugSendDiagnostics {
    static CONFIG: OnceLock<DebugSendDiagnostics> = OnceLock::new();
    *CONFIG.get_or_init(|| match std::env::var("FN64_DEBUG_SEND") {
        Ok(_) => parse_debug_send_diagnostics(
            true,
            std::env::var("FN64_DEBUG_SEND_WORDS").ok().as_deref(),
        ),
        Err(_) => DebugSendDiagnostics::Disabled,
    })
}

fn parse_debug_send_diagnostics(
    enabled: bool,
    message_words: Option<&str>,
) -> DebugSendDiagnostics {
    if !enabled {
        return DebugSendDiagnostics::Disabled;
    }
    let message_words = message_words.map(|raw_count| {
        raw_count.parse::<usize>().unwrap_or_else(|_| {
            panic!("FN64_DEBUG_SEND_WORDS must be an integer, got {raw_count:?}")
        })
    });
    DebugSendDiagnostics::Enabled { message_words }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_send_diagnostics_do_not_parse_dependent_values() {
        assert_eq!(
            parse_debug_send_diagnostics(false, Some("not-an-integer")),
            DebugSendDiagnostics::Disabled
        );
    }

    #[test]
    fn enabled_send_diagnostics_preserve_the_optional_word_limit() {
        assert_eq!(
            parse_debug_send_diagnostics(true, Some("12")),
            DebugSendDiagnostics::Enabled {
                message_words: Some(12)
            }
        );
        assert_eq!(
            parse_debug_send_diagnostics(true, None),
            DebugSendDiagnostics::Enabled {
                message_words: None
            }
        );
    }

    #[test]
    #[should_panic(expected = "FN64_DEBUG_SEND_WORDS must be an integer")]
    fn enabled_send_diagnostics_reject_invalid_word_limits() {
        let _ = parse_debug_send_diagnostics(true, Some("many"));
    }
}
