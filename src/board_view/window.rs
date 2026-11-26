//! The team default **Closed window** (`[board_view] closed_window`, ADR 0023).

pub const DEFAULT: &str = "14d";

/// Normalizes a `<N>h|d|w` or `all` value; `None` when malformed or zero.
pub fn parse(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw == "all" {
        return Some(raw.to_string());
    }
    let unit = raw.chars().last().filter(|c| matches!(c, 'h' | 'd' | 'w'))?;
    let n: u32 = raw[..raw.len() - 1].parse().ok().filter(|n| *n > 0)?;
    Some(format!("{n}{unit}"))
}

/// The configured window, or the default when absent or malformed.
pub fn effective(raw: Option<&str>) -> String {
    raw.and_then(parse).unwrap_or_else(|| DEFAULT.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_values() {
        assert_eq!(parse("3d").as_deref(), Some("3d"));
        assert_eq!(parse("24h").as_deref(), Some("24h"));
        assert_eq!(parse("2w").as_deref(), Some("2w"));
        assert_eq!(parse("all").as_deref(), Some("all"));
    }

    #[test]
    fn rejects_zero_and_malformed() {
        for bad in ["0d", "soon", "", "d", "-1d", "3m", "1.5d", "ALL"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn effective_falls_back_to_default() {
        assert_eq!(effective(None), "14d");
        assert_eq!(effective(Some("0d")), "14d");
        assert_eq!(effective(Some("soon")), "14d");
        assert_eq!(effective(Some("all")), "all");
    }
}
