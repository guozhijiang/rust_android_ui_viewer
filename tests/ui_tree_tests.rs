//! Unit tests for `src/ui_tree.rs` (XML parsing & hit-testing).
//!
//! Node ids are assigned in document order starting at 0, so for `SAMPLE`:
//!   0 = <hierarchy>, 1 = FrameLayout(root), 2 = title, 3 = button.

use android_ui_viewer::ui_tree::{parse, Bounds};

const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<hierarchy rotation="0">
  <node index="0" text="" resource-id="com.example:id/root" class="android.widget.FrameLayout" bounds="[0,0][100,200]">
    <node index="0" text="Hello" resource-id="com.example:id/title" class="android.widget.TextView" bounds="[10,10][90,40]"/>
    <node index="1" text="" resource-id="com.example:id/btn" class="android.widget.Button" bounds="[10,50][90,90]"/>
  </node>
</hierarchy>"#;

#[test]
fn parses_nodes_attrs_and_bounds() {
    let tree = parse(SAMPLE).expect("parse ok");
    // id 1 is the FrameLayout carrying the resource-id and bounds.
    let root = tree.find(1).unwrap();
    assert_eq!(root.attrs.get("resource-id").unwrap(), "com.example:id/root");
    let b = root.bounds.unwrap();
    assert_eq!((b.left, b.top, b.right, b.bottom), (0, 0, 100, 200));
}

#[test]
fn empty_attributes_are_stored_as_empty_strings() {
    // id 1 (FrameLayout) has text="".
    let tree = parse(SAMPLE).unwrap();
    let frame = tree.find(1).unwrap();
    assert_eq!(frame.attrs.get("text").unwrap(), "");
}

#[test]
fn bounds_width_height() {
    let b = Bounds { left: 10, top: 20, right: 110, bottom: 70 };
    assert_eq!(b.width(), 100);
    assert_eq!(b.height(), 50);
}

#[test]
fn bounds_contains_edge() {
    let b = Bounds { left: 0, top: 0, right: 100, bottom: 200 };
    assert!(b.contains(0, 0));
    assert!(b.contains(100, 200));
    assert!(!b.contains(101, 0));
    assert!(!b.contains(0, 201));
}

#[test]
fn hit_test_picks_innermost_node() {
    let tree = parse(SAMPLE).unwrap();
    // (20,20) is inside the title (id 2, bounds [10,10][90,40]).
    let id = tree.hit_test(20, 20).expect("hit");
    assert_eq!(id, 2, "smallest-area node containing the point wins");
    // A point inside the FrameLayout but outside its children resolves to id 1.
    let id2 = tree.hit_test(95, 150).expect("hit root");
    assert_eq!(id2, 1);
}

#[test]
fn hit_test_outside_returns_none() {
    let tree = parse(SAMPLE).unwrap();
    assert!(tree.hit_test(500, 500).is_none());
}

#[test]
fn find_returns_same_node_as_hit_test() {
    let tree = parse(SAMPLE).unwrap();
    let id = tree.hit_test(20, 20).unwrap();
    let by_find = tree.find(id).expect("find by id");
    assert_eq!(by_find.attrs.get("resource-id").unwrap(), "com.example:id/title");
}

#[test]
fn find_missing_returns_none() {
    let tree = parse(SAMPLE).unwrap();
    assert!(tree.find(999).is_none());
}

#[test]
fn node_count_total_subtree() {
    let tree = parse(SAMPLE).unwrap();
    // hierarchy + FrameLayout + title + button = 4
    assert_eq!(tree.count(), 4);
}

#[test]
fn subtree_matches_own_attribute() {
    let tree = parse(SAMPLE).unwrap();
    assert!(tree.subtree_matches("Hello"));
    assert!(tree.subtree_matches("button")); // case-insensitive
}

#[test]
fn subtree_matches_descendant() {
    let tree = parse(SAMPLE).unwrap();
    // id 3 is the button.
    let btn = tree.find(3).unwrap();
    assert!(btn.subtree_matches("btn"));
    // id 2 is the title.
    let title = tree.find(2).unwrap();
    assert!(title.subtree_matches("Hello"));
    // The hierarchy (id 0) contains the button in its subtree.
    let root = tree.find(0).unwrap();
    assert!(root.subtree_matches("btn"));
}

#[test]
fn subtree_matches_none() {
    let tree = parse(SAMPLE).unwrap();
    assert!(!tree.subtree_matches("zzznotpresent"));
}

#[test]
fn parse_bounds_invalid_yields_none() {
    let tree = parse(r#"<hierarchy><node bounds="garbage"/></hierarchy>"#).unwrap();
    let n = tree.find(1).unwrap();
    assert!(n.bounds.is_none());
}

#[test]
fn parse_decode_entity() {
    // `&amp;` should be unescaped into `&`.
    let tree = parse(r#"<hierarchy><node text="a&amp;b"/></hierarchy>"#).unwrap();
    let n = tree.find(1).unwrap();
    assert_eq!(n.attrs.get("text").unwrap(), "a&b");
}

#[test]
fn parse_error_no_root() {
    assert!(parse("").is_err());
    assert!(parse("not xml at all").is_err());
}

#[test]
fn parse_error_extra_end_tag() {
    // An end tag with an empty stack is a structural error.
    let r = parse(r#"<hierarchy><node/></hierarchy></hierarchy>"#);
    assert!(r.is_err());
}

#[test]
fn parse_nested_start_end_branches() {
    // Exercise the Start(node w/ children) + End(pop to parent) code paths.
    let xml = r#"<hierarchy><node bounds="[0,0][10,10]"><node bounds="[1,1][5,5]"/></node></hierarchy>"#;
    let tree = parse(xml).unwrap();
    assert_eq!(tree.count(), 3);
    assert!(tree.hit_test(2, 2).is_some());
}
