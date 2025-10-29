# Multi-Cursor Feature

## Overview
Tabular now supports VSCode-style multi-cursor editing with keyboard shortcuts for efficient text manipulation.

## How to Use

### CMD+D / CTRL+D - Add Next Occurrence
1. **First Press**: 
   - If you have text selected, it will be used as the search term
   - If no selection, the word under cursor will be selected automatically
   - The selection is highlighted with a blue background

2. **Subsequent Presses**: 
   - Each press finds the next occurrence of the selected text
   - A new cursor is added at each occurrence
   - The search wraps around to the beginning of the document
   - All selections are highlighted with blue backgrounds (primary is brighter)

3. **Multi-Cursor Typing**:
   - Once you have multiple cursors, any typing applies to ALL cursor positions
   - Backspace and Delete work on all cursors simultaneously
   - This is perfect for renaming variables, adding prefixes/suffixes, etc.

### ESC - Clear Multi-Selection
- Press `Escape` to clear all multi-cursors and return to single cursor mode

### Arrow Keys - Clear Multi-Selection
- Navigating with arrow keys (without Shift) automatically clears multi-selection
- This provides natural single-cursor behavior when moving around

### Visual Feedback
- **Primary Selection**: Highlighted with steel blue background (RGB: 70, 130, 180)
- **Additional Selections**: Highlighted with lighter blue background (RGB: 100, 150, 200)
- **Cursors**: Thin blue vertical lines at each selection position
- All highlights use semi-transparency to preserve text readability

## Examples

### Example 1: Rename Variable
```sql
SELECT user_id, user_name, user_email FROM users;
```
1. Place cursor on first `user_`
2. Press CMD+D three times to select all occurrences
3. Type `customer_` to replace all at once
Result:
```sql
SELECT customer_id, customer_name, customer_email FROM users;
```

### Example 2: Add Quotes to Multiple Values
```sql
INSERT INTO config VALUES (key1, value1), (key2, value2), (key3, value3);
```
1. Select `value1`
2. Press CMD+D twice to select `value2` and `value3`
3. Press `'` to add opening quote, then End key, then `'` again
Result:
```sql
INSERT INTO config VALUES (key1, 'value1'), (key2, 'value2'), (key3, 'value3');
```

### Example 3: Bulk Comment Addition
```sql
column_a
column_b
column_c
```
1. Place cursor at start of first line
2. Press CMD+D twice (or use Alt+Click for non-sequential positions)
3. Type `-- ` to comment all lines
Result:
```sql
-- column_a
-- column_b
-- column_c
```

## Additional Features

### Alt/Option + Click
- Hold Alt (Option on Mac) and click to add an additional cursor at that position
- Great for adding cursors at non-sequential locations

## Implementation Details
- Uses Unicode-aware word boundary detection for proper word selection
- Handles multi-byte characters correctly (UTF-8 support)
- Efficient search algorithm with wrap-around capability
- Synchronized with the main editor state for consistent behavior
- Logging available for debugging (check console with `RUST_LOG=debug`)

## Tips
- Start with a selection of the text you want to find
- Use CMD+D incrementally - you can stop at any time
- Press Escape if you accidentally select too many occurrences
- Combine with Alt+Click for complex multi-cursor scenarios
- The first CMD+D just selects/highlights - the second CMD+D starts finding occurrences

## Keyboard Shortcuts Summary
| Shortcut | Action |
|----------|--------|
| `CMD+D` (Mac) / `CTRL+D` (Win/Linux) | Select next occurrence |
| `ESC` | Clear multi-selection |
| `Alt+Click` | Add cursor at click position |
| Arrow Keys | Clear multi-selection (when not holding Shift) |

---

This feature brings powerful text editing capabilities from modern code editors into Tabular's SQL editor, making bulk edits fast and intuitive.
