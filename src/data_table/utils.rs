pub(super) fn parse_enum_values(type_str: &str) -> Option<Vec<String>> {
    let lower = type_str.to_lowercase();
    if !lower.starts_with("enum") && !lower.starts_with("set") {
        return None;
    }

    // Find content inside parentheses
    if let Some((start_idx, end_idx)) = type_str
        .find('(')
        .zip(type_str.rfind(')'))
        .filter(|&(start, end)| start < end)
    {
                let content = &type_str[start_idx + 1..end_idx];
                let chars: Vec<char> = content.chars().collect();
                let mut values = Vec::new();
                let mut current = String::new();
                let mut in_quote = false;
                let mut i = 0;

                while i < chars.len() {
                    let c = chars[i];
                    if in_quote {
                        if c == '\'' {
                            // Check for double quote escaping (e.g. 'O''Neil')
                            if i + 1 < chars.len() && chars[i+1] == '\'' {
                                current.push('\'');
                                i += 1;
                            } else {
                                in_quote = false;
                            }
                        } else if c == '\\' {
                             // Handle backslash escaping
                             if i + 1 < chars.len() {
                                 current.push(chars[i+1]);
                                 i += 1;
                             } else {
                                 current.push(c);
                             }
                        } else {
                            current.push(c);
                        }
                    } else if c == '\'' {
                        in_quote = true;
                    } else if c == ',' {
                            values.push(current.clone());
                            current.clear();
                        } else if !c.is_whitespace() {
                            // Should not happen for valid ENUMs, but handle just in case
                            // If we encounter text outside quotes (other than comma/space), push it?
                            // Safest is to ignore or assume it's part of value if we support unquoted (we shouldn't)
                        }

                    i += 1;
                }
                values.push(current);
                return Some(values);
            }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_enum_values() {
        assert_eq!(parse_enum_values("enum('a','b')"), Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(parse_enum_values("ENUM('YES','NO')"), Some(vec!["YES".to_string(), "NO".to_string()]));
        assert_eq!(parse_enum_values("enum('a,b','c')"), Some(vec!["a,b".to_string(), "c".to_string()]));
        assert_eq!(parse_enum_values("enum('O''Neil','Smith')"), Some(vec!["O'Neil".to_string(), "Smith".to_string()]));
        assert_eq!(parse_enum_values("varchar(255)"), None);
        // Test set
        assert_eq!(parse_enum_values("set('a','b')"), Some(vec!["a".to_string(), "b".to_string()]));
        // Test no quotes (rare but possible? No, MySQL enums are always quoted strings)
        // Test spaces
        assert_eq!(parse_enum_values("enum( 'a' , 'b' )"), Some(vec!["a".to_string(), "b".to_string()]));
    }
}
