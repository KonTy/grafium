"""Synthetic WebKitGTK backing-store test; does not launch Grafium or open a graph.

Run with Python GI, Cairo, Pillow, and Xvfb installed:
    python ui/tests/transparency.webkit.py
This checks snapshot alpha, not desktop compositor transparency.
"""
import io
import json
import os
from pathlib import Path
import sys

if "--xvfb-child" not in sys.argv:
    import selectors
    import secrets
    import struct
    import subprocess
    import tempfile

    with tempfile.TemporaryDirectory(prefix=".transparency-webkit-", dir=".") as scratch:
        scratch = str(Path(scratch).resolve())
        authority = Path(scratch) / "Xauthority"
        cookie = secrets.token_bytes(16)

        def authorize(display):
            fields = [b"", display.encode(), b"MIT-MAGIC-COOKIE-1", cookie]
            authority.write_bytes(
                struct.pack(">H", 65535)
                + b"".join(struct.pack(">H", len(field)) + field for field in fields)
            )
            authority.chmod(0o600)

        authorize("")
        env = {
            **os.environ, "TMPDIR": scratch, "XDG_CACHE_HOME": scratch,
            "GDK_BACKEND": "x11", "XAUTHORITY": str(authority),
        }
        # Keep authentication/cache artifacts private to this project fixture.
        server = subprocess.Popen([
            "Xvfb", "-displayfd", "1", "-screen", "0", "640x180x24",
            "-nolock", "-nolisten", "unix", "-listen", "tcp", "-auth", str(authority),
        ], stdout=subprocess.PIPE, text=True, env=env)
        try:
            with selectors.DefaultSelector() as ready:
                ready.register(server.stdout, selectors.EVENT_READ)
                assert ready.select(10), "Xvfb did not start within 10 seconds"
                display = server.stdout.readline().strip()
            assert display.isdecimal(), f"Invalid Xvfb display: {display}"
            authorize(display)
            env["DISPLAY"] = f"localhost:{display}"
            result = subprocess.run(
                [sys.executable, __file__, "--xvfb-child"], env=env, timeout=45,
            )
        finally:
            server.terminate()
            server.wait(timeout=10)
        sys.exit(result.returncode)

os.environ["WEBKIT_DISABLE_DMABUF_RENDERER"] = "1"
os.environ["WEBKIT_DISABLE_COMPOSITING_MODE"] = "1"

import gi
from PIL import Image

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gdk, GLib, Gtk, WebKit2

failed = True
results = []
opacities = iter([0.65, 0, 1])
opacity = next(opacities)
css = (Path(__file__).resolve().parents[1] / "src/styles/global.css").read_text()
context = WebKit2.WebContext.new_ephemeral()
view = WebKit2.WebView.new_with_context(context)
clear = Gdk.RGBA(0, 0, 0, 0)
view.set_background_color(clear)
window = Gtk.Window()
window.set_default_size(640, 180)
window.set_resizable(False)
screen = window.get_screen()
visual = screen.get_rgba_visual()
assert visual is not None, "The synthetic X server must offer an RGBA visual"
window.set_visual(visual)
window.set_app_paintable(True)
window.add(view)


def fail(message):
    print(message, file=sys.stderr, flush=True)
    Gtk.main_quit()
    return False


def snapshot_ready(webview, result, _data):
    global failed, opacity
    try:
        surface = webview.get_snapshot_finish(result)
        png = io.BytesIO()
        surface.write_to_png(png)
        png.seek(0)
        image = Image.open(png).convert("RGBA")
        expected_alpha = round(255 * opacity)
        primary = image.getpixel((300, 150))
        sidebar = image.getpixel((550, 150))
        for label, pixel in [("main", primary), ("sidebar", sidebar)]:
            assert abs(pixel[3] - expected_alpha) <= 1, (
                f"{label}: opacity {opacity} expected alpha {expected_alpha}, got {pixel}"
            )
        glyphs = image.crop((10, 10, 290, 110))
        solid_glyph_pixels = sum(
            count for count, color in glyphs.getcolors(glyphs.width * glyphs.height)
            if color == (31, 35, 40, 255)
        )
        assert solid_glyph_pixels > 100, f"Glyph interiors lost opacity: {solid_glyph_pixels}"
        icon = image.getpixel((350, 60))
        assert icon == (31, 35, 40, 255), f"SVG icon interior lost opacity: {icon}"
        results.append({
            "opacity": opacity,
            "main": primary,
            "sidebar": sidebar,
            "opaqueGlyphPixels": solid_glyph_pixels,
            "icon": icon,
        })
        opacity = next(opacities, None)
        if opacity is None:
            failed = False
            print(json.dumps({
                "snapshots": results,
                "rgbaVisual": True,
                "screenComposited": screen.is_composited(),
                "scope": "WebKit backing-store alpha only; not compositor proof",
                "rendererWorkarounds": {
                    name: os.environ[name] for name in [
                        "WEBKIT_DISABLE_DMABUF_RENDERER",
                        "WEBKIT_DISABLE_COMPOSITING_MODE",
                    ]
                },
            }), flush=True)
            Gtk.main_quit()
            return
        webview.evaluate_javascript(f"""
            document.documentElement.toggleAttribute("data-window-transparency", {opacity} < 1);
            document.documentElement.style.setProperty("--window-bg-primary", "rgba(255,255,255,{opacity})");
            document.documentElement.style.setProperty("--window-bg-sidebar", "rgba(246,248,250,{opacity})");
        """, -1, None, None, None, updated, None)
    except Exception as error:
        fail(f"Native transparency snapshot failed: {error}")


def take_snapshot():
    # NONE is deliberate: TRANSPARENT_BACKGROUND would bypass the WebKit
    # backing color and would hide the native configuration being tested.
    view.get_snapshot(
        WebKit2.SnapshotRegion.VISIBLE, WebKit2.SnapshotOptions.NONE,
        None, snapshot_ready, None,
    )
    return False


def updated(webview, result, _data):
    try:
        webview.evaluate_javascript_finish(result)
        GLib.timeout_add(150, take_snapshot)
    except Exception as error:
        fail(f"Native transparency update failed: {error}")


def loaded(_view, event):
    if event == WebKit2.LoadEvent.FINISHED:
        GLib.timeout_add(150, take_snapshot)


def resource_started(_view, resource, _request):
    if resource.get_uri().startswith(("http:", "https:")):
        fail(f"Unexpected network request: {resource.get_uri()}")


view.connect("load-changed", loaded)
view.connect("load-failed", lambda _v, _e, uri, error: fail(f"Load failed: {uri}: {error}"))
view.connect("resource-load-started", resource_started)
GLib.timeout_add_seconds(30, lambda: fail("Native transparency snapshot timed out"))
window.show_all()
view.load_html(f"""<!doctype html>
<html data-window-transparency style="
  color-scheme:light;
  --bg-primary:#ffffff; --bg-sidebar:#f6f8fa;
  --window-bg-primary:rgba(255,255,255,.65);
  --window-bg-sidebar:rgba(246,248,250,.65)">
<head><style>{css}</style><style>
body {{ width:640px; height:180px; display:flex; color:#1f2328; }}
.main-content {{ position:relative; width:480px; height:180px; background:var(--bg-primary); }}
.sidebar {{ width:160px; height:180px; background:var(--bg-sidebar); }}
.glyphs {{ position:absolute; left:10px; top:10px; font:72px/100px sans-serif; color:#1f2328; }}
.icon {{ position:absolute; left:320px; top:30px; width:60px; height:60px; color:#1f2328; }}
</style></head><body>
<main class="main-content"><span class="glyphs">MMMM</span>
<svg class="icon" viewBox="0 0 60 60" fill="currentColor">
<path d="M24 5h12v19h19v12H36v19H24V36H5V24h19z"/></svg></main>
<aside class="sidebar"></aside>
</body></html>""", "about:blank")
Gtk.main()
window.destroy()
sys.exit(1 if failed else 0)
