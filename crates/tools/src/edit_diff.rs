//! Text matching and replacement for `edit`, ported from pi's `edit-diff.ts`.
//! Offsets are byte offsets; every match lands on a char boundary.

use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
}

pub fn detect_line_ending(content: &str) -> &'static str {
    match (content.find("\r\n"), content.find('\n')) {
        (Some(crlf), Some(lf)) if crlf < lf => "\r\n",
        _ => "\n",
    }
}

pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// Whitespace as JavaScript's `trimEnd` sees it.
fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{0085}') || c == '\u{FEFF}'
}

/// Normalize text for fuzzy matching: NFKC, trailing whitespace stripped per
/// line, smart quotes, Unicode dashes and special spaces mapped to ASCII.
pub fn normalize_for_fuzzy_match(text: &str) -> String {
    let nfkc: String = text.nfkc().collect();
    let trimmed = nfkc
        .split('\n')
        .map(|line| line.trim_end_matches(is_js_whitespace))
        .collect::<Vec<_>>()
        .join("\n");
    trimmed
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            c => c,
        })
        .collect()
}

/// Lines including their trailing `\n` (the last line may lack one).
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    content.split_inclusive('\n').collect()
}

#[derive(Debug, Clone, Copy)]
struct LineSpan {
    start: usize,
    end: usize,
}

fn line_spans(content: &str) -> Vec<LineSpan> {
    let mut offset = 0;
    split_lines_with_endings(content)
        .into_iter()
        .map(|line| {
            let span = LineSpan {
                start: offset,
                end: offset + line.len(),
            };
            offset = span.end;
            span
        })
        .collect()
}

#[derive(Debug, Clone)]
struct Replacement {
    edit_index: usize,
    match_index: usize,
    match_length: usize,
    new_text: String,
}

const OUTSIDE_BASE: &str = "Replacement range is outside the base content.";

fn replacement_line_range(
    lines: &[LineSpan],
    replacement: &Replacement,
) -> Result<(usize, usize), String> {
    let start = replacement.match_index;
    let end = replacement.match_index + replacement.match_length;
    let start_line = lines
        .iter()
        .position(|line| start >= line.start && start < line.end)
        .ok_or(OUTSIDE_BASE)?;
    let mut end_line = start_line;
    while end_line < lines.len() && lines[end_line].end < end {
        end_line += 1;
    }
    if end_line >= lines.len() {
        return Err(OUTSIDE_BASE.to_string());
    }
    Ok((start_line, end_line + 1))
}

/// Apply replacements (sorted by position) to `content`, whose offset 0 is
/// `offset` in the replacement coordinates.
fn apply_replacements(content: &str, replacements: &[&Replacement], offset: usize) -> String {
    let mut result = content.to_string();
    for replacement in replacements.iter().rev() {
        let index = replacement.match_index - offset;
        result.replace_range(
            index..index + replacement.match_length,
            &replacement.new_text,
        );
    }
    result
}

/// Apply replacements matched against `base` (a normalized view of
/// `original`), rewriting only the lines they touch from `base` and copying
/// every other line from `original`.
fn apply_replacements_preserving_unchanged_lines(
    original: &str,
    base: &str,
    replacements: &[Replacement],
) -> Result<String, String> {
    let original_lines = split_lines_with_endings(original);
    let base_lines = line_spans(base);
    if original_lines.len() != base_lines.len() {
        return Err("Cannot preserve unchanged lines because the base content has a different line count.".to_string());
    }

    struct Group<'a> {
        start_line: usize,
        end_line: usize,
        replacements: Vec<&'a Replacement>,
    }
    let mut sorted: Vec<&Replacement> = replacements.iter().collect();
    sorted.sort_by_key(|r| r.match_index);
    let mut groups: Vec<Group> = Vec::new();
    for replacement in sorted {
        let (start_line, end_line) = replacement_line_range(&base_lines, replacement)?;
        if let Some(current) = groups.last_mut()
            && start_line < current.end_line
        {
            current.end_line = current.end_line.max(end_line);
            current.replacements.push(replacement);
            continue;
        }
        groups.push(Group {
            start_line,
            end_line,
            replacements: vec![replacement],
        });
    }

    let mut original_index = 0;
    let mut result = String::new();
    for group in groups {
        result.push_str(&original_lines[original_index..group.start_line].concat());
        let group_start = base_lines[group.start_line].start;
        let group_end = base_lines[group.end_line - 1].end;
        result.push_str(&apply_replacements(
            &base[group_start..group_end],
            &group.replacements,
            group_start,
        ));
        original_index = group.end_line;
    }
    result.push_str(&original_lines[original_index..].concat());
    Ok(result)
}

struct Found {
    index: usize,
    length: usize,
    used_fuzzy_match: bool,
}

/// Find `old_text` in `content`, exactly first, then in fuzzy-normalized space.
fn fuzzy_find_text(content: &str, old_text: &str) -> Option<Found> {
    if let Some(index) = content.find(old_text) {
        return Some(Found {
            index,
            length: old_text.len(),
            used_fuzzy_match: false,
        });
    }
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    fuzzy_content.find(&fuzzy_old).map(|index| Found {
        index,
        length: fuzzy_old.len(),
        used_fuzzy_match: true,
    })
}

/// Occurrences counted in fuzzy-normalized space, as JavaScript's
/// `content.split(oldText).length - 1`.
fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy_match(content);
    let fuzzy_old = normalize_for_fuzzy_match(old_text);
    if fuzzy_old.is_empty() {
        return fuzzy_content.chars().count().saturating_sub(1);
    }
    fuzzy_content.matches(&fuzzy_old).count()
}

fn not_found_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        )
    } else {
        format!(
            "Could not find edits[{index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    }
}

fn duplicate_error(path: &str, index: usize, total: usize, occurrences: usize) -> String {
    if total == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    }
}

fn empty_old_text_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{index}].oldText must not be empty in {path}.")
    }
}

fn no_change_error(path: &str, total: usize) -> String {
    if total == 1 {
        format!(
            "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        )
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

/// Apply `edits` to LF-normalized `content`. All edits are matched against
/// the original content; if any needs fuzzy matching, matching happens in
/// fuzzy-normalized space and only the touched lines are rewritten.
pub fn apply_edits_to_normalized_content(
    content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<String, String> {
    let edits: Vec<Edit> = edits
        .iter()
        .map(|edit| Edit {
            old_text: normalize_to_lf(&edit.old_text),
            new_text: normalize_to_lf(&edit.new_text),
        })
        .collect();
    let total = edits.len();

    if let Some(index) = edits.iter().position(|edit| edit.old_text.is_empty()) {
        return Err(empty_old_text_error(path, index, total));
    }

    let used_fuzzy_match = edits.iter().any(|edit| {
        fuzzy_find_text(content, &edit.old_text).is_some_and(|found| found.used_fuzzy_match)
    });
    let base = if used_fuzzy_match {
        normalize_for_fuzzy_match(content)
    } else {
        content.to_string()
    };

    let mut matched = Vec::with_capacity(total);
    for (index, edit) in edits.iter().enumerate() {
        let found = fuzzy_find_text(&base, &edit.old_text)
            .ok_or_else(|| not_found_error(path, index, total))?;
        let occurrences = count_occurrences(&base, &edit.old_text);
        if occurrences > 1 {
            return Err(duplicate_error(path, index, total, occurrences));
        }
        matched.push(Replacement {
            edit_index: index,
            match_index: found.index,
            match_length: found.length,
            new_text: edit.new_text.clone(),
        });
    }

    matched.sort_by_key(|r| r.match_index);
    for pair in matched.windows(2) {
        let (previous, current) = (&pair[0], &pair[1]);
        if previous.match_index + previous.match_length > current.match_index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                previous.edit_index, current.edit_index
            ));
        }
    }

    let new_content = if used_fuzzy_match {
        apply_replacements_preserving_unchanged_lines(content, &base, &matched)?
    } else {
        apply_replacements(&base, &matched.iter().collect::<Vec<_>>(), 0)
    };
    if new_content == content {
        return Err(no_change_error(path, total));
    }
    Ok(new_content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(old: &str, new: &str) -> Edit {
        Edit {
            old_text: old.into(),
            new_text: new.into(),
        }
    }

    fn apply(content: &str, edits: &[Edit]) -> Result<String, String> {
        apply_edits_to_normalized_content(content, edits, "f.txt")
    }

    #[test]
    fn line_endings() {
        assert_eq!(detect_line_ending("a\r\nb\nc"), "\r\n");
        assert_eq!(detect_line_ending("a\nb\r\nc"), "\n");
        assert_eq!(detect_line_ending("abc"), "\n");
        assert_eq!(normalize_to_lf("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(restore_line_endings("a\nb", "\r\n"), "a\r\nb");
    }

    #[test]
    fn exact_single_and_multiple_edits() {
        assert_eq!(apply("one two three", &[edit("two", "2")]).unwrap(), "one 2 three");
        assert_eq!(
            apply(
                "a\nb\nc\nd\n",
                &[edit("d", "D"), edit("a", "A")]
            )
            .unwrap(),
            "A\nb\nc\nD\n"
        );
    }

    #[test]
    fn edits_match_the_original_not_incrementally() {
        // The second edit's oldText only exists in the original content.
        assert_eq!(
            apply("x y", &[edit("x", "y"), edit("y", "z")]).unwrap(),
            "y z"
        );
    }

    #[test]
    fn fuzzy_match_rewrites_only_touched_lines() {
        let content = "keep \u{201C}smart\u{201D}   \nlet s = \u{2018}a\u{2019};   \ntail\u{2014}x  \n";
        let result = apply(content, &[edit("let s = 'a';", "let s = 'b';")]).unwrap();
        assert_eq!(
            result,
            "keep \u{201C}smart\u{201D}   \nlet s = 'b';\ntail\u{2014}x  \n"
        );
    }

    #[test]
    fn fuzzy_match_ignores_trailing_whitespace_in_old_text() {
        let result = apply("fn a() {  \n}\n", &[edit("fn a() {\n}", "fn b() {\n}")]).unwrap();
        assert_eq!(result, "fn b() {\n}\n");
    }

    #[test]
    fn errors_match_pi() {
        assert_eq!(
            apply("abc", &[edit("", "x")]).unwrap_err(),
            "oldText must not be empty in f.txt."
        );
        assert_eq!(
            apply("abc", &[edit("a", "x"), edit("", "y")]).unwrap_err(),
            "edits[1].oldText must not be empty in f.txt."
        );
        assert_eq!(
            apply("abc", &[edit("zzz", "x")]).unwrap_err(),
            "Could not find the exact text in f.txt. The old text must match exactly including all whitespace and newlines."
        );
        assert_eq!(
            apply("abc", &[edit("a", "x"), edit("zzz", "y")]).unwrap_err(),
            "Could not find edits[1] in f.txt. The oldText must match exactly including all whitespace and newlines."
        );
        assert_eq!(
            apply("aa aa", &[edit("aa", "x")]).unwrap_err(),
            "Found 2 occurrences of the text in f.txt. The text must be unique. Please provide more context to make it unique."
        );
        assert_eq!(
            apply("b aa aa", &[edit("b", "c"), edit("aa", "x")]).unwrap_err(),
            "Found 2 occurrences of edits[1] in f.txt. Each oldText must be unique. Please provide more context to make it unique."
        );
        assert_eq!(
            apply("abcdef", &[edit("abcd", "x"), edit("cdef", "y")]).unwrap_err(),
            "edits[0] and edits[1] overlap in f.txt. Merge them into one edit or target disjoint regions."
        );
        assert_eq!(
            apply("abc", &[edit("b", "b")]).unwrap_err(),
            "No changes made to f.txt. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        );
        assert_eq!(
            apply("abc", &[edit("a", "a"), edit("b", "b")]).unwrap_err(),
            "No changes made to f.txt. The replacements produced identical content."
        );
    }
}
