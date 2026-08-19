use std::collections::HashMap;

use anyhow::{anyhow, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// A rectangle in device/screen pixel coordinates: [left, top] -> [right, bottom].
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
}

/// A node in the UI hierarchy, mirroring a `<node>` element from uiautomator.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: usize,
    pub attrs: HashMap<String, String>,
    pub bounds: Option<Bounds>,
    pub children: Vec<Node>,
}

impl Node {
    fn new() -> Self {
        Node {
            id: 0,
            attrs: HashMap::new(),
            bounds: None,
            children: Vec::new(),
        }
    }

    /// Total number of nodes in this subtree (including self).
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(|c| c.count()).sum::<usize>()
    }

    /// Find a node by its stable id (assigned at parse time).
    pub fn find(&self, id: usize) -> Option<&Node> {
        if self.id == id {
            return Some(self);
        }
        for c in &self.children {
            if let Some(n) = c.find(id) {
                return Some(n);
            }
        }
        None
    }

    /// Return the id of the deepest (smallest-area) node whose bounds contain (x, y),
    /// simulating uiautomatorviewer's "pick the innermost element" behavior.
    pub fn hit_test(&self, x: i32, y: i32) -> Option<usize> {
        fn rec(node: &Node, x: i32, y: i32, best: &mut Option<(usize, i64)>) {
            if let Some(b) = &node.bounds {
                if b.contains(x, y) {
                    let area = b.width() as i64 * b.height() as i64;
                    if best.is_none() || area < best.unwrap().1 {
                        *best = Some((node.id, area));
                    }
                }
            }
            for c in &node.children {
                rec(c, x, y, best);
            }
        }
        let mut best = None;
        rec(self, x, y, &mut best);
        best.map(|(id, _)| id)
    }

    /// True if this node (or any descendant) matches the search query in any attribute.
    pub fn subtree_matches(&self, q: &str) -> bool {
        let q = q.to_lowercase();
        self.attrs.values().any(|v| v.to_lowercase().contains(&q))
            || self.children.iter().any(|c| c.subtree_matches(&q))
    }
}

fn parse_bounds(s: &str) -> Option<Bounds> {
    let nums: Vec<i32> = s
        .split(|c: char| c == '[' || c == ']' || c == ',')
        .filter_map(|p| p.trim().parse::<i32>().ok())
        .collect();
    if nums.len() == 4 {
        Some(Bounds {
            left: nums[0],
            top: nums[1],
            right: nums[2],
            bottom: nums[3],
        })
    } else {
        None
    }
}

fn build_node(e: &BytesStart, counter: &mut usize) -> Result<Node> {
    let mut node = Node::new();
    node.id = *counter;
    *counter += 1;

    for a in e.attributes() {
        let a = a?;
        // uiautomator XML attributes carry no namespace prefix, so the full key
        // (via AsRef<[u8]>) is identical to the local name.
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let val: String = a.unescape_value()?.into_owned();
        node.attrs.insert(key, val);
    }
    node.bounds = parse_bounds(node.attrs.get("bounds").map(|s| s.as_str()).unwrap_or(""));
    Ok(node)
}

/// Parse a uiautomator XML dump into a `Node` tree rooted at `<hierarchy>`.
pub fn parse(xml: &str) -> Result<Node> {
    let mut reader = Reader::from_str(xml);

    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut counter: usize = 0;
    let mut buf = Vec::new();

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let node = build_node(&e, &mut counter)?;
                stack.push(node);
            }
            Ok(Event::Empty(e)) => {
                let node = build_node(&e, &mut counter)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                }
            }
            Ok(Event::End(_)) => {
                let node = stack
                    .pop()
                    .ok_or_else(|| anyhow!("XML 结构异常：多余的结束标签"))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
            Ok(_) => {}
            Err(e) => {
                return Err(anyhow!(
                    "XML 解析错误 (位置 {}): {}",
                    reader.buffer_position(),
                    e
                ))
            }
        }
    }

    root.ok_or_else(|| anyhow!("未找到 hierarchy 根节点"))
}
