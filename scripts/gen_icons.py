#!/usr/bin/env python3
"""Generate iOS and Android icons from assets/icons/master.svg.

Raster rendering is pinned to rsvg-convert 2.63.4. The only non-system font
used in the Play feature graphic is the repository's licensed Noto Sans JP.
Run with: python3 scripts/gen_icons.py
"""

from __future__ import annotations

import copy
import json
import os
import re
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ICON_ROOT = ROOT / "assets" / "icons"
MASTER = ICON_ROOT / "master.svg"
IOS = ICON_ROOT / "ios"
ANDROID = ICON_ROOT / "android"
FONT_DIR = ROOT / "assets" / "fonts"
RENDERER_VERSION = "2.63.4"
SVG_NS = "http://www.w3.org/2000/svg"
ANDROID_NS = "http://schemas.android.com/apk/res/android"
INK = "#173a45"
CANVAS = "#edf3f1"
ACCENT = "#e65c43"

ET.register_namespace("", SVG_NS)
ET.register_namespace("android", ANDROID_NS)


def read_mark() -> tuple[str, ET.Element]:
    root = ET.parse(MASTER).getroot()
    canvas = root.find(f"{{{SVG_NS}}}rect[@id='canvas']")
    if canvas is None or canvas.get("fill") != CANVAS:
        raise RuntimeError("master SVG must contain the opaque #edf3f1 canvas")
    if root.find(f"{{{SVG_NS}}}clipPath") is not None:
        raise RuntimeError("master SVG must not bake an app-icon mask")
    mark = next((node for node in root.iter() if node.get("id") == "atlas-mark"), None)
    if mark is None:
        raise RuntimeError("master SVG is missing #atlas-mark")
    return ET.tostring(mark, encoding="unicode"), mark


def xml_text(root: ET.Element) -> str:
    return '<?xml version="1.0" encoding="utf-8"?>\n' + ET.tostring(root, encoding="unicode") + "\n"


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def renderer_path() -> str:
    renderer = shutil.which("rsvg-convert")
    if renderer is None:
        raise RuntimeError("rsvg-convert 2.63.4 is required to generate icons")
    output = subprocess.run(
        [renderer, "--version"], check=True, capture_output=True, text=True
    ).stdout.splitlines()[0]
    match = re.search(r"\b(\d+\.\d+\.\d+)\b", output)
    if match is None or match.group(1) != RENDERER_VERSION:
        actual = match.group(1) if match else output
        raise RuntimeError(f"expected rsvg-convert {RENDERER_VERSION}, found {actual}")
    return renderer


def svg_wrapper(width: int, height: int, body: str) -> str:
    return (
        f'<svg xmlns="{SVG_NS}" width="{width}" height="{height}" '
        f'viewBox="0 0 1024 1024">{body}</svg>\n'
    )


def render_png(renderer: str, source: Path, destination: Path, width: int, height: int, env: dict[str, str]) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            renderer,
            "--format=png",
            f"--width={width}",
            f"--height={height}",
            f"--output={destination}",
            str(source),
        ],
        check=True,
        env=env,
        stdout=subprocess.DEVNULL,
    )


def png_dimensions(path: Path) -> tuple[int, int]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise RuntimeError(f"renderer did not create a PNG: {path}")
    return int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")


def check_png(path: Path, width: int, height: int) -> None:
    if png_dimensions(path) != (width, height):
        raise RuntimeError(f"wrong PNG dimensions for {path}: expected {width}x{height}")

def app_icon_pixels(filename: str) -> int:
    dimensions = re.search(r"-(\d+(?:-\d+)?)@(\d)x\.png$", filename)
    if dimensions is None:
        raise RuntimeError(f"invalid AppIcon filename: {filename}")
    points = float(dimensions.group(1).replace("-", "."))
    return int(round(points * int(dimensions.group(2))))


def vector_path(source: ET.Element, monochrome: bool = False) -> ET.Element | None:
    path_id = source.get("id", "")
    if monochrome and path_id in {"panel-left", "panel-center", "panel-right", "locator-center"}:
        return None
    path = ET.Element("path")
    path.set(f"{{{ANDROID_NS}}}pathData", source.get("d", ""))
    fill = source.get("fill")
    stroke = source.get("stroke")
    if monochrome:
        color = INK
        if fill and fill != "none":
            path.set(f"{{{ANDROID_NS}}}fillColor", color)
        else:
            path.set(f"{{{ANDROID_NS}}}fillColor", "#00000000")
        if stroke:
            path.set(f"{{{ANDROID_NS}}}strokeColor", color)
            path.set(f"{{{ANDROID_NS}}}strokeWidth", source.get("stroke-width", "1"))
    else:
        if fill and fill != "none":
            path.set(f"{{{ANDROID_NS}}}fillColor", fill)
        else:
            path.set(f"{{{ANDROID_NS}}}fillColor", "#00000000")
        if stroke:
            path.set(f"{{{ANDROID_NS}}}strokeColor", stroke)
            path.set(f"{{{ANDROID_NS}}}strokeWidth", source.get("stroke-width", "1"))
    if source.get("stroke-linecap"):
        path.set(f"{{{ANDROID_NS}}}strokeLineCap", source.attrib["stroke-linecap"])
    if source.get("stroke-linejoin"):
        path.set(f"{{{ANDROID_NS}}}strokeLineJoin", source.attrib["stroke-linejoin"])
    return path


def make_vector(mark: ET.Element, monochrome: bool = False) -> str:
    vector = ET.Element(
        "vector",
        {
            f"{{{ANDROID_NS}}}width": "108dp",
            f"{{{ANDROID_NS}}}height": "108dp",
            f"{{{ANDROID_NS}}}viewportWidth": "1024",
            f"{{{ANDROID_NS}}}viewportHeight": "1024",
        },
    )
    for source in list(mark):
        path = vector_path(source, monochrome=monochrome)
        if path is not None:
            vector.append(path)
    return xml_text(vector)


def create_ios_catalog() -> dict[str, Path]:
    images: list[dict[str, str]] = []
    outputs: dict[str, Path] = {}

    def add_icon(idiom: str, size: str, points: float, scale: int) -> None:
        pixels = int(round(points * scale))
        size_name = size.replace(".", "-")
        filename = f"AppIcon-{idiom}-{size_name}@{scale}x.png"
        images.append(
            {
                "filename": filename,
                "idiom": idiom,
                "scale": f"{scale}x",
                "size": f"{size}x{size}",
            }
        )
        outputs[filename] = IOS / "Assets.xcassets" / "AppIcon.appiconset" / filename

    for size, scales in (("20", (2, 3)), ("29", (2, 3)), ("40", (2, 3)), ("60", (2, 3))):
        for scale in scales:
            add_icon("iphone", size, float(size), scale)
    for size, scales in (("20", (1, 2)), ("29", (1, 2)), ("40", (1, 2)), ("76", (1, 2)), ("83.5", (2,))):
        for scale in scales:
            add_icon("ipad", size, float(size), scale)
    add_icon("ios-marketing", "1024", 1024, 1)

    appicon = {
        "images": images,
        "info": {"author": "xcode", "version": 1},
    }
    appicon_path = IOS / "Assets.xcassets" / "AppIcon.appiconset" / "Contents.json"
    write_text(appicon_path, json.dumps(appicon, ensure_ascii=False, indent=2) + "\n")

    launch_dir = IOS / "Assets.xcassets" / "LaunchMark.imageset"
    launch_images = [
        ("LaunchMark.png", 256, "1x"),
        ("LaunchMark@2x.png", 512, "2x"),
        ("LaunchMark@3x.png", 768, "3x"),
    ]
    write_text(
        launch_dir / "Contents.json",
        json.dumps(
            {
                "images": [
                    {"filename": filename, "idiom": "universal", "scale": scale}
                    for filename, _, scale in launch_images
                ],
                "info": {"author": "xcode", "version": 1},
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
    )

    color_components = {
        "alpha": "1.000",
        "blue": "0.945",
        "green": "0.953",
        "red": "0.929",
    }
    write_text(
        IOS / "Assets.xcassets" / "LaunchBackground.colorset" / "Contents.json",
        json.dumps(
            {
                "colors": [
                    {
                        "color": {
                            "color-space": "srgb",
                            "components": color_components,
                        },
                        "idiom": "universal",
                    }
                ],
                "info": {"author": "xcode", "version": 1},
            },
            indent=2,
        )
        + "\n",
    )
    for filename, pixels, _ in launch_images:
        outputs[filename] = launch_dir / filename
    return outputs


def create_android_resources(mark: ET.Element) -> dict[Path, tuple[int, int]]:
    outputs: dict[Path, tuple[int, int]] = {}
    drawable = ANDROID / "res" / "drawable"
    anydpi = ANDROID / "res" / "mipmap-anydpi-v26"
    values = ANDROID / "res" / "values"

    write_text(drawable / "ic_launcher_foreground.xml", make_vector(mark))
    write_text(drawable / "launch_mark.xml", make_vector(mark))
    write_text(drawable / "ic_launcher_monochrome.xml", make_vector(mark, monochrome=True))
    background_vector = ET.Element(
        "vector",
        {
            f"{{{ANDROID_NS}}}width": "108dp",
            f"{{{ANDROID_NS}}}height": "108dp",
            f"{{{ANDROID_NS}}}viewportWidth": "1024",
            f"{{{ANDROID_NS}}}viewportHeight": "1024",
        },
    )
    ET.SubElement(
        background_vector,
        "path",
        {
            f"{{{ANDROID_NS}}}fillColor": "@color/ic_launcher_background",
            f"{{{ANDROID_NS}}}pathData": "M0,0 H1024 V1024 H0 Z",
        },
    )
    write_text(drawable / "ic_launcher_background.xml", xml_text(background_vector))

    adaptive = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        f'<adaptive-icon xmlns:android="{ANDROID_NS}">\n'
        '  <background android:drawable="@drawable/ic_launcher_background" />\n'
        '  <foreground android:drawable="@drawable/ic_launcher_foreground" />\n'
        '  <monochrome android:drawable="@drawable/ic_launcher_monochrome" />\n'
        '</adaptive-icon>\n'
    )
    for name in ("ic_launcher.xml", "ic_launcher_round.xml"):
        write_text(anydpi / name, adaptive)
    write_text(
        values / "colors.xml",
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<resources>\n'
        f'  <color name="ic_launcher_background">{CANVAS}</color>\n'
        f'  <color name="launch_background">{CANVAS}</color>\n'
        '</resources>\n',
    )
    write_text(
        values / "styles.xml",
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<resources>\n'
        '  <style name="KuniankiThemeBase" parent="@android:style/Theme.Material.Light.NoActionBar">\n'
        '    <item name="android:windowBackground">@color/launch_background</item>\n'
        '    <item name="android:windowDrawsSystemBarBackgrounds">true</item>\n'
        '    <item name="android:statusBarColor">@android:color/transparent</item>\n'
        '    <item name="android:navigationBarColor">@android:color/transparent</item>\n'
        '    <item name="android:windowLightStatusBar">true</item>\n'
        '    <item name="android:windowLightNavigationBar">true</item>\n'
        '  </style>\n'
        '  <style name="KuniankiTheme" parent="@style/KuniankiThemeBase" />\n'
        '</resources>\n',
    )
    write_text(
        ANDROID / "res" / "values-v31" / "styles.xml",
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<resources>\n'
        '  <style name="KuniankiTheme" parent="@style/KuniankiThemeBase">\n'
        '    <item name="android:windowSplashScreenBackground">@color/launch_background</item>\n'
        '    <item name="android:windowSplashScreenAnimatedIcon">@drawable/launch_mark</item>\n'
        '    <item name="android:windowSplashScreenAnimationDuration">0</item>\n'
        '  </style>\n'
        '</resources>\n',
    )

    for density, pixels in (("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)):
        directory = ANDROID / "res" / f"mipmap-{density}"
        for name in ("ic_launcher.png", "ic_launcher_round.png"):
            outputs[directory / name] = (pixels, pixels)
    outputs[ANDROID / "play-icon-512.png"] = (512, 512)
    outputs[ANDROID / "previews" / "mark-1024.png"] = (1024, 1024)
    outputs[ANDROID / "previews" / "mark-180.png"] = (180, 180)
    outputs[ANDROID / "previews" / "mark-48.png"] = (48, 48)
    return outputs


def preview_svg(mark: ET.Element, mask: str) -> str:
    if mask == "round":
        shape = '<circle cx="512" cy="512" r="512" />'
    else:
        shape = (
            '<path d="M512 36 C824 36 988 200 988 512 '
            'C988 824 824 988 512 988 C200 988 36 824 36 512 '
            'C36 200 200 36 512 36 Z" />'
        )
    group = ET.tostring(mark, encoding="unicode")
    return (
        f'<svg xmlns="{SVG_NS}" width="512" height="512" viewBox="0 0 1024 1024">'
        f'<defs><clipPath id="icon-mask">{shape}</clipPath></defs>'
        '<g clip-path="url(#icon-mask)">'
        f'<rect width="1024" height="1024" fill="{CANVAS}" />{group}'
        '</g></svg>\n'
    )


def feature_svg(mark: ET.Element) -> str:
    feature_mark = copy.deepcopy(mark)
    feature_mark.set("transform", "translate(-87.92 -87.92) scale(0.66)")
    group = ET.tostring(feature_mark, encoding="unicode")
    return (
        f'<svg xmlns="{SVG_NS}" width="1024" height="500" viewBox="0 0 1024 500">'
        f'<rect width="1024" height="500" fill="{CANVAS}" />'
        f'{group}'
        '<text x="500" y="306" fill="{ink}" font-family="Noto Sans JP" '
        'font-size="112" font-weight="700">くに暗記</text>'
        '</svg>\n'
    ).replace("{ink}", INK)


def setup_font_env(temp_dir: Path) -> dict[str, str]:
    config = temp_dir / "fonts.conf"
    cache = temp_dir / "font-cache"
    config.write_text(
        '<?xml version="1.0"?>\n'
        '<!DOCTYPE fontconfig SYSTEM "fonts.dtd">\n'
        '<fontconfig>\n'
        f'  <dir>{FONT_DIR}</dir>\n'
        f'  <cachedir>{cache}</cachedir>\n'
        '</fontconfig>\n',
        encoding="utf-8",
    )
    env = os.environ.copy()
    env["FONTCONFIG_FILE"] = str(config)
    env["XDG_CACHE_HOME"] = str(temp_dir / "xdg-cache")
    subprocess.run(["fc-cache", "-f", str(FONT_DIR)], check=True, env=env, stdout=subprocess.DEVNULL)
    match = subprocess.run(
        ["fc-match", "Noto Sans JP", "--format=%{file}"],
        check=True,
        capture_output=True,
        text=True,
        env=env,
    ).stdout
    if Path(match).resolve() != (FONT_DIR / "NotoSansJP.ttf").resolve():
        raise RuntimeError(f"Play graphic font did not resolve to bundled Noto Sans JP: {match}")
    return env


def main() -> None:
    renderer = renderer_path()
    _, mark = read_mark()
    mark_svg = ET.tostring(mark, encoding="unicode")
    ios_outputs = create_ios_catalog()
    android_outputs = create_android_resources(mark)

    with tempfile.TemporaryDirectory(prefix="kunianki-icons-", dir=ROOT / "target") as temporary:
        temp_dir = Path(temporary)
        env = setup_font_env(temp_dir)
        transparent_source = temp_dir / "launch-mark.svg"
        transparent_source.write_text(svg_wrapper(1024, 1024, mark_svg), encoding="utf-8")
        feature_source = temp_dir / "play-feature.svg"
        feature_source.write_text(feature_svg(mark), encoding="utf-8")

        for filename, destination in ios_outputs.items():
            if filename.startswith("AppIcon-"):
                pixels = app_icon_pixels(filename)
                render_png(renderer, MASTER, destination, pixels, pixels, env)
            else:
                pixels = 256 if filename == "LaunchMark.png" else int(filename.split("@")[1].split("x")[0]) * 256
                render_png(renderer, transparent_source, destination, pixels, pixels, env)

        for destination, (width, height) in android_outputs.items():
            render_png(renderer, MASTER, destination, width, height, env)
        render_png(
            renderer,
            feature_source,
            ANDROID / "play-feature-graphic.png",
            1024,
            500,
            env,
        )
        for shape in ("round", "squircle"):
            source = temp_dir / f"adaptive-{shape}.svg"
            source.write_text(preview_svg(mark, shape), encoding="utf-8")
            render_png(
                renderer,
                source,
                ANDROID / "previews" / f"adaptive-{shape}.png",
                512,
                512,
                env,
            )
        # Android launch icons use the vector foreground/background. Store and
        # legacy raster files are rendered from the same unmasked master SVG.
        for density, pixels in (("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)):
            for name in ("ic_launcher.png", "ic_launcher_round.png"):
                check_png(ANDROID / "res" / f"mipmap-{density}" / name, pixels, pixels)
        check_png(ANDROID / "play-icon-512.png", 512, 512)
        check_png(ANDROID / "play-feature-graphic.png", 1024, 500)
        check_png(ANDROID / "previews" / "mark-1024.png", 1024, 1024)
        check_png(ANDROID / "previews" / "mark-180.png", 180, 180)
        check_png(ANDROID / "previews" / "mark-48.png", 48, 48)
        check_png(ANDROID / "previews" / "adaptive-round.png", 512, 512)
        check_png(ANDROID / "previews" / "adaptive-squircle.png", 512, 512)
        for filename, path in ios_outputs.items():
            if filename.startswith("AppIcon-"):
                pixels = app_icon_pixels(filename)
            elif filename == "LaunchMark.png":
                pixels = 256
            else:
                pixels = int(filename.split("@")[1].split("x")[0]) * 256
            check_png(path, pixels, pixels)

    print(f"Generated {len(ios_outputs)} iOS images and {len(android_outputs) + 3} Android images using rsvg-convert {RENDERER_VERSION}.")


if __name__ == "__main__":
    main()
