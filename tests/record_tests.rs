//! Unit tests for `src/record.rs` (selectors, replay, serialization).

use android_ui_viewer::record::{
    build_selector, find_center, load_yaml, node_matches, replay, resolve, save_yaml, screen_size,
    ReplayMsg, ReplayOpts, RecordStep, UiSelector,
};
use android_ui_viewer::ui_tree::{parse, Node};

fn node(xml: &str) -> Node {
    parse(xml).unwrap()
}

#[test]
fn record_step_new_defaults() {
    let s = RecordStep::new("tap");
    assert_eq!(s.action, "tap");
    assert!(s.selector.is_none());
    assert!(s.text.is_none());
    assert_eq!(s.ts, 0.0);
}

#[test]
fn describe_all_actions() {
    let sel = UiSelector {
        resource_id: Some("com.x:id/a".into()),
        text: Some("hi".into()),
        content_desc: Some("d".into()),
        class: Some("android.widget.Button".into()),
    };
    let s = |action: &str, selector: Option<UiSelector>| {
        let mut st = RecordStep::new(action);
        st.selector = selector;
        st
    };
    // class is intentionally omitted from the human-readable summary.
    assert_eq!(s("tap", None).describe(), "点击 []");
    assert_eq!(
        s("tap", Some(sel.clone())).describe(),
        "点击 [id=com.x:id/a,text=hi,desc=d]"
    );
    assert_eq!(
        s("long_tap", Some(sel.clone())).describe(),
        "长按 [id=com.x:id/a,text=hi,desc=d]"
    );
    let mut sw = RecordStep::new("swipe");
    sw.from_selector = Some(sel.clone());
    sw.to_selector = Some(sel.clone());
    assert_eq!(sw.describe(), "滑动 id=com.x:id/a,text=hi,desc=d→id=com.x:id/a,text=hi,desc=d");
    let mut tx = RecordStep::new("text");
    tx.text = Some("abc".into());
    assert_eq!(tx.describe(), "输入文本 \"abc\"");
    let mut ky = RecordStep::new("key");
    ky.key = Some("HOME".into());
    assert_eq!(ky.describe(), "按键 HOME");
    assert_eq!(RecordStep::new("weird").describe(), "weird");
}

#[test]
fn build_selector_keeps_identifying_attrs() {
    let n = node(r#"<hierarchy><node resource-id="a" text="b" content-desc="c" class="d"/></hierarchy>"#);
    let sel = build_selector(n.find(1).unwrap());
    assert_eq!(sel.resource_id.as_deref(), Some("a"));
    assert_eq!(sel.text.as_deref(), Some("b"));
    assert_eq!(sel.content_desc.as_deref(), Some("c"));
    assert_eq!(sel.class.as_deref(), Some("d"));
}

#[test]
fn build_selector_drops_empty_attrs() {
    let n = node(r#"<hierarchy><node text="" class="android.widget.FrameLayout"/></hierarchy>"#);
    let sel = build_selector(n.find(1).unwrap());
    assert!(sel.text.is_none());
    assert_eq!(sel.class.as_deref(), Some("android.widget.FrameLayout"));
    assert!(sel.resource_id.is_none());
    assert!(sel.content_desc.is_none());
}

#[test]
fn node_matches_empty_selector_matches_all() {
    let t = node(r#"<hierarchy><node resource-id="a" text="b"/></hierarchy>"#);
    let n = t.find(1).unwrap();
    assert!(node_matches(n, &UiSelector::default()));
}

#[test]
fn node_matches_by_each_field() {
    let xml = r#"<hierarchy><node resource-id="a" text="b" content-desc="c" class="d"/></hierarchy>"#;
    let t = node(xml);
    let n = t.find(1).unwrap();
    assert!(node_matches(n, &UiSelector { resource_id: Some("a".into()), ..Default::default() }));
    assert!(node_matches(n, &UiSelector { text: Some("b".into()), ..Default::default() }));
    assert!(node_matches(n, &UiSelector { content_desc: Some("c".into()), ..Default::default() }));
    assert!(node_matches(n, &UiSelector { class: Some("d".into()), ..Default::default() }));
    // mismatch in any single field => no match
    assert!(!node_matches(n, &UiSelector { resource_id: Some("WRONG".into()), ..Default::default() }));
    assert!(!node_matches(n, &UiSelector { text: Some("WRONG".into()), ..Default::default() }));
    assert!(!node_matches(n, &UiSelector { content_desc: Some("WRONG".into()), ..Default::default() }));
    assert!(!node_matches(n, &UiSelector { class: Some("WRONG".into()), ..Default::default() }));
}

#[test]
fn node_matches_combined_fields() {
    let t = node(r#"<hierarchy><node resource-id="a" text="b" class="d"/></hierarchy>"#);
    let n = t.find(1).unwrap();
    let sel = UiSelector { resource_id: Some("a".into()), text: Some("b".into()), class: Some("d".into()), ..Default::default() };
    assert!(node_matches(n, &sel));
    let wrong = UiSelector { resource_id: Some("a".into()), text: Some("WRONG".into()), ..Default::default() };
    assert!(!node_matches(n, &wrong));
}

#[test]
fn find_center_picks_smallest_matching() {
    // Two matching nodes; the smaller one (inner) should win.
    let xml = r#"<hierarchy>
      <node resource-id="x" bounds="[0,0][100,100]">
        <node resource-id="x" bounds="[10,10][20,20]"/>
      </node>
    </hierarchy>"#;
    let tree = node(xml);
    let sel = UiSelector { resource_id: Some("x".into()), ..Default::default() };
    let (cx, cy) = find_center(&tree, &sel).unwrap();
    assert_eq!((cx, cy), (15, 15));
}

#[test]
fn find_center_none_when_no_match() {
    let tree = node(r#"<hierarchy><node resource-id="a" bounds="[0,0][10,10]"/></hierarchy>"#);
    let sel = UiSelector { resource_id: Some("nope".into()), ..Default::default() };
    assert!(find_center(&tree, &sel).is_none());
}

#[test]
fn find_center_skips_nodes_without_bounds() {
    let tree = node(r#"<hierarchy><node resource-id="a"/></hierarchy>"#);
    let sel = UiSelector { resource_id: Some("a".into()), ..Default::default() };
    assert!(find_center(&tree, &sel).is_none());
}

#[test]
fn resolve_without_selector_uses_fractional_coords() {
    let no_sel: Option<UiSelector> = None;
    let (pt, ok) = resolve("adb", "", None, &no_sel, 0.5, 0.25, (200, 400), 1);
    assert!(ok);
    assert_eq!(pt, Some((100, 100)));
}

#[test]
fn resolve_with_selector_falls_back_when_unreachable() {
    // No device: fetch fails repeatedly, so we fall back to fractional coords
    // and report `ok = false` so the UI can flag the unresolved step.
    let sel = UiSelector { resource_id: Some("x".into()), ..Default::default() };
    let some_sel = Some(sel);
    let (pt, ok) = resolve(
        "nonexistent_adb_xyz",
        "",
        None,
        &some_sel,
        0.25,
        0.5,
        (200, 400),
        12,
    );
    assert!(!ok);
    assert_eq!(pt, Some((50, 200)));
}

#[test]
fn screen_size_falls_back_without_device() {
    assert_eq!(screen_size("nonexistent_adb_xyz", ""), (1080, 1920));
}

#[test]
fn yaml_round_trip() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("rec_rt_{}.yaml", std::process::id()));
    let steps = vec![
        RecordStep::new("tap"),
        RecordStep::new("text"),
    ];
    save_yaml(&path, &steps).unwrap();
    let loaded = load_yaml(&path).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].action, "tap");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn yaml_preserves_all_fields() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("rec_full_{}.yaml", std::process::id()));
    let mut s = RecordStep::new("swipe");
    s.from_selector = Some(UiSelector { resource_id: Some("a".into()), ..Default::default() });
    s.to_selector = Some(UiSelector { text: Some("b".into()), ..Default::default() });
    s.from_fx = Some(0.1);
    s.from_fy = Some(0.2);
    s.to_fx = Some(0.3);
    s.to_fy = Some(0.4);
    s.text = Some("hi".into());
    s.keycode = Some(3);
    s.key = Some("HOME".into());
    s.app = Some("pkg".into());
    s.activity = Some(".Main".into());
    s.ts = 1.5;
    save_yaml(&path, &[s.clone()]).unwrap();
    let loaded = load_yaml(&path).unwrap();
    assert_eq!(loaded.len(), 1);
    let l = &loaded[0];
    assert_eq!(l.action, "swipe");
    assert_eq!(l.from_selector.as_ref().unwrap().resource_id.as_deref(), Some("a"));
    assert_eq!(l.to_selector.as_ref().unwrap().text.as_deref(), Some("b"));
    assert_eq!(l.from_fx, Some(0.1));
    assert_eq!(l.to_fy, Some(0.4));
    assert_eq!(l.text.as_deref(), Some("hi"));
    assert_eq!(l.keycode, Some(3));
    assert_eq!(l.key.as_deref(), Some("HOME"));
    assert_eq!(l.app.as_deref(), Some("pkg"));
    assert_eq!(l.activity.as_deref(), Some(".Main"));
    assert_eq!(l.ts, 1.5);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn yaml_skip_none_fields() {
    let s = serde_yaml::to_string(&RecordStep::new("tap")).unwrap();
    assert!(!s.contains("selector"));
    assert!(!s.contains("text"));
}

#[test]
fn yaml_from_web_output_parses() {
    // Interop: this is byte-for-byte the shape the web app's
    // /api/save-recording-yaml emits (snake_case, same field names), including
    // web-only extras ("scroll" action, unknown fields) that serde must ignore.
    let web_yaml = "\
- action: tap
  selector:
    resource_id: com.example:id/btn_ok
    class: android.widget.Button
  fx: 0.501
  fy: 0.733
  app: com.example.app
  activity: com.example.app.MainActivity
  ts: 3.55
- action: scroll
  from_fx: 0.5
  from_fy: 0.7
  to_fx: 0.5
  to_fy: 0.3
  ts: 8.1
- action: text
  text: 'hello 阿布'
  ts: 11.0
";
    let dir = std::env::temp_dir();
    let path = dir.join(format!("rec_web_{}.yaml", std::process::id()));
    std::fs::write(&path, web_yaml).unwrap();
    let loaded = load_yaml(&path).unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].action, "tap");
    assert_eq!(
        loaded[0].selector.as_ref().unwrap().resource_id.as_deref(),
        Some("com.example:id/btn_ok")
    );
    assert_eq!(loaded[0].fx, Some(0.501));
    assert_eq!(loaded[0].app.as_deref(), Some("com.example.app"));
    assert_eq!(loaded[1].action, "scroll"); // web-only action still deserializes
    assert_eq!(loaded[1].from_fy, Some(0.7));
    assert_eq!(loaded[2].text.as_deref(), Some("hello 阿布"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn load_yaml_rejects_garbage() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("rec_bad_{}.yaml", std::process::id()));
    std::fs::write(&path, "this: is not a recording list :::").unwrap();
    assert!(load_yaml(&path).is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn replay_runs_all_actions_and_reports() {
    let steps = vec![
        {
            let mut s = RecordStep::new("key");
            s.keycode = Some(3);
            s
        },
        {
            let mut s = RecordStep::new("tap");
            s.ts = 0.01; // tiny gap to also exercise the inter-step wait branch
            s.selector = Some(UiSelector { resource_id: Some("x".into()), ..Default::default() });
            s
        },
        {
            let mut s = RecordStep::new("swipe");
            s.from_selector = Some(UiSelector { resource_id: Some("a".into()), ..Default::default() });
            s.to_selector = Some(UiSelector { resource_id: Some("b".into()), ..Default::default() });
            s
        },
        {
            let mut s = RecordStep::new("text");
            s.selector = Some(UiSelector { resource_id: Some("x".into()), ..Default::default() });
            s.text = Some("hello world".into());
            s
        },
        {
            let s = RecordStep::new("key"); // key with no keycode -> no-op branch
            s
        },
        RecordStep::new("bogus"), // unknown action -> `_ => {}` arm
    ];
    let (tx, rx) = std::sync::mpsc::channel();
    let opts = ReplayOpts { speed: 1.0, loops: 1 };
    let handle = std::thread::spawn(move || {
        replay("nonexistent_adb_xyz", "", None, &steps, &tx, &opts);
    });
    let mut saw_progress = false;
    let mut saw_failed = false;
    let mut saw_done = false;
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(ReplayMsg::Progress { .. }) => saw_progress = true,
            Ok(ReplayMsg::Failed { .. }) => saw_failed = true,
            Ok(ReplayMsg::Info(_)) => {}
            Ok(ReplayMsg::Done) => {
                saw_done = true;
                break;
            }
            Err(_) => break,
        }
    }
    handle.join().unwrap();
    assert!(saw_progress, "expected at least one Progress message");
    assert!(saw_failed, "expected a Failed message for unresolved selectors");
    assert!(saw_done, "expected Done message");
}

#[test]
fn replay_multi_loop_emits_info() {
    let steps = vec![{
        let mut s = RecordStep::new("key");
        s.keycode = Some(4);
        s
    }];
    let (tx, rx) = std::sync::mpsc::channel();
    let opts = ReplayOpts { speed: 1.0, loops: 2 };
    let handle = std::thread::spawn(move || {
        replay("nonexistent_adb_xyz", "", None, &steps, &tx, &opts);
    });
    let mut saw_info = false;
    let mut saw_done = false;
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(ReplayMsg::Info(_)) => saw_info = true,
            Ok(ReplayMsg::Done) => {
                saw_done = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    handle.join().unwrap();
    assert!(saw_info, "expected Info between loops");
    assert!(saw_done);
}
