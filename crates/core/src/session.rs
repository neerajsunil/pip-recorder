//! The recording session state machine shared by every frontend and backend.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionState {
    #[default]
    Idle,
    Starting,
    Recording,
    Stopping,
}

#[derive(Default, Debug)]
pub struct Session {
    state: SessionState,
}

impl Session {
    pub fn state(&self) -> SessionState {
        self.state
    }
    pub fn begin(&mut self) -> bool {
        if self.state != SessionState::Idle {
            return false;
        }
        self.state = SessionState::Starting;
        true
    }
    pub fn started(&mut self) {
        if self.state == SessionState::Starting {
            self.state = SessionState::Recording;
        }
    }
    pub fn stop(&mut self) -> bool {
        if !matches!(self.state, SessionState::Starting | SessionState::Recording) {
            return false;
        }
        self.state = SessionState::Stopping;
        true
    }
    pub fn finished(&mut self) {
        self.state = SessionState::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_during_startup_cannot_return_to_recording() {
        let mut session = Session::default();
        assert!(session.begin());
        assert!(!session.begin());
        assert!(session.stop());
        session.started();
        assert_eq!(session.state(), SessionState::Stopping);
        assert!(!session.stop());
        session.finished();
        assert!(session.begin());
    }
}
