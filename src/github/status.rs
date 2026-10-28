use crate::github::model::IssueState;
use crate::task::Status;

pub const WONTFIX_LABEL: &str = "wontfix";

/// Map GitHub open/closed state and labels to an Orbit Status.
pub fn status_from_issue(state: IssueState, labels: &[String]) -> Status {
    match state {
        IssueState::Open => Status::Todo,
        IssueState::Closed => {
            if labels.iter().any(|label| label == WONTFIX_LABEL) {
                Status::Cancelled
            } else {
                Status::Done
            }
        }
    }
}

/// Map Orbit Status to GitHub issue state. Non-terminal todo/in_progress stay open.
pub fn issue_state_from_status(status: Status) -> IssueState {
    match status {
        Status::Todo | Status::InProgress => IssueState::Open,
        Status::Done | Status::Cancelled => IssueState::Closed,
    }
}

/// When pushing status to GitHub, preserve Orbit in_progress as open without forcing todo.
pub fn preserves_in_progress(status: Status) -> bool {
    matches!(status, Status::InProgress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_closed_issues_to_done_or_cancelled() {
        assert_eq!(
            status_from_issue(IssueState::Closed, &[]),
            Status::Done
        );
        assert_eq!(
            status_from_issue(IssueState::Closed, &["wontfix".to_string()]),
            Status::Cancelled
        );
    }

    #[test]
    fn open_issues_do_not_force_orbit_status() {
        assert_eq!(status_from_issue(IssueState::Open, &[]), Status::Todo);
    }
}
