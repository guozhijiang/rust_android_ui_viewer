"""Python port of the Rust `ui_tree.rs`.

Parses a uiautomator XML hierarchy into a nested `Node` tree with stable ids,
and provides the same two helpers used by the desktop app:
  - `hit_test(x, y)` -> innermost (smallest-area) node containing a point
  - `subtree_matches(q)` -> any attr contains the search query (for filtering)
"""

from __future__ import annotations

import re
import xml.etree.ElementTree as ET
from typing import Optional

_BOUNDS_RE = re.compile(r"[\[\],]")


class Bounds:
    __slots__ = ("left", "top", "right", "bottom")

    def __init__(self, left: int, top: int, right: int, bottom: int):
        self.left = left
        self.top = top
        self.right = right
        self.bottom = bottom

    @property
    def width(self) -> int:
        return self.right - self.left

    @property
    def height(self) -> int:
        return self.bottom - self.top

    def contains(self, x: int, y: int) -> bool:
        return self.left <= x <= self.right and self.top <= y <= self.bottom


class Node:
    __slots__ = ("id", "attrs", "bounds", "children")

    def __init__(self, node_id: int, attrs: dict[str, str], bounds: Optional[Bounds]):
        self.id = node_id
        self.attrs = attrs
        self.bounds = bounds
        self.children: list["Node"] = []

    def count(self) -> int:
        return 1 + sum(c.count() for c in self.children)

    def find(self, node_id: int) -> Optional["Node"]:
        if self.id == node_id:
            return self
        for c in self.children:
            found = c.find(node_id)
            if found:
                return found
        return None

    def hit_test(self, x: int, y: int) -> Optional[int]:
        best: Optional[tuple[int, int]] = None  # (id, area)

        def rec(node: "Node"):
            nonlocal best
            if node.bounds and node.bounds.contains(x, y):
                area = node.bounds.width * node.bounds.height
                if best is None or area < best[1]:
                    best = (node.id, area)
            for c in node.children:
                rec(c)

        rec(self)
        return best[0] if best else None

    def subtree_matches(self, q: str) -> bool:
        q = q.lower()
        if any(q in v.lower() for v in self.attrs.values()):
            return True
        return any(c.subtree_matches(q) for c in self.children)

    def to_dict(self) -> dict:
        b = self.bounds
        return {
            "id": self.id,
            "attrs": self.attrs,
            "bounds": {
                "left": b.left,
                "top": b.top,
                "right": b.right,
                "bottom": b.bottom,
            }
            if b
            else None,
            "children": [c.to_dict() for c in self.children],
        }


def _parse_bounds(s: str) -> Optional[Bounds]:
    nums = [int(p) for p in _BOUNDS_RE.split(s) if p.strip().isdigit()]
    if len(nums) == 4:
        return Bounds(nums[0], nums[1], nums[2], nums[3])
    return None


def parse(xml: str) -> Node:
    """Parse a uiautomator XML dump into a `Node` tree rooted at <hierarchy>."""
    root_el = ET.fromstring(xml)
    counter = 0

    def build(el) -> Node:
        nonlocal counter
        node_id = counter
        counter += 1
        attrs = {k: (v if v is not None else "") for k, v in el.attrib.items()}
        bounds = _parse_bounds(attrs.get("bounds", ""))
        node = Node(node_id, attrs, bounds)
        for child in el:
            node.children.append(build(child))
        return node

    return build(root_el)
