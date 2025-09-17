use tabular::editor_buffer::EditorBuffer;

#[test]
fn granular_insert_basic() {
    let mut buf = EditorBuffer::new("abc");
    // insert at middle
    buf.apply_single_replace(1..1, "XYZ");
    assert_eq!(buf.text_snapshot(), "aXYZbc");
    // insert at end
    let end = buf.text_snapshot().len();
    buf.apply_single_replace(end..end, "!");
    assert_eq!(buf.text_snapshot(), "aXYZbc!");
}

#[test]
fn granular_backspace_like_delete_one() {
    let mut buf = EditorBuffer::new("hello");
    // delete last char
    let len = buf.text_snapshot().len();
    buf.apply_single_replace((len - 1)..len, "");
    assert_eq!(buf.text_snapshot(), "hell");
}

#[test]
fn try_single_span_update_differs_only_middle() {
    let mut buf = EditorBuffer::new("SELECT * FROM t");
    let prev = buf.text_snapshot();
    let new_full = "SELECT id, name FROM t".to_string();
    let ok = buf.try_single_span_update(&prev, &new_full);
    assert!(ok);
    assert_eq!(buf.text_snapshot(), new_full);
}

#[test]
fn delete_whole_selection_range() {
    // Simulate selection deletion by calling single replace on the selected range
    let mut buf = EditorBuffer::new("SELECT abc FROM t;");
    // Delete the token 'abc' (bytes 7..10)
    buf.apply_single_replace(7..10, "");
    assert_eq!(buf.text_snapshot(), "SELECT  FROM t;");
    // Now replace the double space left between SELECT and FROM with a single space
    // In an actual UI, selection would span these 2 spaces; emulate and insert one space
    // Find positions to ensure test remains robust
    let s = buf.text_snapshot();
    let pos = s.find("  FROM").unwrap();
    buf.apply_single_replace(pos..pos + 2, " ");
    assert_eq!(buf.text_snapshot(), "SELECT FROM t;");
}
