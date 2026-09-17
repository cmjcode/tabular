//! Pure functions for ergonomic text manipulation in SQL editor.

/// Toggle SQL line comments (`-- `) on lines spanned by selection.
/// If all non-empty selected lines already start with `--`, comments are removed.
/// Otherwise, `-- ` is added to each line.
pub fn toggle_line_comments(text: &str, selection_start: usize, selection_end: usize) -> (String, usize, usize) {
    let reversed = selection_start > selection_end;
    let (sel_min, sel_max) = if reversed {
        (selection_end, selection_start)
    } else {
        (selection_start, selection_end)
    };

    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return (text.to_string(), selection_start, selection_end);
    }

    // Compute line byte offsets
    let mut line_offsets: Vec<(usize, usize)> = Vec::with_capacity(lines.len());
    let mut curr_offset = 0;
    for line in &lines {
        let start = curr_offset;
        let end = start + line.len();
        line_offsets.push((start, end));
        curr_offset = end + 1; // +1 for '\n'
    }

    // Determine which lines are spanned by the selection
    let mut target_lines = Vec::new();
    for (i, &(l_start, l_end)) in line_offsets.iter().enumerate() {
        if sel_min == sel_max {
            // Single cursor point
            if sel_min >= l_start && sel_min <= l_end {
                target_lines.push(i);
                break;
            }
        } else {
            // Selection range
            // Line overlaps if l_start <= sel_max and (sel_min < l_end || (sel_min == l_end && sel_min == l_start))
            // Special case: if sel_max is exactly at l_start of the next line, do not include it unless it's the only line
            if l_start < sel_max && l_end >= sel_min {
                target_lines.push(i);
            }
        }
    }

    if target_lines.is_empty() {
        if let Some(last_idx) = lines.len().checked_sub(1) {
            target_lines.push(last_idx);
        }
    }

    // Check if all non-empty target lines start with `--`
    let all_commented = target_lines.iter().all(|&idx| {
        let trimmed = lines[idx].trim_start();
        trimmed.is_empty() || trimmed.starts_with("--")
    });

    let mut new_lines = lines.clone();
    let mut delta_before_min = 0isize;
    let mut delta_total = 0isize;

    for (idx, line) in lines.iter().enumerate() {
        if target_lines.contains(&idx) {
            let (l_start, _) = line_offsets[idx];
            if all_commented {
                // Uncomment: remove leading `-- ` or `--`
                let trimmed = line.trim_start();
                if let Some(after_dashes) = trimmed.strip_prefix("--") {
                    let indent_len = line.len() - trimmed.len();
                    let indent = &line[..indent_len];
                    let rest = after_dashes.strip_prefix(' ').unwrap_or(after_dashes);
                    let modified = format!("{}{}", indent, rest);
                    let diff = modified.len() as isize - line.len() as isize;
                    if l_start < sel_min {
                        delta_before_min += diff;
                    }
                    delta_total += diff;
                    new_lines[idx] = Box::leak(modified.into_boxed_str());
                }
            } else {
                // Comment: add `-- `
                let trimmed = line.trim_start();
                let indent_len = line.len() - trimmed.len();
                let indent = &line[..indent_len];
                let modified = format!("{}-- {}", indent, trimmed);
                let diff = modified.len() as isize - line.len() as isize;
                if l_start < sel_min {
                    delta_before_min += diff;
                }
                delta_total += diff;
                new_lines[idx] = Box::leak(modified.into_boxed_str());
            }
        }
    }

    let result = new_lines.join("\n");
    let new_min = (sel_min as isize + delta_before_min).max(0) as usize;
    let new_max = (sel_max as isize + delta_total).max(new_min as isize) as usize;

    if reversed {
        (result, new_max, new_min)
    } else {
        (result, new_min, new_max)
    }
}

/// Duplicate current line or lines spanned by selection downwards.
pub fn duplicate_lines(text: &str, start_pos: usize, end_pos: usize) -> (String, usize, usize) {
    let (sel_min, sel_max) = if start_pos > end_pos {
        (end_pos, start_pos)
    } else {
        (start_pos, end_pos)
    };

    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return (text.to_string(), start_pos, end_pos);
    }

    let mut line_offsets: Vec<(usize, usize)> = Vec::with_capacity(lines.len());
    let mut curr_offset = 0;
    for line in &lines {
        let start = curr_offset;
        let end = start + line.len();
        line_offsets.push((start, end));
        curr_offset = end + 1;
    }

    let mut first_line_idx = 0;
    let mut last_line_idx = lines.len() - 1;

    for (i, &(l_start, l_end)) in line_offsets.iter().enumerate() {
        if sel_min >= l_start && sel_min <= l_end {
            first_line_idx = i;
            break;
        }
    }

    for (i, &(l_start, _l_end)) in line_offsets.iter().enumerate().rev() {
        if sel_max >= l_start {
            // If sel_max is exactly at l_start and sel_max > sel_min, exclude this line
            if sel_max == l_start && sel_max > sel_min && i > first_line_idx {
                last_line_idx = i - 1;
            } else {
                last_line_idx = i;
            }
            break;
        }
    }

    let block_lines = &lines[first_line_idx..=last_line_idx];
    let block_text = block_lines.join("\n");
    let block_byte_len = block_text.len();

    let insert_pos = line_offsets[last_line_idx].1;
    let mut result = String::with_capacity(text.len() + block_byte_len + 1);

    result.push_str(&text[..insert_pos]);
    result.push('\n');
    result.push_str(&block_text);
    result.push_str(&text[insert_pos..]);

    let offset_diff = block_byte_len + 1;
    (result, start_pos + offset_diff, end_pos + offset_diff)
}

/// Move selected lines up or down (Alt+Up / Alt+Down).
pub fn move_lines(text: &str, start_pos: usize, end_pos: usize, move_up: bool) -> (String, usize, usize) {
    let reversed = start_pos > end_pos;
    let (sel_min, sel_max) = if reversed {
        (end_pos, start_pos)
    } else {
        (start_pos, end_pos)
    };

    let mut lines: Vec<String> = text.split('\n').map(|s| s.to_string()).collect();
    if lines.is_empty() {
        return (text.to_string(), start_pos, end_pos);
    }

    let mut line_offsets: Vec<(usize, usize)> = Vec::with_capacity(lines.len());
    let mut curr_offset = 0;
    for line in &lines {
        let start = curr_offset;
        let end = start + line.len();
        line_offsets.push((start, end));
        curr_offset = end + 1;
    }

    let mut first_line_idx = 0;
    let mut last_line_idx = lines.len() - 1;

    for (i, &(l_start, l_end)) in line_offsets.iter().enumerate() {
        if sel_min >= l_start && sel_min <= l_end {
            first_line_idx = i;
            break;
        }
    }

    for (i, &(l_start, _l_end)) in line_offsets.iter().enumerate().rev() {
        if sel_max >= l_start {
            if sel_max == l_start && sel_max > sel_min && i > first_line_idx {
                last_line_idx = i - 1;
            } else {
                last_line_idx = i;
            }
            break;
        }
    }

    if move_up {
        if first_line_idx == 0 {
            // Already at the top
            return (text.to_string(), start_pos, end_pos);
        }
        let target_idx = first_line_idx - 1;
        let line_above_len = lines[target_idx].len() + 1;
        // Move lines up: remove line_above and insert it after last_line_idx
        let line_above = lines.remove(target_idx);
        lines.insert(last_line_idx, line_above);

        let result = lines.join("\n");
        let new_start = start_pos.saturating_sub(line_above_len);
        let new_end = end_pos.saturating_sub(line_above_len);
        (result, new_start, new_end)
    } else {
        if last_line_idx + 1 >= lines.len() {
            // Already at bottom
            return (text.to_string(), start_pos, end_pos);
        }
        let target_idx = last_line_idx + 1;
        let line_below_len = lines[target_idx].len() + 1;
        let line_below = lines.remove(target_idx);
        lines.insert(first_line_idx, line_below);

        let result = lines.join("\n");
        let new_start = start_pos + line_below_len;
        let new_end = end_pos + line_below_len;
        (result, new_start, new_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toggle_comment_single_line() {
        let text = "SELECT 1;\nSELECT 2;";
        // cursor at 'SELECT 1;'
        let (res, s, e) = toggle_line_comments(text, 2, 2);
        assert_eq!(res, "-- SELECT 1;\nSELECT 2;");
        assert_eq!(s, 5);
        assert_eq!(e, 5);

        // Toggle again to uncomment
        let (res2, s2, e2) = toggle_line_comments(&res, 5, 5);
        assert_eq!(res2, "SELECT 1;\nSELECT 2;");
        assert_eq!(s2, 2);
        assert_eq!(e2, 2);
    }

    #[test]
    fn test_toggle_comment_indented() {
        let text = "    SELECT 1;";
        let (res, _, _) = toggle_line_comments(text, 4, 4);
        assert_eq!(res, "    -- SELECT 1;");

        let (res2, _, _) = toggle_line_comments(&res, 7, 7);
        assert_eq!(res2, "    SELECT 1;");
    }

    #[test]
    fn test_duplicate_lines() {
        let text = "line 1\nline 2\nline 3";
        // cursor on line 2 (offset 7)
        let (res, s, e) = duplicate_lines(text, 7, 7);
        assert_eq!(res, "line 1\nline 2\nline 2\nline 3");
        assert_eq!(s, 7 + 7);
        assert_eq!(e, 7 + 7);
    }

    #[test]
    fn test_move_lines() {
        let text = "line 1\nline 2\nline 3";
        // move line 2 up
        let (res, s, e) = move_lines(text, 7, 7, true);
        assert_eq!(res, "line 2\nline 1\nline 3");
        assert_eq!(s, 0);
        assert_eq!(e, 0);

        // move line 2 down (now at line 1 index 0)
        let (res2, _, _) = move_lines(&res, 0, 0, false);
        assert_eq!(res2, "line 1\nline 2\nline 3");
    }
}
