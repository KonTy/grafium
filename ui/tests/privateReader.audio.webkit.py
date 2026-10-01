"""Exercise the real Rust capability stream with a synthetic two-hour MP3."""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import GLib, Gtk, WebKit2

failed = True
manager = WebKit2.UserContentManager()
manager.register_script_message_handler("result")


def received(_manager, result):
    global failed
    value = json.loads(result.get_js_value().to_string())
    failed = not value.get("ok")
    print(json.dumps(value), flush=True)
    Gtk.main_quit()


def timeout():
    print("Native audio playback did not complete within 45 seconds", flush=True)
    Gtk.main_quit()
    return False


manager.connect("script-message-received::result", received)
view = WebKit2.WebView.new_with_user_content_manager(manager)
view.get_settings().set_media_playback_requires_user_gesture(False)
window = Gtk.Window()
window.set_default_size(800, 600)
window.add(view)
window.show_all()
script = """
const report = value => webkit.messageHandlers.result.postMessage(JSON.stringify(value));
(async () => {
  const audio = new Audio();
  audio.volume = 0;
  audio.preload = 'auto';
  document.body.appendChild(audio);
  const errors = [];
  const state = () => JSON.stringify({ready:audio.readyState, network:audio.networkState,
    duration:String(audio.duration), time:audio.currentTime, paused:audio.paused,
    visible:!document.hidden, mp3:audio.canPlayType('audio/mpeg')});
  audio.addEventListener('error', () => errors.push(audio.error?.message || 'Media error'));
  const waitFor = async (predicate, label) => {
    const deadline = Date.now() + 12000;
    while (!predicate()) {
      if (errors.length || Date.now() > deadline)
        throw new Error(label + ': ' + errors.join('; ') + ' state=' +
          state());
      await new Promise(resolve => setTimeout(resolve, 50));
    }
  };
  const play = () => Promise.race([
    audio.play(),
    new Promise((_, reject) => setTimeout(() => reject(new Error('play timed out: ' + state())), 12000))
  ]);
  audio.src = AUDIO_URL;
  audio.load();
  await waitFor(() => Number.isFinite(audio.duration), 'metadata');
  if (Math.abs(audio.duration - 7200) > 0.1) throw new Error('Wrong two-hour duration');
  audio.currentTime = 3600;
  await waitFor(() => !audio.seeking && audio.currentTime >= 3600, 'one-hour seek');
  await play();
  await waitFor(() => audio.currentTime > 3600.2, 'playing after seek');
  audio.pause();
  const pausedAt = audio.currentTime;
  await new Promise(resolve => setTimeout(resolve, 8000));
  await play();
  await waitFor(() => audio.currentTime > pausedAt + 0.2, 'resume after stream timeout');
  audio.pause();
  audio.removeAttribute('src');
  audio.load();
  report({ok:true, duration:7200, seek:3600, pausedMilliseconds:8000, errors});
})().catch(error => report({ok:false, error:String(error)}));
""".replace("AUDIO_URL", json.dumps(sys.argv[1]))
html = f"<!doctype html><body><script>{script}</script>".encode()


class FixtureHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(html)))
        self.end_headers()
        self.wfile.write(html)

    def log_message(self, _format, *_args):
        pass


server = HTTPServer(("127.0.0.1", 0), FixtureHandler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
view.load_uri(f"http://127.0.0.1:{server.server_port}/")
GLib.timeout_add_seconds(45, timeout)
Gtk.main()
window.destroy()
server.shutdown()
server.server_close()
thread.join()
sys.exit(1 if failed else 0)
