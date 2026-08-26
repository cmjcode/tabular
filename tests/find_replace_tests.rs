use tabular::editor::{get_search_matches, EditorSearchMatch};

#[test]
fn test_plain_text_search_case_insensitive() {
    let sql = "SELECT name, Name, NAME FROM users WHERE name = 'John';";
    let matches = get_search_matches(sql, "name", false, false, false, false, None).unwrap();
    assert_eq!(matches.len(), 4);
    assert_eq!(matches[0], EditorSearchMatch { start: 7, end: 11 });
    assert_eq!(matches[1], EditorSearchMatch { start: 13, end: 17 });
    assert_eq!(matches[2], EditorSearchMatch { start: 19, end: 23 });
    assert_eq!(matches[3], EditorSearchMatch { start: 41, end: 45 });
}

#[test]
fn test_plain_text_search_case_sensitive() {
    let sql = "SELECT name, Name, NAME FROM users WHERE name = 'John';";
    let matches = get_search_matches(sql, "name", true, false, false, false, None).unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0], EditorSearchMatch { start: 7, end: 11 });
    assert_eq!(matches[1], EditorSearchMatch { start: 41, end: 45 });
}

#[test]
fn test_plain_text_search_whole_word() {
    let sql = "SELECT username, user, user_id FROM users WHERE user = 1;";
    let matches = get_search_matches(sql, "user", false, true, false, false, None).unwrap();
    assert_eq!(matches.len(), 2);
    assert_eq!(&sql[matches[0].start..matches[0].end], "user");
    assert_eq!(&sql[matches[1].start..matches[1].end], "user");
}

#[test]
fn test_regex_search_patterns() {
    let sql = "SELECT col1, col2, col99, other FROM my_table WHERE col123 > 0;";
    let matches = get_search_matches(sql, r"col\d+", false, false, true, false, None).unwrap();
    assert_eq!(matches.len(), 4);
    assert_eq!(&sql[matches[0].start..matches[0].end], "col1");
    assert_eq!(&sql[matches[1].start..matches[1].end], "col2");
    assert_eq!(&sql[matches[2].start..matches[2].end], "col99");
    assert_eq!(&sql[matches[3].start..matches[3].end], "col123");
}

#[test]
fn test_regex_invalid_pattern_returns_error() {
    let sql = "SELECT * FROM users;";
    let result = get_search_matches(sql, r"col[0-9", false, false, true, false, None);
    assert!(result.is_err());
}

#[test]
fn test_in_selection_search() {
    let sql = "SELECT name FROM a; SELECT name FROM b; SELECT name FROM c;";
    // Select range covering only the middle query "SELECT name FROM b;"
    let sel_start = 20;
    let sel_end = 39;
    let matches = get_search_matches(sql, "name", false, false, false, true, Some((sel_start, sel_end))).unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(&sql[matches[0].start..matches[0].end], "name");
    assert!(matches[0].start >= sel_start && matches[0].end <= sel_end);
}
