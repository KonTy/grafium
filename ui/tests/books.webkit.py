"""Run only synthetic reader fixtures in the installed native WebKitGTK."""
import json
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import GLib, Gtk, WebKit2

failed = True
unexpected_requests = []
manager = WebKit2.UserContentManager()
manager.register_script_message_handler("result")


def received(_manager, result):
    global failed
    value = json.loads(result.get_js_value().to_string())
    failed = not value.get("ok") or bool(unexpected_requests)
    print(json.dumps({"reader": value, "unexpected_requests": unexpected_requests}), flush=True)
    Gtk.main_quit()


manager.connect("script-message-received::result", received)
# Native automation, not a reader command: select visible fixture words as a
# user would, without weakening the shipped opaque-frame bridge or its CSP.
manager.add_script(WebKit2.UserScript.new("""
let attempts = 0;
const selectFixture = setInterval(() => {
  if (++attempts > 100) return clearInterval(selectFixture);
  const passage = document.querySelector('#passage')
    || [...document.querySelectorAll('p')].find(p => p.textContent.startsWith('Select this'))
    || document.querySelector('.textLayer');
  if (!passage || !passage.textContent.trim()) return;
  const range = document.createRange();
  range.selectNodeContents(passage);
  const selection = getSelection();
  selection.removeAllRanges();
  selection.addRange(range);
}, 200);
""", WebKit2.UserContentInjectedFrames.ALL_FRAMES,
    WebKit2.UserScriptInjectionTime.END, None, None))

view = WebKit2.WebView.new_with_user_content_manager(manager)


def resource_started(_view, resource, _request):
    uri = resource.get_uri()
    if uri.startswith(("http:", "https:")) and not uri.startswith("http://127.0.0.1:"):
        unexpected_requests.append(uri)


def load_failed(_view, _event, uri, error):
    print(f"Native reader load failed: {uri}: {error}", file=sys.stderr, flush=True)
    Gtk.main_quit()
    return False


def timeout():
    print("Native reader did not complete within 75 seconds", file=sys.stderr, flush=True)
    Gtk.main_quit()
    return False


view.connect("resource-load-started", resource_started)
view.connect("load-failed", load_failed)
window = Gtk.OffscreenWindow()
window.set_default_size(1200, 900)
window.add(view)
window.show_all()
GLib.timeout_add_seconds(75, timeout)
view.load_uri(sys.argv[1])
Gtk.main()
window.destroy()
sys.exit(1 if failed else 0)
