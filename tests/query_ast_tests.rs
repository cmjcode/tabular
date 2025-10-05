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
}
