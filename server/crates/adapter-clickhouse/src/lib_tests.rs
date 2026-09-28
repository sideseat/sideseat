use super::*;

#[test]
fn test_clickhouse_error_types() {
    let err = ClickhouseError::Connection("test".to_string());
    assert!(err.to_string().contains("test"));
}
