"""Generate a multi-resolution .ico app icon for android-ui-viewer.

Concept: a rounded-square app tile (purple->blue gradient) with a white
phone outline and an overlapping magnifier (UI inspector). Rendered at high
resolution then downscaled with antialiasing into the standard Windows
icon sizes (16/24/32/48/64/128/256).
"""
from PIL import Image, ImageDraw

S = 512  # master render size


def lerp(a, b, t):
    return a + (b - a) * t


def round_rect(draw, box, radius, fill):
    draw.rounded_rectangle(box, radius=radius, fill=fill)


def make_master():
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # --- gradient tile background ---
    top = (124, 92, 255)   # purple
    bot = (56, 132, 255)   # blue
    tile = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    td = ImageDraw.Draw(tile)
    for y in range(S):
        t = y / (S - 1)
        r = int(lerp(top[0], bot[0], t))
        g = int(lerp(top[1], bot[1], t))
        b = int(lerp(top[2], bot[2], t))
        td.line([(0, y), (S, y)], fill=(r, g, b, 255))
    # rounded mask
    mask = Image.new("L", (S, S), 0)
    md = ImageDraw.Draw(mask)
    md.rounded_rectangle([0, 0, S - 1, S - 1], radius=int(S * 0.22), fill=255)
    img = Image.composite(tile, img, mask)

    d = ImageDraw.Draw(img)

    # soft top highlight (glossy)
    hl = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    hld = ImageDraw.Draw(hl)
    hld.rounded_rectangle(
        [int(S * 0.06), int(S * 0.04), int(S * 0.94), int(S * 0.5)],
        radius=int(S * 0.18), fill=(255, 255, 255, 40),
    )
    img = Image.alpha_composite(img, hl)
    d = ImageDraw.Draw(img)

    cx, cy = S / 2, S / 2
    # --- phone body (white outline) ---
    pw, ph = S * 0.30, S * 0.50
    px, py = cx - pw / 2, cy - ph / 2 - S * 0.02
    line_w = int(S * 0.028)
    d.rounded_rectangle(
        [px, py, px + pw, py + ph], radius=int(S * 0.05),
        outline=(255, 255, 255, 255), width=line_w,
    )
    # screen inner fill (slightly translucent)
    d.rounded_rectangle(
        [px + line_w, py + line_w, px + pw - line_w, py + ph - line_w],
        radius=int(S * 0.035), fill=(255, 255, 255, 38),
    )
    # home dot
    hr = int(S * 0.018)
    d.ellipse(
        [cx - hr, py + ph - int(S * 0.05) - hr, cx + hr, py + ph - int(S * 0.05) + hr],
        fill=(255, 255, 255, 255),
    )

    # --- magnifier (inspector) bottom-right, overlapping ---
    mx, my = px + pw * 0.74, py + ph * 0.80
    mr = S * 0.13
    ring_w = int(S * 0.032)
    # lens
    d.ellipse(
        [mx - mr, my - mr, mx + mr, my + mr],
        outline=(255, 255, 255, 255), width=ring_w,
        fill=(56, 132, 255, 120),
    )
    # handle
    hx2 = mx + mr * 0.72
    hy2 = my + mr * 0.72
    hx1 = mx + mr
    hy1 = my + mr
    d.line([(hx1, hy1), (hx2, hy2)], fill=(255, 255, 255, 255), width=ring_w)

    # subtle drop shadow under tile for depth
    return img


def main():
    master = make_master()
    sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [master.resize((sz, sz), Image.LANCZOS).convert("RGBA") for sz in sizes]
    # PIL's ICO writer uses the base image as one frame and appends the rest.
    # Use the largest frame as the base so every size is emitted.
    base = frames[-1]
    others = frames[:-1]
    base.save(
        "assets/appicon.ico",
        format="ICO",
        sizes=[(s, s) for s in sizes],
        append_images=others,
    )
    print("wrote assets/appicon.ico with sizes", sizes)


if __name__ == "__main__":
    main()
