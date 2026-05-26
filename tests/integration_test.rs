use dump_beacon::sanitize_sql_file;
use std::collections::HashMap;
use std::fs;
use tokio_util::sync::CancellationToken;

fn unique_temp_path(name: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("dump_beacon_test_{}_{}.sql", name, stamp))
        .display()
        .to_string()
}

#[tokio::test]
async fn test_small_test_fixture() {
    let input_path = "tests/fixtures/small_test.sql";
    let output_path = unique_temp_path("small_output");

    let cancel_token = CancellationToken::new();
    let result = sanitize_sql_file(
        input_path.to_string(),
        output_path.to_string(),
        None,
        cancel_token,
        None,
    )
    .await
    .unwrap();

    let output_sql = fs::read_to_string(&output_path).unwrap();

    let mut expected_schema = HashMap::new();
    expected_schema.insert(
        "users".to_string(),
        vec![
            "id".to_string(),
            "email".to_string(),
            "password".to_string(),
            "phone".to_string(),
        ],
    );
    assert_eq!(result.schema, expected_schema);

    // Assert first line masks
    assert!(output_sql.contains("user_masked@example.com"));
    // Assert multi-line masks
    assert!(output_sql.contains("[MASKED_PHONE]"));
    assert!(!output_sql.contains("test@example.com"));
    assert!(!output_sql.contains("secretpass"));
    assert!(!output_sql.contains("123-456-7890"));
    assert!(!output_sql.contains("multiline@example.com"));
    assert!(!output_sql.contains("anotherpass"));

    fs::remove_file(output_path).unwrap();
}

#[tokio::test]
async fn test_malformed_test_fixture() {
    let input_path = "tests/fixtures/malformed_test.sql";
    let output_path = unique_temp_path("malformed_output");

    let cancel_token = CancellationToken::new();
    let result = sanitize_sql_file(
        input_path.to_string(),
        output_path.to_string(),
        None,
        cancel_token,
        None,
    )
    .await
    .unwrap();

    // The malformed schema parser is actually resilient enough to find the `email` column
    // despite missing commas, so it SHOULD mask the email!
    let output_sql = fs::read_to_string(&output_path).unwrap();
    println!("MALFORMED OUTPUT SQL: {}", output_sql);
    assert!(output_sql.contains("user_masked@example.com"));

    fs::remove_file(output_path).unwrap();
}
