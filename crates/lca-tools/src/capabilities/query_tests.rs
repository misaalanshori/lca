//! Split from `capabilities.rs` (cycle 7, P3).

use super::parse_query;

#[test]
fn query_parsing_decodes_and_keeps_empty_values() {
    let params = parse_query("code=abc%20def&state=x+y&flag");
    assert_eq!(
        params,
        vec![
            ("code".to_string(), "abc def".to_string()),
            ("state".to_string(), "x y".to_string()),
            ("flag".to_string(), String::new()),
        ]
    );
}
