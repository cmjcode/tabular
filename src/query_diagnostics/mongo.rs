//! Diagnostik query MongoDB berformat JSON
//! (`{"collection": "orders", "filter": {...}}`, lihat
//! `data_transfer::mongo_source::parse_mongo_query`): JSON tidak valid,
//! field query yang salah, operator `$...` yang tidak dikenal, dan collection
//! yang tidak ada.

use std::ops::Range;

use super::words;
use super::{DiagCode, DiagSource, SchemaSnapshot, diag};
use crate::query_tools::{LintMessage, LintSeverity};

/// Operator query, stage, operator ekspresi/akumulator, dan Extended JSON.
const OPERATORS: &[&str] = &[
    // query & update
    "$eq",
    "$ne",
    "$gt",
    "$gte",
    "$lt",
    "$lte",
    "$in",
    "$nin",
    "$and",
    "$or",
    "$not",
    "$nor",
    "$exists",
    "$type",
    "$expr",
    "$jsonSchema",
    "$mod",
    "$regex",
    "$options",
    "$text",
    "$search",
    "$language",
    "$caseSensitive",
    "$diacriticSensitive",
    "$where",
    "$geoIntersects",
    "$geoWithin",
    "$near",
    "$nearSphere",
    "$box",
    "$center",
    "$centerSphere",
    "$geometry",
    "$maxDistance",
    "$minDistance",
    "$polygon",
    "$all",
    "$elemMatch",
    "$size",
    "$bitsAllClear",
    "$bitsAllSet",
    "$bitsAnyClear",
    "$bitsAnySet",
    "$comment",
    "$meta",
    "$slice",
    "$natural",
    "$hint",
    "$inc",
    "$mul",
    "$rename",
    "$setOnInsert",
    "$currentDate",
    "$pop",
    "$pull",
    "$pullAll",
    "$each",
    "$position",
    "$sort",
    "$set",
    "$unset",
    "$push",
    "$addToSet",
    "$min",
    "$max",
    // Extended JSON
    "$oid",
    "$date",
    "$numberLong",
    "$numberInt",
    "$numberDouble",
    "$numberDecimal",
    "$binary",
    "$base64",
    "$subType",
    "$timestamp",
    "$regularExpression",
    "$pattern",
    "$symbol",
    "$code",
    "$scope",
    "$minKey",
    "$maxKey",
    "$undefined",
    "$dbPointer",
    "$uuid",
    "$t",
    "$i",
    // stage agregasi
    "$addFields",
    "$bucket",
    "$bucketAuto",
    "$changeStream",
    "$collStats",
    "$count",
    "$densify",
    "$documents",
    "$facet",
    "$fill",
    "$geoNear",
    "$graphLookup",
    "$group",
    "$indexStats",
    "$limit",
    "$listSessions",
    "$lookup",
    "$match",
    "$merge",
    "$out",
    "$planCacheStats",
    "$project",
    "$redact",
    "$replaceRoot",
    "$replaceWith",
    "$sample",
    "$searchMeta",
    "$setWindowFields",
    "$skip",
    "$sortByCount",
    "$unionWith",
    "$unwind",
    "$vectorSearch",
    // akumulator & ekspresi
    "$sum",
    "$avg",
    "$first",
    "$last",
    "$stdDevPop",
    "$stdDevSamp",
    "$mergeObjects",
    "$top",
    "$bottom",
    "$topN",
    "$bottomN",
    "$firstN",
    "$lastN",
    "$maxN",
    "$minN",
    "$median",
    "$percentile",
    "$abs",
    "$add",
    "$ceil",
    "$divide",
    "$exp",
    "$floor",
    "$ln",
    "$log",
    "$log10",
    "$multiply",
    "$pow",
    "$round",
    "$sqrt",
    "$subtract",
    "$trunc",
    "$arrayElemAt",
    "$arrayToObject",
    "$concatArrays",
    "$filter",
    "$indexOfArray",
    "$isArray",
    "$map",
    "$objectToArray",
    "$range",
    "$reduce",
    "$reverseArray",
    "$zip",
    "$cmp",
    "$cond",
    "$ifNull",
    "$switch",
    "$branches",
    "$case",
    "$then",
    "$default",
    "$dateAdd",
    "$dateDiff",
    "$dateFromParts",
    "$dateFromString",
    "$dateSubtract",
    "$dateToParts",
    "$dateToString",
    "$dateTrunc",
    "$dayOfMonth",
    "$dayOfWeek",
    "$dayOfYear",
    "$hour",
    "$isoDayOfWeek",
    "$isoWeek",
    "$isoWeekYear",
    "$millisecond",
    "$minute",
    "$month",
    "$second",
    "$week",
    "$year",
    "$literal",
    "$concat",
    "$indexOfBytes",
    "$indexOfCP",
    "$ltrim",
    "$rtrim",
    "$trim",
    "$split",
    "$strLenBytes",
    "$strLenCP",
    "$strcasecmp",
    "$substr",
    "$substrBytes",
    "$substrCP",
    "$toLower",
    "$toUpper",
    "$regexFind",
    "$regexFindAll",
    "$regexMatch",
    "$replaceOne",
    "$replaceAll",
    "$toBool",
    "$toDate",
    "$toDecimal",
    "$toDouble",
    "$toInt",
    "$toLong",
    "$toObjectId",
    "$toString",
    "$toUUID",
    "$convert",
    "$setField",
    "$getField",
    "$let",
    "$rand",
    "$allElementsTrue",
    "$anyElementTrue",
    "$setDifference",
    "$setEquals",
    "$setIntersection",
    "$setIsSubset",
    "$setUnion",
    "$binarySize",
    "$bsonSize",
    "$function",
    "$accumulator",
    "$sortArray",
    "$documentNumber",
    "$rank",
    "$denseRank",
    "$shift",
    "$derivative",
    "$integral",
    "$expMovingAvg",
    "$covariancePop",
    "$covarianceSamp",
    "$locf",
    "$linearFill",
    "$tsIncrement",
    "$tsSecond",
    "$sin",
    "$cos",
    "$tan",
    "$asin",
    "$acos",
    "$atan",
    "$atan2",
    "$asinh",
    "$acosh",
    "$atanh",
    "$sinh",
    "$cosh",
    "$tanh",
    "$degreesToRadians",
    "$radiansToDegrees",
    "$isNumber",
    "$sampleRate",
    "$input",
    "$as",
    "$in",
    "$vars",
    "$initialValue",
    "$cond",
];

/// Posisi byte setiap key objek JSON (`"key"` diikuti `:`), dengan kedalamannya.
fn object_keys(text: &str) -> Vec<(String, Range<usize>, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                let end = (i + 1).min(bytes.len());
                let mut k = end;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b':' && end > start + 1 {
                    let key = text[start + 1..end - 1].to_string();
                    out.push((key, start..end, depth));
                }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Rentang nilai string setelah key `key` di level teratas.
fn top_level_value_span(text: &str, key: &str) -> Option<Range<usize>> {
    let (_, span, _) = object_keys(text)
        .into_iter()
        .find(|(k, _, d)| k == key && *d == 1)?;
    let rest = &text[span.end..];
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    let lead = after.len() - after.trim_start().len();
    let value_start = span.end + colon + 1 + lead;
    if !text[value_start..].starts_with('"') {
        return None;
    }
    let close = text[value_start + 1..].find('"')?;
    Some(value_start..value_start + close + 2)
}

pub fn check(text: &str, schema: &SchemaSnapshot) -> Vec<LintMessage> {
    let mut out = Vec::new();
    let trimmed = text.trim();
    if trimmed.is_empty() || super::has_template_vars(text) {
        return out;
    }
    if let Err(e) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let lead = text.len() - text.trim_start().len();
        let pos = super::syntax::line_col_to_offset(trimmed, e.line(), e.column().max(1))
            .map_or(text.len(), |p| (lead + p).min(text.len()));
        // Tandai karakter di posisi error (atau karakter terakhir bila di akhir).
        let start = if pos >= text.len() {
            text.len().saturating_sub(1)
        } else {
            pos
        };
        let start = (0..=start)
            .rev()
            .find(|p| text.is_char_boundary(*p))
            .unwrap_or(0);
        let end = text[start..]
            .chars()
            .next()
            .map_or(start, |c| start + c.len_utf8());
        let mut m = diag(
            LintSeverity::Error,
            DiagCode::InvalidJson,
            DiagSource::Syntax,
            start..end,
            format!("Invalid JSON: {e}"),
        );
        m.hint = Some(
            "MongoDB queries are JSON, e.g. {\"collection\": \"orders\", \"filter\": {}}."
                .to_string(),
        );
        out.push(m);
        return out;
    }
    let keys = object_keys(text);
    for (key, span, _) in &keys {
        if !key.starts_with('$') || OPERATORS.contains(&key.as_str()) {
            continue;
        }
        let candidates: Vec<String> = OPERATORS.iter().map(|s| s.to_string()).collect();
        let suggestions = words::suggest(key, &candidates, 3);
        let severity = if suggestions.is_empty() {
            LintSeverity::Warning
        } else {
            LintSeverity::Error
        };
        let mut m = diag(
            severity,
            DiagCode::UnknownOperator,
            DiagSource::Syntax,
            span.clone(),
            format!("Unknown MongoDB operator \"{key}\""),
        );
        m.hint = suggestions.first().map(|s| format!("Did you mean {s}?"));
        m.suggestions = suggestions
            .into_iter()
            .map(|s| format!("\"{s}\""))
            .collect();
        out.push(m);
    }
    match crate::data_transfer::mongo_source::parse_mongo_query(text) {
        Ok(q) => {
            if let Some(collections) = &schema.collections
                && !collections.contains(&q.collection)
                && let Some(span) = top_level_value_span(text, "collection")
            {
                let suggestions = words::suggest(&q.collection, collections, 3);
                let mut m = diag(
                    LintSeverity::Error,
                    DiagCode::UnknownCollection,
                    DiagSource::Schema,
                    span,
                    format!("Collection \"{}\" does not exist", q.collection),
                );
                m.hint = suggestions.first().map(|s| format!("Did you mean {s}?"));
                m.suggestions = suggestions
                    .into_iter()
                    .map(|s| format!("\"{s}\""))
                    .collect();
                out.push(m);
            }
        }
        Err(message) => {
            // Field top-level yang disebut pesan, kalau ada; selain itu awal query.
            let named = keys
                .iter()
                .filter(|(_, _, d)| *d == 1)
                .find(|(k, _, _)| message.contains(&format!("\"{k}\"")))
                .map(|(_, s, _)| s.clone());
            let lead = text.len() - text.trim_start().len();
            let span = named.unwrap_or(lead..lead + 1);
            let mut m = diag(
                LintSeverity::Error,
                DiagCode::InvalidQuery,
                DiagSource::Syntax,
                span,
                message,
            );
            m.hint = Some(
                "Fields: collection, filter, projection, sort, limit, or pipeline.".to_string(),
            );
            out.push(m);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Vec<LintMessage> {
        let schema = SchemaSnapshot {
            collections: Some(vec!["orders".into(), "users".into()]),
            ..Default::default()
        };
        check(text, &schema)
    }

    #[test]
    fn valid_query_passes() {
        assert!(
            run(r#"{"collection": "orders", "filter": {"total": {"$gt": 10}}, "limit": 5}"#)
                .is_empty()
        );
        assert!(run(r#"{"collection": "orders", "pipeline": [{"$match": {"a": 1}}, {"$group": {"_id": "$a", "n": {"$sum": 1}}}]}"#).is_empty());
    }

    #[test]
    fn json_errors_have_a_position() {
        let text = "{\"collection\": \"orders\",\n \"filter\": {\"a\": 1,}}";
        let out = run(text);
        assert_eq!(out[0].code, Some(DiagCode::InvalidJson));
        assert_eq!(&text[out[0].span.clone().unwrap()], "}");
    }

    #[test]
    fn operators_fields_and_collections() {
        let text = r#"{"collection": "orders", "filter": {"total": {"$gtt": 10}}}"#;
        let out = run(text);
        assert_eq!(out[0].code, Some(DiagCode::UnknownOperator));
        assert_eq!(&text[out[0].span.clone().unwrap()], "\"$gtt\"");
        let text = r#"{"collection": "ordres", "filter": {}}"#;
        let out = run(text);
        assert_eq!(out[0].code, Some(DiagCode::UnknownCollection));
        assert_eq!(&text[out[0].span.clone().unwrap()], "\"ordres\"");
        let text = r#"{"collection": "orders", "filtr": {}}"#;
        let out = run(text);
        assert_eq!(out[0].code, Some(DiagCode::InvalidQuery));
        assert_eq!(&text[out[0].span.clone().unwrap()], "\"filtr\"");
    }
}
