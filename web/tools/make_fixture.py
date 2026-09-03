"""Generate an offline test fixture (screenshot + uiautomator XML).

Lets the web UI (and its E2E tests) be exercised without a physical device:
the synthetic screen and the hierarchy share the same bounds, so overlay
highlighting, hit-testing, search and zoom can all be verified offline.

  python make_fixture.py [outdir]

Writes fixture.png and fixture.xml next to this file by default.
Sized to match the V2509A test device (470x1024).
"""
from __future__ import annotations

import os
import sys
from xml.sax.saxutils import quoteattr

W, H = 470, 1024

SB_H, AB_H, NAV_H = 40, 56, 84
CONTENT_TOP = SB_H + AB_H      # 96
CONTENT_BOT = H - NAV_H        # 940
ROW_H = 84

ITEMS = [
    ("WLAN", "wlan"),
    ("蓝牙", "bluetooth"),
    ("移动网络", "mobile"),
    ("显示与亮度", "display"),
    ("声音与振动", "sound"),
    ("电池", "battery"),
    ("应用管理", "apps"),
    ("关于手机", "about"),
    ("系统更新", "update"),
]

PKG = "com.android.settings"
SYSUI = "com.android.systemui"


def N(cls="android.view.View", text="", rid="", desc="", bounds=(0, 0, 0, 0),
      clickable=False, children=None):
    """One uiautomator node as plain data (serialized later)."""
    return dict(cls=cls, text=text, rid=rid, desc=desc, bounds=bounds,
                clickable=clickable, children=children or [])


def build_tree():
    """The synthetic 设置 screen, matching build_image()'s geometry."""
    rows = []
    for idx, (label, key) in enumerate(ITEMS):
        top = CONTENT_TOP + idx * ROW_H
        rows.append(N(
            cls="android.widget.LinearLayout",
            rid=f"{PKG}:id/{key}_row",
            bounds=(0, top, W, top + ROW_H),
            clickable=True,
            children=[
                N(cls="android.widget.TextView", text=label, rid=f"{PKG}:id/{key}",
                  bounds=(56, top + 26, 300, top + 58)),
                N(cls="android.widget.ImageView", desc=f"{label} 图标",
                  rid=f"{PKG}:id/{key}_icon",
                  bounds=(16, top + 22, 48, top + 62)),
            ],
        ))

    return N(cls="android.widget.FrameLayout", bounds=(0, 0, W, H), children=[
        # status bar
        N(cls="android.widget.LinearLayout", rid=f"{SYSUI}:id/status_bar",
          bounds=(0, 0, W, SB_H), children=[
              N(cls="android.widget.TextView", text="09:41",
                rid=f"{SYSUI}:id/clock", bounds=(12, 8, 74, 32)),
              N(cls="android.widget.TextView", text="85%", desc="电量 85%",
                rid=f"{SYSUI}:id/battery", bounds=(W - 62, 8, W - 12, 32)),
          ]),
        # action bar
        N(cls="android.widget.LinearLayout", rid=f"{PKG}:id/action_bar",
          bounds=(0, SB_H, W, SB_H + AB_H), children=[
              N(cls="android.widget.TextView", text="设置",
                rid=f"{PKG}:id/title", bounds=(16, SB_H + 14, W - 16, SB_H + 42)),
          ]),
        # scrolling list
        N(cls="android.widget.ScrollView", rid=f"{PKG}:id/list",
          bounds=(0, CONTENT_TOP, W, CONTENT_BOT), children=[
              N(cls="android.widget.LinearLayout", rid=f"{PKG}:id/list_container",
                bounds=(0, CONTENT_TOP, W, CONTENT_BOT), children=rows),
          ]),
        # navigation bar
        N(cls="android.widget.LinearLayout", rid=f"{SYSUI}:id/nav_bar",
          bounds=(0, CONTENT_BOT, W, H), children=[
              N(cls="android.widget.ImageView", desc="返回",
                rid=f"{SYSUI}:id/back",
                bounds=(40, CONTENT_BOT + 26, 100, H - 26), clickable=True),
              N(cls="android.widget.ImageView", desc="主页",
                rid=f"{SYSUI}:id/home",
                bounds=(190, CONTENT_BOT + 26, 280, H - 26), clickable=True),
              N(cls="android.widget.ImageView", desc="最近任务",
                rid=f"{SYSUI}:id/recent",
                bounds=(370, CONTENT_BOT + 26, 440, H - 26), clickable=True),
          ]),
    ])


def _serialize(node: dict, lines: list, indent: int) -> None:
    b = "[%d,%d][%d,%d]" % node["bounds"]
    attrs = [
        'index="0"',
        "text=%s" % quoteattr(node["text"]),
        "resource-id=%s" % quoteattr(node["rid"]),
        "class=%s" % quoteattr(node["cls"]),
        "package=%s" % quoteattr(PKG),
        "content-desc=%s" % quoteattr(node["desc"]),
        'checkable="false"', 'checked="false"',
        'clickable="%s"' % ("true" if node["clickable"] else "false"),
        'enabled="true"', 'focusable="false"', 'focused="false"',
        'scrollable="false"', 'long-clickable="false"', 'password="false"',
        'selected="false"',
        "bounds=%s" % quoteattr(b),
    ]
    pad = "  " * indent
    if node["children"]:
        lines.append(pad + "<node " + " ".join(attrs) + ">")
        for c in node["children"]:
            _serialize(c, lines, indent + 1)
        lines.append(pad + "</node>")
    else:
        lines.append(pad + "<node " + " ".join(attrs) + " />")


def build_hierarchy() -> str:
    lines = ["<?xml version='1.0' encoding='UTF-8'?>", "<hierarchy rotation=\"0\">"]
    _serialize(build_tree(), lines, 1)
    lines.append("</hierarchy>")
    return "\n".join(lines)


def build_image(path: str) -> None:
    from PIL import Image, ImageDraw

    img = Image.new("RGB", (W, H), (18, 18, 20))
    d = ImageDraw.Draw(img)

    # status bar
    d.rectangle([0, 0, W, SB_H], fill=(10, 10, 12))
    d.text((12, 12), "09:41", fill=(230, 230, 235))
    d.text((W - 58, 12), "85%", fill=(230, 230, 235))

    # action bar
    d.rectangle([0, SB_H, W, SB_H + AB_H], fill=(28, 28, 32))
    d.text((16, SB_H + 16), "设置", fill=(245, 245, 250))

    # list rows
    for i, (label, _key) in enumerate(ITEMS):
        top = CONTENT_TOP + i * ROW_H
        d.rectangle([0, top, W, top + ROW_H - 1], fill=(24, 24, 27))
        d.rectangle([16, top + 22, 48, top + 62], fill=(60, 120, 200))
        d.text((56, top + 30), label, fill=(225, 225, 232))
        d.line([(56, top + ROW_H - 1), (W, top + ROW_H - 1)], fill=(38, 38, 42))

    # nav bar
    d.rectangle([0, CONTENT_BOT, W, H], fill=(10, 10, 12))
    for cx, sym in ((70, "<"), (235, "O"), (405, "[]")):
        d.ellipse([cx - 26, CONTENT_BOT + 26, cx + 26, H - 26], outline=(90, 90, 98))
        d.text((cx - 4, CONTENT_BOT + 40), sym, fill=(200, 200, 208))

    img.save(path, "PNG")


def main() -> None:
    outdir = sys.argv[1] if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__))
    os.makedirs(outdir, exist_ok=True)
    png = os.path.join(outdir, "fixture.png")
    xml = os.path.join(outdir, "fixture.xml")
    build_image(png)
    with open(xml, "w", encoding="utf-8") as f:
        f.write(build_hierarchy())
    print("wrote", png)
    print("wrote", xml)


if __name__ == "__main__":
    main()
