//! Terminal entry point: sets up/tears down the terminal, opens (or creates)
//! a Grafium graph directory, and runs the event loop. All application logic
//! lives in `App`; this file only wires the terminal to it.

mod app;
mod data;
mod panels;
mod widgets;

use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::app::App;
use crate::data::{CoreRepository, GraphRepository};
use grafium_core::Graph;

fn graph_root_from_args() -> PathBuf {
    std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current dir"))
}

fn open_or_create_graph(root: &std::path::Path) -> Result<Graph> {
    if !Graph::is_graph_root_dir(root) {
        std::fs::create_dir_all(root.join("pages"))?;
        std::fs::create_dir_all(root.join("journals"))?;
        std::fs::create_dir_all(root.join(Graph::default_metadata_dir_name()))?;
    }
    Graph::open(root).with_context(|| format!("opening graph at {}", root.display()))
}

fn main() -> Result<()> {
    let root = graph_root_from_args();
    let graph = open_or_create_graph(&root)?;
    let repo: Rc<dyn GraphRepository> = Rc::new(CoreRepository::new(graph));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Fullscreen })?;

    let result = run(&mut terminal, repo);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    repo: Rc<dyn GraphRepository>,
) -> Result<()> {
    let mut app = App::new(repo);
    loop {
        terminal.draw(|f| app.draw(f))?;
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                app.on_key(key);
            }
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}
