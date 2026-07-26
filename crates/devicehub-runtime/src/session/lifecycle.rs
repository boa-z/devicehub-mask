use std::time::Duration;

use devicehub_core::ConnKind;

use crate::WIFI_REAUTHORIZE_REQUIRED;

const WIFI_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(8);
const WIFI_STABLE_SESSION: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionRetry {
    pub(crate) attempt: u32,
    pub(crate) delay: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionFailureAction {
    Stop,
    Retry(SessionRetry),
}

/// Tracks retry state for the parent device session. Child service recovery is
/// handled separately by [`crate::ServiceSupervisor`].
#[derive(Debug, Default)]
pub(crate) struct SessionRetryPolicy {
    wifi_attempt: u32,
}

impl SessionRetryPolicy {
    pub(crate) fn after_failure(
        &mut self,
        connection: ConnKind,
        error_message: &str,
        session_runtime: Duration,
    ) -> SessionFailureAction {
        if connection != ConnKind::Network || error_message == WIFI_REAUTHORIZE_REQUIRED {
            self.reset();
            return SessionFailureAction::Stop;
        }

        if session_runtime >= WIFI_STABLE_SESSION {
            self.reset();
        }
        let delay = wifi_reconnect_delay(self.wifi_attempt);
        self.wifi_attempt = self.wifi_attempt.saturating_add(1);
        SessionFailureAction::Retry(SessionRetry {
            attempt: self.wifi_attempt,
            delay,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.wifi_attempt = 0;
    }
}

fn wifi_reconnect_delay(attempt: u32) -> Duration {
    Duration::from_secs(1_u64 << attempt.min(3)).min(WIFI_RECONNECT_MAX_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_retryable_network_failures_rebuild_the_parent_tunnel() {
        let mut policy = SessionRetryPolicy::default();
        assert_eq!(
            policy.after_failure(ConnKind::Usb, "transport failure", Duration::ZERO),
            SessionFailureAction::Stop
        );
        assert_eq!(
            policy.after_failure(ConnKind::Network, WIFI_REAUTHORIZE_REQUIRED, Duration::ZERO,),
            SessionFailureAction::Stop
        );
        assert_eq!(
            policy.after_failure(ConnKind::Network, "early eof", Duration::ZERO),
            SessionFailureAction::Retry(SessionRetry {
                attempt: 1,
                delay: Duration::from_secs(1),
            })
        );
    }

    #[test]
    fn repeated_failures_back_off_and_a_stable_session_resets_the_attempt() {
        let mut policy = SessionRetryPolicy::default();
        let expected = [1, 2, 4, 8, 8];
        for (index, seconds) in expected.into_iter().enumerate() {
            assert_eq!(
                policy.after_failure(ConnKind::Network, "closed", Duration::ZERO),
                SessionFailureAction::Retry(SessionRetry {
                    attempt: index as u32 + 1,
                    delay: Duration::from_secs(seconds),
                })
            );
        }
        assert_eq!(
            policy.after_failure(ConnKind::Network, "closed", WIFI_STABLE_SESSION),
            SessionFailureAction::Retry(SessionRetry {
                attempt: 1,
                delay: Duration::from_secs(1),
            })
        );
    }
}
