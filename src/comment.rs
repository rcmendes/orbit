use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{OrbitError, Result, code};

pub const COMMENT_START: &str = "<!-- orbit:comment ";
pub const COMMENT_END: &str = "<!-- /orbit:comment -->";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Comment {
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub body: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CommentMarker {
    author: String,
    created_at: DateTime<Utc>,
}

pub fn parse_comments(content: &str) -> Result<Vec<Comment>> {
    let mut comments = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = content[cursor..].find(COMMENT_START) {
        let marker_start = cursor + relative_start;
        let marker_json_start = marker_start + COMMENT_START.len();
        let Some(relative_marker_end) = content[marker_json_start..].find(" -->") else {
            return Err(corrupt("comment marker is missing its closing ` -->`"));
        };
        let marker_json_end = marker_json_start + relative_marker_end;
        let marker: CommentMarker =
            serde_json::from_str(&content[marker_json_start..marker_json_end])
                .map_err(|error| corrupt(format!("invalid comment marker: {error}")))?;

        if marker.author.trim().is_empty() {
            return Err(corrupt("comment has an empty author"));
        }

        let after_marker = marker_json_end + " -->".len();
        let body_start = if content[after_marker..].starts_with("\r\n") {
            after_marker + 2
        } else if content[after_marker..].starts_with('\n') {
            after_marker + 1
        } else {
            after_marker
        };

        let Some(relative_end) = content[body_start..].find(COMMENT_END) else {
            return Err(corrupt("comment is missing its closing marker"));
        };
        let body_end = body_start + relative_end;
        let body = content[body_start..body_end]
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if body.trim().is_empty() {
            return Err(corrupt("comment has an empty body"));
        }

        comments.push(Comment {
            author: marker.author,
            created_at: marker.created_at,
            body,
        });
        cursor = body_end + COMMENT_END.len();
    }

    Ok(comments)
}

pub fn render_comments_file(task_id: &str, comments: &[Comment]) -> String {
    let mut output = format!("# Comments: {task_id}\n");
    for comment in comments {
        output.push('\n');
        output.push_str(&render_block(comment));
        output.push('\n');
    }
    output
}

pub fn append_comment(existing: &str, task_id: &str, comment: &Comment) -> String {
    if existing.trim().is_empty() {
        return render_comments_file(task_id, std::slice::from_ref(comment));
    }

    let mut output = existing.trim_end_matches(['\r', '\n']).to_string();
    output.push_str("\n\n");
    output.push_str(&render_block(comment));
    output.push('\n');
    output
}

pub fn validate_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(OrbitError::invalid_argument(
            "comment body must not be empty",
        ));
    }
    if body.contains(COMMENT_START) || body.contains(COMMENT_END) {
        return Err(OrbitError::invalid_argument(
            "comment body must not contain Orbit comment markers",
        ));
    }
    Ok(())
}

fn render_block(comment: &Comment) -> String {
    let marker = serde_json::to_string(&CommentMarker {
        author: comment.author.clone(),
        created_at: comment.created_at,
    })
    .expect("comment marker serialization cannot fail");

    format!(
        "{COMMENT_START}{marker} -->\n{}\n{COMMENT_END}",
        comment.body.trim_end_matches(['\r', '\n'])
    )
}

fn corrupt(message: impl Into<String>) -> OrbitError {
    OrbitError::new(code::CORRUPT_COMMENT, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn comment(author: &str, body: &str) -> Comment {
        Comment {
            author: author.to_string(),
            created_at: Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap(),
            body: body.to_string(),
        }
    }

    #[test]
    fn round_trips_comments_with_markdown_bodies() {
        let comments = vec![
            comment(
                "opencode",
                "## Triage Notes\n\n- point 1\n- point 2\n\n---\n\nMore.",
            ),
            comment("rodrigo", "A second comment."),
        ];
        let rendered = render_comments_file("orb-a7b2c9", &comments);
        let parsed = parse_comments(&rendered).unwrap();
        assert_eq!(parsed, comments);
    }

    #[test]
    fn appends_without_rewriting_prior_comments() {
        let first = comment("opencode", "First.");
        let second = comment("rodrigo", "Second.");
        let file = render_comments_file("orb-a7b2c9", std::slice::from_ref(&first));
        let appended = append_comment(&file, "orb-a7b2c9", &second);
        let parsed = parse_comments(&appended).unwrap();
        assert_eq!(parsed, vec![first, second]);
    }

    #[test]
    fn rejects_malformed_comments() {
        let error = parse_comments("<!-- orbit:comment {\"author\":\"x\"} -->\nbody").unwrap_err();
        assert_eq!(error.code, code::CORRUPT_COMMENT);
    }

    #[test]
    fn rejects_empty_and_marker_bearing_bodies() {
        assert!(validate_body("  \n ").is_err());
        assert!(validate_body("contains <!-- /orbit:comment -->").is_err());
    }
}
