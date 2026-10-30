"""Draws the sample slide and cursor the README demo is rendered from."""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

FONTS = Path("/usr/share/fonts/truetype/dejavu")
WIDTH, HEIGHT = 2560, 1440

BACKGROUND = (24, 26, 33)
PANEL = (33, 36, 46)
TEXT = (230, 232, 238)
MUTED = (140, 146, 160)
ACCENT = (255, 196, 0)
KEYWORD = (198, 120, 221)
FUNCTION = (97, 175, 239)
STRING = (152, 195, 121)
NUMBER = (209, 154, 102)

CODE = [
    [("fn ", KEYWORD), ("zoom_around", FUNCTION), ("(camera: &", TEXT), ("mut ", KEYWORD), ("Camera, cursor: Vec2) {", TEXT)],
    [("    ", TEXT), ("let ", KEYWORD), ("size = camera.view_size(camera.zoom());", TEXT)],
    [("    ", TEXT), ("let ", KEYWORD), ("offset = (cursor - camera.center()).unscale(size);", TEXT)],
    [("    ", TEXT), ("// keep the point under the cursor fixed on screen", MUTED)],
    [("    camera.anchor = ", TEXT), ("Some", FUNCTION), ("(Anchor { point: cursor, offset });", TEXT)],
    [("    camera.log_zoom.target = ", TEXT), ("2.0", NUMBER), ("_f64.ln();", TEXT)],
    [("}", TEXT)],
    [("", TEXT)],
    [("#[test]", MUTED)],
    [("fn ", KEYWORD), ("zoom_keeps_cursor_point_fixed", FUNCTION), ("() {", TEXT)],
    [("    ", TEXT), ("let ", KEYWORD), ("cursor = Vec2::new(", TEXT), ("800.0", NUMBER), (", ", TEXT), ("500.0", NUMBER), (");", TEXT)],
    [("    assert_close(fraction_after, fraction_before);", TEXT)],
    [("}", TEXT)],
]

BULLETS = [
    "Share one window: the Closeup Stage",
    "Press 1-9 to zoom toward your cursor",
    "Digits still type in text fields",
    "Your own screen never zooms",
]

FINE_PRINT = [
    ("Stage output", "1920x1080 at 60 fps, bicubic upscaling"),
    ("Capture", "PipeWire via xdg-desktop-portal, cursor as metadata"),
    ("Motion", "critically damped springs, 70% dead zone"),
    ("Keys", "evdev + uinput, or GNOME Shell accelerators"),
]


def font(name, size):
    return ImageFont.truetype(str(FONTS / name), size)


def draw_slide(path):
    image = Image.new("RGB", (WIDTH, HEIGHT), BACKGROUND)
    draw = ImageDraw.Draw(image)

    draw.text((140, 120), "Live zoom, done right", font=font("DejaVuSans-Bold.ttf", 82), fill=TEXT)
    draw.rectangle((140, 250, 380, 258), fill=ACCENT)
    body = font("DejaVuSans.ttf", 44)
    for index, line in enumerate(BULLETS):
        y = 330 + index * 92
        draw.ellipse((150, y + 14, 170, y + 34), fill=ACCENT)
        draw.text((200, y), line, font=body, fill=TEXT)

    code_box = (1290, 120, 2420, 860)
    draw.rounded_rectangle(code_box, radius=24, fill=PANEL)
    for index, color in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
        draw.ellipse((1330 + index * 44, 152, 1356 + index * 44, 178), fill=color)
    mono = font("DejaVuSansMono.ttf", 26)
    for row, spans in enumerate(CODE):
        x = 1330
        for text, color in spans:
            draw.text((x, 220 + row * 46), text, font=mono, fill=color)
            x += draw.textlength(text, font=mono)

    chart_box = (140, 860, 1150, 1300)
    draw.rounded_rectangle(chart_box, radius=24, fill=PANEL)
    draw.text((180, 890), "Zoom levels", font=font("DejaVuSans-Bold.ttf", 34), fill=TEXT)
    for index, zoom in enumerate([1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]):
        x = 190 + index * 104
        height = 40 * zoom
        draw.rounded_rectangle((x, 1250 - height, x + 64, 1250), radius=10, fill=ACCENT if index == 2 else (90, 96, 112))
        draw.text((x + 22, 1258), str(index + 1), font=font("DejaVuSansMono.ttf", 24), fill=MUTED)

    table_box = (1290, 940, 2420, 1300)
    draw.rounded_rectangle(table_box, radius=24, fill=PANEL)
    small = font("DejaVuSans.ttf", 22)
    small_bold = font("DejaVuSans-Bold.ttf", 22)
    for row, (label, value) in enumerate(FINE_PRINT):
        y = 985 + row * 72
        draw.text((1340, y), label, font=small_bold, fill=ACCENT)
        draw.text((1560, y), value, font=small, fill=TEXT)
        if row:
            draw.line((1340, y - 18, 2370, y - 18), fill=(52, 56, 70), width=2)

    draw.text((140, 1360), "closeup  |  github.com/cyenite/closeup", font=font("DejaVuSans.ttf", 26), fill=MUTED)
    image.save(path)


def draw_cursor(path):
    scale = 2
    points = [(0, 0), (0, 17), (4, 13), (7, 20), (10, 19), (7, 12), (12, 12)]
    image = Image.new("RGBA", (16 * scale, 24 * scale), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    outline = [(x * scale + 2, y * scale + 2) for x, y in points]
    draw.polygon(outline, fill=(255, 255, 255, 255), outline=(0, 0, 0, 255), width=2)
    image.save(path)


if __name__ == "__main__":
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "target/demo")
    out.mkdir(parents=True, exist_ok=True)
    draw_slide(out / "slide.png")
    draw_cursor(out / "cursor.png")
    print(f"wrote {out / 'slide.png'} and {out / 'cursor.png'}")
