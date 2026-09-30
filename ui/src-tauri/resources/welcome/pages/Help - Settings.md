# Help - Settings

Settings controls Grafium's appearance, graph behavior, indexing, AI features,
and synchronization.

On Linux, local deployment installs the matching Grafium launcher and icon
files. New windows use the stable `grafium` application ID, even when the native
executable is named `grafium-bin`. If an already-running window retains a generic
icon after an update, close it normally and reopen Grafium.
The smplOS menu has its own application index. Local deployment refreshes that
index when its helper is available and uses a distinct local icon name so an
older system icon cannot override it. Close and reopen the menu after an update.

**Mobile location format** controls what an Android long-press on the editor
bar's **Time** button inserts. OpenStreetMap link is the default; plain
coordinates and a device `geo:` map link are also available.

## Theme and colors

Open **Theme** to choose a palette. The color swatches preview its accents on
its own background. All built-in themes use vivid reds, blues, purples, cyan,
orange, and pink for headings, tags, and callouts, with deeper shades on light
surfaces and brighter shades on dark surfaces so the text stays readable.

Markdown heading levels use red, blue, purple, cyan, orange, and pink in order
from H1 to H6. The colors are presentation only; your Markdown is unchanged.
Matrix keeps its green text, black canvas, and terminal font, with colorful
headings and highlights rather than an all-green page. OLED keeps its true-black
background. Selecting another theme updates these colors immediately.

## AI / Knowledge Engine

Open this section to configure an embedded local model, Ollama, an
OpenAI-compatible endpoint, or a cloud provider. Test the connection, select
generation and embedding models separately, and index only the graph content
you want AI to retrieve. See [[AI Setup And Privacy]] for provider, privacy,
and troubleshooting guidance.

**Native model recovery** lists models whose worker exit was not confirmed or
which stopped abnormally. **Allow one GPU attempt** authorizes only that model's
next attempt; it does not disable memory admission. **Request GPU for Chat**
requests offload for the selected chat model. Active native jobs must finish
before their worker can be evicted for a retry.

## Sync

Add filesystem or WebDAV targets here. USB drives and file-server folders are
filesystem targets. Configure each destination separately and use **Sync Now**
when it is available. See [[Sync And Privacy]] before syncing a real graph.

Press **F1** while focused inside a Settings section for section-specific help.
For sync instructions, focus a control in the Sync section and press F1 again.

Grafium's source code is licensed under the GNU Affero General Public License
version 3. Modified versions made available over a network must also make their
corresponding source available to their users.
