#[cfg(feature = "query_ast")]
mod query_ast_tests {
    use tabular::query_ast::compile_single_select;
    use tabular::models::enums::DatabaseType;

    #[test]
    fn simple_select_projection() {
        let sql = "select id, name from users";
        let (out_sql, headers) = compile_single_select(sql, &DatabaseType::MySQL, None, true).expect("ok");
        assert!(out_sql.to_lowercase().contains("select"));
        assert!(out_sql.to_lowercase().contains("from"));
        assert!(headers.iter().any(|h| h.eq_ignore_ascii_case("id")));
        assert!(headers.iter().any(|h| h.eq_ignore_ascii_case("name")));
    }

    #[test]
    fn auto_limit_injection() {
        let sql = "select * from products order by created_at desc";
        let (out_sql, _headers) = compile_single_select(sql, &DatabaseType::PostgreSQL, Some((1,50)), true).expect("ok");
        assert!(out_sql.to_lowercase().contains("limit"));
    }

    #[test]
    fn aggregate_auto_alias() {
        let sql = "select count(*) from orders";
        let (_out, headers) = compile_single_select(sql, &DatabaseType::MySQL, None, true).expect("ok");
        // Expect auto alias like count_col or count
        assert!(headers.iter().any(|h| h.starts_with("count")));
    }

    #[test]
    fn distinct_with_limit() {
        let sql = "select distinct user_id from sessions";
        let (out, _h) = compile_single_select(sql, &DatabaseType::SQLite, Some((0,10)), true).expect("ok");
        let lo = out.to_lowercase();
        assert!(lo.contains("select distinct"));
        assert!(lo.contains("limit"));
    }

    #[test]
    fn group_by_having_order_limit_sequence() {
        let sql = "select user_id, count(*) from logins where status = 'OK' group by user_id having count(*) > 2 order by user_id desc limit 5";
        let (out, headers) = compile_single_select(sql, &DatabaseType::PostgreSQL, None, true).expect("ok");
    let lo = out.to_lowercase();
    eprintln!("EMITTED_SQL={}", lo);
        // Ensure clause ordering: select .. from .. where .. group by .. having .. order by .. limit ..
        let where_pos = lo.find(" where ").unwrap();
        let group_pos = lo.find(" group by ").unwrap();
        let having_pos = lo.find(" having ").unwrap();
        let order_pos = lo.find(" order by ").unwrap();
        assert!(where_pos < group_pos && group_pos < having_pos && having_pos < order_pos);
        assert!(lo.ends_with(" limit 5") || lo.contains(" limit 5"));
        // Headers should include user_id and an aggregate alias
        assert!(headers.iter().any(|h| h.eq("user_id")));
        assert!(headers.iter().any(|h| h.starts_with("count")));
    }

    #[test]
    fn join_group_distinct_having_combo() {
        let sql = "select distinct a.user_id, count(b.id) from accounts a left join sessions b on a.user_id = b.user_id where a.active = true group by a.user_id having count(b.id) > 1 order by a.user_id limit 20";
        let (out, headers) = compile_single_select(sql, &DatabaseType::MySQL, None, true).expect("ok");
        let lo = out.to_lowercase();
        assert!(lo.starts_with("select distinct"));
        assert!(lo.contains(" left join "));
        assert!(lo.contains(" group by "));
        assert!(lo.contains(" having "));
        assert!(lo.contains(" order by "));
        assert!(lo.contains(" limit 20"));
        assert!(headers.iter().any(|h| h.eq("user_id")));
    }

    #[test]
    fn subquery_in_where_fallback() {
        let sql = "select id from users where id in (select user_id from logins where success = true) limit 3";
        let (out, _h) = compile_single_select(sql, &DatabaseType::SQLite, None, true).expect("ok");
        assert!(out.to_lowercase().contains("limit 3"));
    }

    #[test]
    fn subquery_in_from_supported() {
        let sql = "select t.user_id from (select user_id, count(*) c from logs group by user_id) t limit 10";
        let (out, _h) = compile_single_select(sql, &DatabaseType::PostgreSQL, None, true).expect("ok");
        assert!(out.to_lowercase().contains("from (select"));
    }

    #[test]
    fn debug_plan_basic() {
        let sql = "select id from users limit 1";
        let dbg = tabular::query_ast::debug_plan(sql, &DatabaseType::MySQL).expect("debug");
        assert!(dbg.contains("TableScan"));
    }

    #[test]
    fn cte_pass_through() {
        let sql = "WITH recent AS (select id, created_at from orders where created_at > now() - interval '7 days') select id from recent limit 10";
        let (out, _h) = compile_single_select(sql, &DatabaseType::PostgreSQL, None, true).expect("ok");
        let lo = out.to_lowercase();
        assert!(lo.trim_start().starts_with("with "));
        assert!(lo.contains("limit 10"));
    }

    #[test]
    fn cache_hits_increment() {
        // Warm cache
        let sql = "select id, name from users limit 5";
        let _ = compile_single_select(sql, &DatabaseType::MySQL, None, true).unwrap();
        let (h1, m1) = tabular::query_ast::cache_stats();
        let _ = compile_single_select(sql, &DatabaseType::MySQL, None, true).unwrap();
        let (h2, m2) = tabular::query_ast::cache_stats();
    assert!(h2 > h1, "expected hit counter to increase");
        assert!(m2 == m1);
    }

    #[test]
    fn cte_inlining_basic() {
        let sql = "WITH cte AS (select id from users) select * from cte limit 5";
        let (out, _h) = compile_single_select(sql, &DatabaseType::MySQL, None, true).expect("ok");
        let lo = out.to_lowercase();
        assert!(!lo.trim_start().starts_with("with "), "expected CTE to be inlined, got {lo}");
        assert!(lo.contains("(select id from users)"), "expected subquery present: {lo}");
        assert!(lo.contains("limit 5"));
    }

    #[test]
    fn canonical_cache_fingerprint_hits() {
        // Use a query pattern unlikely used elsewhere to isolate stats
        let q1 = "SELECT   id  FROM   users   LIMIT 7";
        let _ = compile_single_select(q1, &DatabaseType::PostgreSQL, None, true).unwrap();
        let (h1, m1) = tabular::query_ast::cache_stats();
        let q2 = "select id from users limit 7"; // structurally identical
        let _ = compile_single_select(q2, &DatabaseType::PostgreSQL, None, true).unwrap();
        let (h2, m2) = tabular::query_ast::cache_stats();
        assert!(h2 > h1, "expected cache hit to increase (h1={h1}, h2={h2}) m1={m1} m2={m2}");
    }
}
