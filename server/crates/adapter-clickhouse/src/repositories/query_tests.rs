use super::*;

#[test]
fn test_list_traces_params_default() {
    let params = ListTracesParams::default();
    assert_eq!(params.page, 0);
    assert_eq!(params.limit, 0);
}
