use crate::github::model::{IssueState, IssueStateReason};
use crate::task::Status;

/// Label that projects an `in_progress` Status onto a GitHub Issue. It is
/// managed outside the user Label field and never read back as a Status.
pub const IN_PROGRESS_LABEL: &str = "orbit:in-progress";
pub const IN_PROGRESS_LABEL_COLOR: &str = "fbca04";
pub const IN_PROGRESS_LABEL_DESCRIPTION: &str = "Orbit status: in progress";
pub const WONTFIX_LABEL: &str = "wontfix";

/// Map a GitHub Issue to an Orbit Status. `state_reason` wins over the legacy
/// `wontfix` label when GitHub supplies one.
pub fn status_from_issue(
    state: IssueState,
    reason: Option<IssueStateReason>,
    labels: &[String],
) -> Status {
    match state {
        IssueState::Open => Status::Todo,
        IssueState::Closed => match reason {
            Some(IssueStateReason::NotPlanned) => Status::Cancelled,
            Some(IssueStateReason::Completed) => Status::Done,
            Some(IssueStateReason::Reopened) | None => {
                if labels.iter().any(|label| label == WONTFIX_LABEL) {
                    Status::Cancelled
                } else {
                    Status::Done
                }
            }
        },
    }
}

/// Map Orbit Status to GitHub issue state. Non-terminal todo/in_progress stay open.
pub fn issue_state_from_status(status: Status) -> IssueState {
    match status {
        Status::Todo | Status::InProgress => IssueState::Open,
        Status::Done | Status::Cancelled => IssueState::Closed,
    }
}

/// Closed terminal statuses carry a reason; open statuses do not.
pub fn issue_state_reason_from_status(status: Status) -> Option<IssueStateReason> {
    match status {
        Status::Done => Some(IssueStateReason::Completed),
        Status::Cancelled => Some(IssueStateReason::NotPlanned),
        Status::Todo | Status::InProgress => None,
    }
}

pub fn has_reserved_status_label(labels: &[String]) -> bool {
    labels.iter().any(|label| label == IN_PROGRESS_LABEL)
}

/// Strip Orbit-managed labels before comparing or applying user Labels.
pub fn user_labels(labels: &[String]) -> Vec<String> {
    labels
        .iter()
        .filter(|label| label.as_str() != IN_PROGRESS_LABEL)
        .cloned()
        .collect()
}

/// User Labels plus the reserved in-progress label when the Status requires it.
pub fn labels_for_status(labels: &[String], status: Status) -> Vec<String> {
    let mut labels = user_labels(labels);
    if status == Status::InProgress {
        labels.push(IN_PROGRESS_LABEL.to_string());
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_closed_issues_to_done_or_cancelled() {
        assert_eq!(
            status_from_issue(IssueState::Closed, None, &[]),
            Status::Done
        );
        assert_eq!(
            status_from_issue(IssueState::Closed, None, &["wontfix".to_string()]),
            Status::Cancelled
        );
        assert_eq!(
            status_from_issue(IssueState::Closed, Some(IssueStateReason::NotPlanned), &[]),
            Status::Cancelled
        );
        assert_eq!(
            status_from_issue(
                IssueState::Closed,
                Some(IssueStateReason::Completed),
                &["wontfix".to_string()]
            ),
            Status::Done
        );
    }

    #[test]
    fn open_issues_do_not_force_orbit_status() {
        assert_eq!(status_from_issue(IssueState::Open, None, &[]), Status::Todo);
        assert_eq!(
            status_from_issue(IssueState::Open, None, &[IN_PROGRESS_LABEL.to_string()]),
            Status::Todo
        );
    }

    #[test]
    fn reserved_label_tracks_in_progress_only() {
        let labels = vec!["bug".to_string(), IN_PROGRESS_LABEL.to_string()];
        assert_eq!(user_labels(&labels), vec!["bug".to_string()]);
        assert_eq!(
            labels_for_status(&labels, Status::InProgress),
            vec!["bug".to_string(), IN_PROGRESS_LABEL.to_string()]
        );
        assert_eq!(
            labels_for_status(&labels, Status::Todo),
            vec!["bug".to_string()]
        );
    }
}
