//! Message state machine (spec 004). Pure: no IO.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Queued,
    Dispatched,
    Sent,
    Delivered,
    Failed,
    Expired,
    Received,
}

impl State {
    pub const ALL: [State; 7] = [
        State::Queued,
        State::Dispatched,
        State::Sent,
        State::Delivered,
        State::Failed,
        State::Expired,
        State::Received,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Dispatched => "dispatched",
            State::Sent => "sent",
            State::Delivered => "delivered",
            State::Failed => "failed",
            State::Expired => "expired",
            State::Received => "received",
        }
    }

    /// Terminal states never change again.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            State::Delivered | State::Failed | State::Expired | State::Received
        )
    }

    /// Terminal for pacing (spec 006): the device is done with the message.
    pub fn releases_window(self) -> bool {
        matches!(
            self,
            State::Sent | State::Delivered | State::Failed | State::Expired
        )
    }

    /// Webhook event fired when a message enters this state, if any.
    pub fn webhook_event(self) -> Option<&'static str> {
        match self {
            State::Sent => Some("message.sent"),
            State::Delivered => Some("message.delivered"),
            State::Failed => Some("message.failed"),
            State::Expired => Some("message.expired"),
            State::Received => Some("message.received"),
            _ => None,
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for State {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        State::ALL.into_iter().find(|st| st.as_str() == s).ok_or(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Dispatch,
    StatusSent,
    StatusDelivered,
    StatusFailed,
    /// TTL elapsed. The sweeper only raises this for `dispatched` rows that were never acked.
    Ttl,
    Cancel,
    StatusTimeout,
}

impl Event {
    pub const ALL: [Event; 7] = [
        Event::Dispatch,
        Event::StatusSent,
        Event::StatusDelivered,
        Event::StatusFailed,
        Event::Ttl,
        Event::Cancel,
        Event::StatusTimeout,
    ];
}

/// The only place outbound transitions are decided. `None` means "ignore".
pub fn transition(from: State, event: Event) -> Option<State> {
    use Event as E;
    use State as S;
    match (from, event) {
        (S::Queued, E::Dispatch) => Some(S::Dispatched),
        (S::Queued, E::Ttl) => Some(S::Expired),
        (S::Queued, E::Cancel) => Some(S::Failed),
        (S::Dispatched, E::StatusSent) => Some(S::Sent),
        (S::Dispatched, E::StatusDelivered) => Some(S::Delivered),
        (S::Dispatched, E::StatusFailed) => Some(S::Failed),
        (S::Dispatched, E::Ttl) => Some(S::Expired),
        (S::Dispatched, E::StatusTimeout) => Some(S::Failed),
        (S::Dispatched, E::Cancel) => Some(S::Failed),
        (S::Sent, E::StatusDelivered) => Some(S::Delivered),
        (S::Sent, E::StatusFailed) => Some(S::Failed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOWED: &[(State, Event, State)] = &[
        (State::Queued, Event::Dispatch, State::Dispatched),
        (State::Queued, Event::Ttl, State::Expired),
        (State::Queued, Event::Cancel, State::Failed),
        (State::Dispatched, Event::StatusSent, State::Sent),
        (State::Dispatched, Event::StatusDelivered, State::Delivered),
        (State::Dispatched, Event::StatusFailed, State::Failed),
        (State::Dispatched, Event::Ttl, State::Expired),
        (State::Dispatched, Event::StatusTimeout, State::Failed),
        (State::Dispatched, Event::Cancel, State::Failed),
        (State::Sent, Event::StatusDelivered, State::Delivered),
        (State::Sent, Event::StatusFailed, State::Failed),
    ];

    #[test]
    fn ac_004_01_exhaustive_transitions() {
        let mut applied = 0;
        for from in State::ALL {
            for ev in Event::ALL {
                let expected = ALLOWED
                    .iter()
                    .find(|(f, e, _)| *f == from && *e == ev)
                    .map(|(_, _, to)| *to);
                assert_eq!(transition(from, ev), expected, "{from:?} + {ev:?}");
                if expected.is_some() {
                    applied += 1;
                }
            }
        }
        assert_eq!(applied, ALLOWED.len());
    }

    #[test]
    fn ac_004_01_terminal_states_never_move() {
        for from in [
            State::Delivered,
            State::Failed,
            State::Expired,
            State::Received,
        ] {
            assert!(from.is_terminal());
            for ev in Event::ALL {
                assert_eq!(transition(from, ev), None);
            }
        }
    }

    #[test]
    fn ac_004_03_delivered_before_sent() {
        let s = transition(State::Dispatched, Event::StatusDelivered).unwrap();
        assert_eq!(s, State::Delivered);
        assert_eq!(transition(s, Event::StatusSent), None);
    }

    #[test]
    fn ac_004_cancel_fails_queued_and_dispatched() {
        assert_eq!(
            transition(State::Queued, Event::Cancel),
            Some(State::Failed)
        );
        assert_eq!(
            transition(State::Dispatched, Event::Cancel),
            Some(State::Failed)
        );
        assert_eq!(transition(State::Sent, Event::Cancel), None);
    }

    #[test]
    fn state_round_trips_through_str() {
        for s in State::ALL {
            assert_eq!(s.as_str().parse::<State>(), Ok(s));
        }
        assert!("nope".parse::<State>().is_err());
    }
}
