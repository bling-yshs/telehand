//! Output truncation, following pi's `truncate.ts`: whichever of the line
//! limit and the byte limit is hit first wins, and partial lines are never
//! returned.

pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncation {
    pub content: String,
    pub truncated_by: Option<TruncatedBy>,
    pub output_lines: usize,
    /// The first line alone exceeds the byte limit; `content` is empty.
    pub first_line_exceeds_limit: bool,
}

impl Truncation {
    pub fn truncated(&self) -> bool {
        self.truncated_by.is_some()
    }
}

/// Human-readable size, as pi's `formatSize`.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn split_lines_for_counting(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Keep the first lines of `content` within `max_lines` and `max_bytes`.
pub fn truncate_head(content: &str, max_lines: usize, max_bytes: usize) -> Truncation {
    let lines = split_lines_for_counting(content);
    if lines.len() <= max_lines && content.len() <= max_bytes {
        return Truncation {
            content: content.to_string(),
            truncated_by: None,
            output_lines: lines.len(),
            first_line_exceeds_limit: false,
        };
    }

    if lines[0].len() > max_bytes {
        return Truncation {
            content: String::new(),
            truncated_by: Some(TruncatedBy::Bytes),
            output_lines: 0,
            first_line_exceeds_limit: true,
        };
    }

    let mut output: Vec<&str> = Vec::new();
    let mut output_bytes = 0;
    let mut truncated_by = TruncatedBy::Lines;
    for (i, line) in lines.iter().enumerate().take(max_lines) {
        let line_bytes = line.len() + usize::from(i > 0);
        if output_bytes + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output.push(line);
        output_bytes += line_bytes;
    }
    if output.len() >= max_lines && output_bytes <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    Truncation {
        content: output.join("\n"),
        truncated_by: Some(truncated_by),
        output_lines: output.len(),
        first_line_exceeds_limit: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_formatted_like_pi() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(50 * 1024), "50.0KB");
        assert_eq!(format_size(1536), "1.5KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0MB");
    }

    #[test]
    fn content_within_limits_is_untouched() {
        let t = truncate_head("a\nb\n", 2, 100);
        assert_eq!(t.content, "a\nb\n");
        assert!(!t.truncated());
        assert_eq!(t.output_lines, 2);
    }

    #[test]
    fn line_limit_keeps_first_lines() {
        let t = truncate_head("a\nb\nc\nd", 2, 100);
        assert_eq!(t.content, "a\nb");
        assert_eq!(t.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(t.output_lines, 2);
    }

    #[test]
    fn byte_limit_never_returns_partial_lines() {
        let t = truncate_head("aaaa\nbbbb\ncccc", 100, 10);
        assert_eq!(t.content, "aaaa\nbbbb");
        assert_eq!(t.truncated_by, Some(TruncatedBy::Bytes));
        assert_eq!(t.output_lines, 2);
    }

    #[test]
    fn oversized_first_line_is_flagged() {
        let t = truncate_head("aaaaaaaaaaaa\nb", 100, 10);
        assert!(t.first_line_exceeds_limit);
        assert_eq!(t.content, "");
        assert_eq!(t.output_lines, 0);
    }
}
