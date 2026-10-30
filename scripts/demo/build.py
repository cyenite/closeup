"""Composes the README demo GIF and stills from the frames rendered by
`cargo run --release --example demo`.

Left: the presenter's screen with the outline the GNOME extension draws.
Right: what the meeting sees, i.e. the Stage frames exactly as rendered.
"""

import json
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

FONTS = Path("/usr/share/fonts/truetype/dejavu")
PANEL = (560, 315)
GUTTER = 28
MARGIN = 28
TITLE = 44
KEYS_HEIGHT = 74
WIDTH = MARGIN * 2 + PANEL[0] * 2 + GUTTER
HEIGHT = MARGIN + TITLE + PANEL[1] + KEYS_HEIGHT

BACKGROUND = (13, 14, 18)
FRAME = (52, 56, 70)
TEXT = (230, 232, 238)
MUTED = (140, 146, 160)
ACCENT = (255, 196, 0)


def font(name, size):
    return ImageFont.truetype(str(FONTS / name), size)


def presenter_view(slide, cursor, frame):
    view = slide.copy()
    draw = ImageDraw.Draw(view)
    if frame["zoom"] > 1.001:
        x, y, w, h = frame["region"]
        box = (x * PANEL[0], y * PANEL[1], (x + w) * PANEL[0], (y + h) * PANEL[1])
        draw.rectangle((box[0] - 3, box[1] - 3, box[2] + 3, box[3] + 3), outline=ACCENT, width=2)
        label = f"{frame['zoom']:.1f}x"
        label_font = font("DejaVuSans-Bold.ttf", 12)
        width = draw.textlength(label, font=label_font) + 12
        top = box[1] - 24 if box[1] - 24 >= 0 else box[3] + 6
        draw.rounded_rectangle((box[0] - 3, top, box[0] - 3 + width, top + 18), radius=5, fill=(18, 18, 20))
        draw.text((box[0] + 3, top + 2), label, font=label_font, fill=ACCENT)
    cx, cy = frame["cursor"]
    view.alpha_composite(cursor, (int(cx * PANEL[0]) - 1, int(cy * PANEL[1]) - 1))
    return view


def keycaps(draw, frame, top):
    caps = [str(level) for level in range(1, 10)] + ["0"]
    size, gap = 34, 8
    total = len(caps) * size + (len(caps) - 1) * gap
    left = (WIDTH - total) // 2
    cap_font = font("DejaVuSansMono-Bold.ttf", 17)
    for index, cap in enumerate(caps):
        x = left + index * (size + gap)
        level = index + 1
        is_pressed = frame["key"] == level
        is_active = frame["level"] == level and cap != "0"
        fill = ACCENT if is_pressed else (40, 43, 54) if is_active else (24, 26, 33)
        outline = ACCENT if is_active or is_pressed else FRAME
        draw.rounded_rectangle((x, top, x + size, top + size), radius=7, fill=fill, outline=outline, width=2)
        color = (18, 18, 20) if is_pressed else TEXT if is_active else MUTED
        draw.text((x + size / 2, top + size / 2), cap, font=cap_font, fill=color, anchor="mm")
    hint = "press a digit to zoom toward the cursor, 1 to zoom back out"
    draw.text((WIDTH / 2, top + size + 16), hint, font=font("DejaVuSans.ttf", 13), fill=MUTED, anchor="mm")


def compose(slide, cursor, stage, frame):
    canvas = Image.new("RGBA", (WIDTH, HEIGHT), BACKGROUND)
    draw = ImageDraw.Draw(canvas)
    title_font = font("DejaVuSans-Bold.ttf", 16)
    left = (MARGIN, MARGIN + TITLE)
    right = (MARGIN + PANEL[0] + GUTTER, MARGIN + TITLE)
    subtitle_font = font("DejaVuSans.ttf", 13)
    for (x, _), title, subtitle in [
        (left, "Your screen", "stays normal; the outline shows what's shared"),
        (right, "What the meeting sees", "the Closeup Stage window"),
    ]:
        draw.text((x, MARGIN + 8), title, font=title_font, fill=TEXT)
        offset = draw.textlength(title, font=title_font) + 10
        draw.text((x + offset, MARGIN + 10), subtitle, font=subtitle_font, fill=MUTED)
    canvas.alpha_composite(presenter_view(slide, cursor, frame), left)
    canvas.alpha_composite(stage.resize(PANEL, Image.LANCZOS), right)
    for x, y in (left, right):
        draw.rounded_rectangle((x - 1, y - 1, x + PANEL[0], y + PANEL[1]), radius=4, outline=FRAME, width=2)
    keycaps(draw, frame, MARGIN + TITLE + PANEL[1] + 16)
    return canvas.convert("RGB")


def main():
    work = Path(sys.argv[1] if len(sys.argv) > 1 else "target/demo")
    media = Path(sys.argv[2] if len(sys.argv) > 2 else "docs/media")
    media.mkdir(parents=True, exist_ok=True)
    composed = work / "composed"
    shutil.rmtree(composed, ignore_errors=True)
    composed.mkdir()

    slide = Image.open(work / "slide.png").convert("RGBA").resize(PANEL, Image.LANCZOS)
    full_cursor = Image.open(work / "cursor.png").convert("RGBA")
    cursor = full_cursor.resize((full_cursor.width // 3, full_cursor.height // 3), Image.LANCZOS)
    frames = [json.loads(line) for line in (work / "timeline.jsonl").read_text().splitlines()]
    for frame in frames:
        stage = Image.open(work / "frames" / f"{frame['frame']:04d}.png").convert("RGBA")
        compose(slide, cursor, stage, frame).save(composed / f"{frame['frame']:04d}.png")

    palette = "fps=20,split[a][b];[a]palettegen=max_colors=192:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle"
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-framerate", "25", "-i", str(composed / "%04d.png"),
         "-vf", palette, "-loop", "0", str(media / "demo.gif")],
        check=True,
    )
    shutil.copy(composed / "0075.png", media / "side-by-side.png")
    shutil.copy(work / "frames" / "0075.png", media / "stage-2x.png")
    shutil.copy(work / "frames" / "0140.png", media / "stage-4x.png")
    print(f"wrote {media / 'demo.gif'} ({(media / 'demo.gif').stat().st_size // 1024} KiB) and stills")


if __name__ == "__main__":
    main()
