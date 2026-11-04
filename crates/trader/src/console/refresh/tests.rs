use super::*;

/// Cursors order by seq, then by index, and only `<seq>:<index>` is one.
#[test]
fn cursors_order_by_seq_then_index() {
    assert_eq!(cursor("12:3"), Some((12, 3)));
    assert!(cursor("12:3") < cursor("12:4") && cursor("12:4") < cursor("13:0"));
    for bad in ["", "12", "12:", ":3", "a:1", "1:b", "1:2:3", "-1:0", "1:2&limit=1"] {
        assert_eq!(cursor(bad), None, "{bad}");
    }
}
