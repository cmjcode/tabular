fn main() {
    let text = "SELECT *
FROM foxlogger_wh.summary_data_dates
WHERE imei = '0356153592967175';

SELECT last_time
FROM foxlogger.`user_data`
WHERE imei = '0353701096261488';";

    // Approximate cursor position: end of first query + 1 (semicolon)
    // The query part:
    // SELECT * \n FROM foxlogger_wh.summary_data_dates \n WHERE imei = '0356153592967175';
    
    // Let's print chars and find index of first semicolon.
    let mut semi_idx = 0;
    for (i, c) in text.char_indices() {
        if c == ';' {
            semi_idx = i;
            break;
        }
    }
    
    println!("First semicolon byte index: {}", semi_idx);
    
    // Simulate cursor RIGHT AFTER semicolon
    let cur = semi_idx + 1;
    println!("Cursor position (byte): {}", cur);
    
    test_parser(text, cur);
}

fn test_parser(text: &str, cur: usize) {
    let text_len = text.len();
    let (start_byte, end_byte) = {
       let mut stmt_start = 0;
       let mut found_range = (0, text_len);
       
       let mut chars = text.char_indices().peekable();
       let mut in_quote = None; // None, Some('\''), Some('"'), Some('`')
       let mut in_line_comment = false;
       let mut in_block_comment = false;
       
       while let Some((i, c)) = chars.next() {
           // 1. Handle String Literals
           if let Some(q) = in_quote {
               if c == '\\' {
                   // Skip next char (escape)
                   let _ = chars.next();
               } else if c == q {
                   in_quote = None;
               }
               continue;
           }

           // 2. Handle Block Comments
           if in_block_comment {
               if c == '*' {
                   if let Some(&(_, '/')) = chars.peek() {
                       chars.next(); // consume '/'
                       in_block_comment = false;
                   }
               }
               continue;
           }

           // 3. Handle Line Comments
           if in_line_comment {
               if c == '\n' || c == '\r' {
                   in_line_comment = false;
               }
               continue;
           }

           // 4. Normal Mode
           match c {
               '\'' | '"' | '`' => in_quote = Some(c),
               '-' => {
                   if let Some(&(_, '-')) = chars.peek() {
                       chars.next(); // consume second '-'
                       in_line_comment = true;
                   }
               }
               '#' => in_line_comment = true,
               '/' => {
                   if let Some(&(_, '*')) = chars.peek() {
                       chars.next(); // consume '*'
                       in_block_comment = true;
                   }
               }
               ';' => {
                   // Statement ends here
                   let stmt_end = i + 1; 
                   println!("Found semicolon at byte {}, stmt_end={}", i, stmt_end);
                   println!("Checking: cur ({}) >= stmt_start ({}) && cur ({}) <= stmt_end ({})", cur, stmt_start, cur, stmt_end);
                   
                   if cur >= stmt_start && cur <= stmt_end {
                       found_range = (stmt_start, stmt_end);
                       break;
                   }
                   stmt_start = stmt_end;
               }
               _ => {}
           }
       }
       // Handle last statement if cursor is past the last semicolon or no semicolons
       if found_range.1 == text_len && cur < stmt_start {
            // Logic error in fallback??
            // Original code:
            // if cur >= stmt_start { found_range = (stmt_start, text_len); }
            // Let's use exact code from editor.rs
            // Note: `break` exits the loop, so we only reach here if loop finishes OR break happens?
            // No, break exits `while`. We are inside the block expression.
            // Wait, if we `break`, `found_range` is already set.
            // But we can flow through if no break.
       }
       
       // Fallback check from editor.rs (simplified for this test context)
       // Original:
       //    if cur >= stmt_start {
       //        found_range = (stmt_start, text_len);
       //    }
       // Note: This overrides found_range if we broke out??
       // NO! `break` breaks the `while`. We proceed to the lines after `while`.
       
       // CRITICAL FLAW potential:
       // If we found a range and broke, `found_range` is set.
       // BUT... `cur >= stmt_start` might ALSO be true for the *next* simplified check?
       // Let's trace.
       
       // If we matched Stmt 1. `stmt_start`=0. `stmt_end`=10. `cur`=5.
       // `found_range` = (0, 10). `break`.
       // Loop ends.
       // `cur` (5) >= `stmt_start` (0).
       // `found_range` becomes (0, text_len) !!!!
       
       // BUG FOUND! 
       // `stmt_start` is NOT updated when we `break`.
       // So `cur` is indeed >= `stmt_start`.
       // We overwrite the correct result with the "rest of file" result.
       
       // Logic needed:
       // We should only do the fallback if we did NOT find a range yet.
       
       if cur >= stmt_start && found_range.1 == text_len {
            // Only if we haven't narrowed it down yet? 
            // Wait, initial found_range is (0, text_len).
            // So we can't distinguish "default" from "found (0, text_len)".
            // Actually, if we break, we should RETURN/yield immediately or set a flag.
       }
       
       found_range 
    };
    
    println!("Resolved Range: {:?}", (start_byte, end_byte));
}
