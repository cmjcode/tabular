//! Diagnostik command Redis di Query Editor: command tidak dikenal, command
//! yang tidak dijalankan editor, dan jumlah argumen yang salah.
//!
//! Aturannya mengikuti `connection::execute::execute_redis_query_job`: editor
//! hanya menjalankan GET, KEYS, SCAN, INFO, dan HGETALL.

use super::words;
use super::{DiagCode, DiagSource, diag};
use crate::query_tools::{LintMessage, LintSeverity};

/// Command yang dijalankan Query Editor.
const SUPPORTED: &[&str] = &["GET", "KEYS", "SCAN", "INFO", "HGETALL"];

/// Command Redis yang mengubah data/server (selain daftar baca di
/// `agent::classify::REDIS_READ_COMMANDS`), untuk membedakan "tidak didukung"
/// dari "salah ketik".
const OTHER_COMMANDS: &[&str] = &[
    "SET",
    "SETEX",
    "SETNX",
    "PSETEX",
    "MSET",
    "MSETNX",
    "GETSET",
    "GETDEL",
    "GETEX",
    "APPEND",
    "INCR",
    "INCRBY",
    "INCRBYFLOAT",
    "DECR",
    "DECRBY",
    "DEL",
    "UNLINK",
    "EXPIRE",
    "PEXPIRE",
    "EXPIREAT",
    "PEXPIREAT",
    "PERSIST",
    "RENAME",
    "RENAMENX",
    "COPY",
    "MOVE",
    "HSET",
    "HMSET",
    "HSETNX",
    "HDEL",
    "HINCRBY",
    "HINCRBYFLOAT",
    "LPUSH",
    "RPUSH",
    "LPUSHX",
    "RPUSHX",
    "LPOP",
    "RPOP",
    "LSET",
    "LREM",
    "LTRIM",
    "LINSERT",
    "LMOVE",
    "BLPOP",
    "BRPOP",
    "SADD",
    "SREM",
    "SPOP",
    "SMOVE",
    "ZADD",
    "ZREM",
    "ZINCRBY",
    "ZPOPMIN",
    "ZPOPMAX",
    "XADD",
    "XDEL",
    "XTRIM",
    "XGROUP",
    "XREAD",
    "XREADGROUP",
    "XACK",
    "PFADD",
    "PFMERGE",
    "GEOADD",
    "FLUSHDB",
    "FLUSHALL",
    "SELECT",
    "CONFIG",
    "PING",
    "ECHO",
    "MULTI",
    "EXEC",
    "DISCARD",
    "WATCH",
    "UNWATCH",
    "PUBLISH",
    "SUBSCRIBE",
    "PSUBSCRIBE",
    "EVAL",
    "EVALSHA",
    "SCRIPT",
    "FUNCTION",
    "FCALL",
    "CLIENT",
    "MONITOR",
    "SAVE",
    "BGSAVE",
    "BGREWRITEAOF",
    "OBJECT",
    "MEMORY",
    "TIME",
    "LASTSAVE",
    "SLOWLOG",
    "COMMAND",
    "ACL",
    "AUTH",
    "DUMP",
    "RESTORE",
    "SORT",
    "WAIT",
    "SWAPDB",
];

fn known(cmd: &str) -> bool {
    let is = |list: &[&str]| list.iter().any(|c| c.eq_ignore_ascii_case(cmd));
    is(crate::agent::classify::REDIS_READ_COMMANDS) || is(OTHER_COMMANDS)
}

/// Periksa setiap baris (satu command per baris).
pub fn check(text: &str) -> Vec<LintMessage> {
    let mut out = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        check_line(line, offset, &mut out);
        offset += line.len();
    }
    out
}

fn check_line(line: &str, offset: usize, out: &mut Vec<LintMessage>) {
    let trimmed = line.trim_start();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("//")
        || trimmed.starts_with("--")
    {
        return;
    }
    // Kata beserta posisinya di teks.
    let mut parts: Vec<(usize, &str)> = Vec::new();
    let mut pos = 0;
    for word in line.split_whitespace() {
        let at = line[pos..].find(word).map_or(pos, |p| pos + p);
        parts.push((offset + at, word));
        pos = at + word.len();
    }
    let Some(&(cmd_at, cmd)) = parts.first() else {
        return;
    };
    let cmd_span = cmd_at..cmd_at + cmd.len();
    let line_span = cmd_at..offset + line.trim_end().len();
    let up = cmd.to_ascii_uppercase();
    if !SUPPORTED.contains(&up.as_str()) {
        let supported = SUPPORTED.join(", ");
        let mut m = if known(&up) {
            diag(
                LintSeverity::Error,
                DiagCode::UnsupportedCommand,
                DiagSource::Syntax,
                cmd_span,
                format!("{up} is not run by the query editor"),
            )
        } else {
            let mut m = diag(
                LintSeverity::Error,
                DiagCode::UnknownCommand,
                DiagSource::Syntax,
                cmd_span,
                format!("Unknown Redis command \"{cmd}\""),
            );
            let supported: Vec<String> = SUPPORTED.iter().map(|s| s.to_string()).collect();
            m.suggestions = words::suggest(&up, &supported, 1);
            m
        };
        m.hint = Some(format!(
            "Supported here: {supported}. Use the Redis browser in the sidebar for other commands."
        ));
        out.push(m);
        return;
    }
    let args = parts.len() - 1;
    let arity_error = |msg: &str| {
        let mut m = diag(
            LintSeverity::Error,
            DiagCode::WrongArgCount,
            DiagSource::Syntax,
            line_span.clone(),
            msg.to_string(),
        );
        m.hint = Some(usage(&up).to_string());
        m
    };
    match up.as_str() {
        "GET" if args != 1 => out.push(arity_error("GET takes exactly one key")),
        "HGETALL" if args != 1 => out.push(arity_error("HGETALL takes exactly one key")),
        "KEYS" if args != 1 => out.push(arity_error("KEYS takes exactly one pattern")),
        "INFO" if args > 1 => out.push(arity_error("INFO takes at most one section")),
        "SCAN" if args == 0 => out.push(arity_error("SCAN needs a cursor")),
        "SCAN" => {
            if parts[1].1.parse::<u64>().is_err() {
                let (at, w) = parts[1];
                let mut m = diag(
                    LintSeverity::Error,
                    DiagCode::WrongArgCount,
                    DiagSource::Syntax,
                    at..at + w.len(),
                    "SCAN cursor must be a number (start with 0)",
                );
                m.hint = Some(usage("SCAN").to_string());
                out.push(m);
                return;
            }
            let mut k = 2;
            while k < parts.len() {
                let (at, w) = parts[k];
                let opt = w.to_ascii_uppercase();
                let value = parts.get(k + 1);
                let problem = match opt.as_str() {
                    "MATCH" | "TYPE" if value.is_none() => Some(format!("{opt} needs a value")),
                    "COUNT" => match value {
                        None => Some("COUNT needs a number".to_string()),
                        Some((_, v)) if v.parse::<i64>().is_err() => {
                            Some("COUNT must be a number".to_string())
                        }
                        _ => None,
                    },
                    "MATCH" | "TYPE" => None,
                    _ => Some(format!("Unknown SCAN option \"{w}\"")),
                };
                if let Some(problem) = problem {
                    let mut m = diag(
                        LintSeverity::Error,
                        DiagCode::WrongArgCount,
                        DiagSource::Syntax,
                        at..at + w.len(),
                        problem,
                    );
                    m.hint = Some(usage("SCAN").to_string());
                    out.push(m);
                    return;
                }
                k += 2;
            }
        }
        _ => {}
    }
}

fn usage(cmd: &str) -> &'static str {
    match cmd {
        "GET" => "Usage: GET key",
        "HGETALL" => "Usage: HGETALL key",
        "KEYS" => "Usage: KEYS pattern",
        "INFO" => "Usage: INFO [section]",
        "SCAN" => "Usage: SCAN cursor [MATCH pattern] [COUNT n] [TYPE type]",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(text: &str) -> Vec<DiagCode> {
        check(text).into_iter().filter_map(|m| m.code).collect()
    }

    #[test]
    fn supported_commands_pass() {
        assert!(
            codes(
                "GET user:1\nKEYS user:*\nSCAN 0 MATCH a* COUNT 100\nINFO memory\nHGETALL h\n# note"
            )
            .is_empty()
        );
    }

    #[test]
    fn unknown_unsupported_and_arity() {
        let out = check("GTE user:1");
        assert_eq!(out[0].code, Some(DiagCode::UnknownCommand));
        assert_eq!(out[0].suggestions, vec!["GET".to_string()]);
        assert_eq!(codes("SET a 1"), vec![DiagCode::UnsupportedCommand]);
        assert_eq!(codes("GET a b"), vec![DiagCode::WrongArgCount]);
        assert_eq!(codes("SCAN 0 COUNT x"), vec![DiagCode::WrongArgCount]);
        assert_eq!(codes("SCAN abc"), vec![DiagCode::WrongArgCount]);
    }

    #[test]
    fn spans_are_absolute() {
        let text = "GET a\nGTE b";
        let out = check(text);
        assert_eq!(&text[out[0].span.clone().unwrap()], "GTE");
    }
}
