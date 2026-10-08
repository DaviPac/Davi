"""Generate Davi's app icon (assets/icon.png + Windows .ico).

Run: python3 scripts/gen_icon.py
"""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
S = 1024  # master size


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def gradient(size, top_left, bottom_right):
    img = Image.new("RGBA", (size, size))
    px = img.load()
    for y in range(size):
        for x in range(size):
            t = (x + y) / (2 * (size - 1))
            px[x, y] = lerp(top_left, bottom_right, t) + (255,)
    return img


def rounded_mask(size, radius, inset=0):
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (inset, inset, size - 1 - inset, size - 1 - inset), radius=radius, fill=255
    )
    return mask


def build():
    inset = 56
    radius = 220

    # Soft drop shadow under the tile.
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle(
        (inset, inset + 18, S - inset, S - inset + 18), radius=radius, fill=(0, 0, 0, 110)
    )
    shadow = shadow.filter(ImageFilter.GaussianBlur(22))

    # Tile: indigo -> cyan diagonal gradient.
    tile = gradient(S, (99, 102, 241), (34, 211, 238))
    canvas = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    canvas.alpha_composite(shadow)
    canvas.paste(tile, (0, 0), rounded_mask(S, radius, inset))

    # Subtle top highlight for depth.
    gloss = Image.new("RGBA", (S, S), (255, 255, 255, 0))
    ImageDraw.Draw(gloss).rounded_rectangle(
        (inset, inset, S - inset, S // 2), radius=radius, fill=(255, 255, 255, 28)
    )
    canvas.alpha_composite(Image.composite(gloss, Image.new("RGBA", (S, S)), rounded_mask(S, radius, inset)))

    d = ImageDraw.Draw(canvas)
    white = (255, 255, 255, 255)

    # Bold geometric "D": stem + half-stadium bowl, hollowed out.
    left, top, bottom = 300, 270, 754
    stroke = 112
    bowl_right = 744
    h = bottom - top
    d.rounded_rectangle((left, top, bowl_right, bottom), radius=h // 2, fill=white)
    d.rectangle((left, top, left + h // 2, bottom), fill=white)
    inner = (left + stroke, top + stroke, bowl_right - stroke, bottom - stroke)
    ih = inner[3] - inner[1]
    hole = Image.new("L", (S, S), 0)
    hd = ImageDraw.Draw(hole)
    hd.rounded_rectangle(inner, radius=ih // 2, fill=255)
    hd.rectangle((inner[0], inner[1], inner[0] + ih // 2, inner[3]), fill=255)
    canvas.paste(tile, (0, 0), hole)

    # Request/response arrows inside the bowl.
    cx0, cx1 = inner[0] + 28, inner[2] - 40
    mid = (inner[1] + inner[3]) // 2
    w = 34
    for y, direction in ((mid - 52, 1), (mid + 52, -1)):
        d.rounded_rectangle((cx0, y - w // 2, cx1, y + w // 2), radius=w // 2, fill=white)
        tip_x = cx1 + 18 if direction == 1 else cx0 - 18
        base_x = tip_x - direction * 70
        d.polygon([(tip_x, y), (base_x, y - 46), (base_x, y + 46)], fill=white)

    # Amber accent dot (top right), like a "live" indicator.
    r = 58
    cx, cy = 770, 252
    d.ellipse((cx - r - 14, cy - r - 14, cx + r + 14, cy + r + 14), fill=(255, 255, 255, 255))
    d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=(251, 191, 36, 255))
    return canvas


def main():
    icon = build()
    (ROOT / "assets").mkdir(exist_ok=True)
    icon.resize((512, 512), Image.LANCZOS).save(ROOT / "assets" / "icon.png")
    ico_dir = ROOT / "crates" / "davi-ui" / "resources" / "windows"
    ico_dir.mkdir(parents=True, exist_ok=True)
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    icon.resize((256, 256), Image.LANCZOS).save(
        ico_dir / "davi.ico", sizes=[(s, s) for s in sizes]
    )
    print("wrote assets/icon.png and", ico_dir / "davi.ico")


if __name__ == "__main__":
    main()
