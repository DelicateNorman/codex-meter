//! Keyboard-driven, full-screen terminal interface.

use crate::tui::{BLUE, CYAN, GREEN, LIGHT, MUTED, RESET, YELLOW, display_width, fit_display};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, size,
};
use std::collections::HashSet;
use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Today,
    Week,
    Month,
    All,
    HistoryDay,
    HistoryWeek,
    HistoryMonth,
    Network,
}

impl View {
    pub fn key(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Week => "week",
            Self::Month => "month",
            Self::All => "all",
            Self::HistoryDay => "history_day",
            Self::HistoryWeek => "history_week",
            Self::HistoryMonth => "history_month",
            Self::Network => "network",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Week => "Week",
            Self::Month => "Month",
            Self::All => "All time",
            Self::HistoryDay => "Daily history",
            Self::HistoryWeek => "Weekly history",
            Self::HistoryMonth => "Monthly history",
            Self::Network => "Network",
        }
    }
}

impl fmt::Display for View {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuTarget {
    View(View),
    Project,
    Refresh,
    Help,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuItem {
    pub target: MenuTarget,
    pub label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandItem {
    pub name: &'static str,
    pub description: &'static str,
    pub target: MenuTarget,
}

pub const MENU_ITEMS: &[MenuItem] = &[
    MenuItem {
        target: MenuTarget::View(View::Today),
        label: "Today",
    },
    MenuItem {
        target: MenuTarget::View(View::Week),
        label: "Week",
    },
    MenuItem {
        target: MenuTarget::View(View::Month),
        label: "Month",
    },
    MenuItem {
        target: MenuTarget::View(View::All),
        label: "All time",
    },
    MenuItem {
        target: MenuTarget::View(View::HistoryDay),
        label: "Daily history",
    },
    MenuItem {
        target: MenuTarget::View(View::HistoryWeek),
        label: "Weekly history",
    },
    MenuItem {
        target: MenuTarget::View(View::HistoryMonth),
        label: "Monthly history",
    },
    MenuItem {
        target: MenuTarget::View(View::Network),
        label: "Network",
    },
    MenuItem {
        target: MenuTarget::Project,
        label: "Project",
    },
    MenuItem {
        target: MenuTarget::Refresh,
        label: "Refresh",
    },
    MenuItem {
        target: MenuTarget::Help,
        label: "Help",
    },
    MenuItem {
        target: MenuTarget::Quit,
        label: "Quit",
    },
];

pub const COMMAND_ITEMS: &[CommandItem] = &[
    CommandItem {
        name: "today",
        description: "View today's usage",
        target: MenuTarget::View(View::Today),
    },
    CommandItem {
        name: "week",
        description: "View this week's usage",
        target: MenuTarget::View(View::Week),
    },
    CommandItem {
        name: "month",
        description: "View this month's usage",
        target: MenuTarget::View(View::Month),
    },
    CommandItem {
        name: "all",
        description: "View usage since first use",
        target: MenuTarget::View(View::All),
    },
    CommandItem {
        name: "history day",
        description: "Show daily usage history",
        target: MenuTarget::View(View::HistoryDay),
    },
    CommandItem {
        name: "history week",
        description: "Show weekly usage history",
        target: MenuTarget::View(View::HistoryWeek),
    },
    CommandItem {
        name: "history month",
        description: "Show monthly usage history",
        target: MenuTarget::View(View::HistoryMonth),
    },
    CommandItem {
        name: "network",
        description: "Show token speed and response latency",
        target: MenuTarget::View(View::Network),
    },
    CommandItem {
        name: "project",
        description: "Choose one project or all projects",
        target: MenuTarget::Project,
    },
    CommandItem {
        name: "refresh",
        description: "Reload local, remote, and account data",
        target: MenuTarget::Refresh,
    },
    CommandItem {
        name: "help",
        description: "Show keyboard and command help",
        target: MenuTarget::Help,
    },
    CommandItem {
        name: "quit",
        description: "Exit Codex Meter",
        target: MenuTarget::Quit,
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractiveState {
    pub selected: usize,
    pub active_view: View,
    pub command_mode: bool,
    pub command_text: String,
    pub command_selected: usize,
    pub show_help: bool,
    pub running: bool,
    pub message: String,
    pub project_options: Vec<String>,
    pub project_selected: usize,
    pub project_filter: Option<String>,
    pub project_query: String,
    pub project_picker: bool,
    pub project_return_view: View,
}

impl Default for InteractiveState {
    fn default() -> Self {
        Self {
            selected: 0,
            active_view: View::Today,
            command_mode: false,
            command_text: String::new(),
            command_selected: 0,
            show_help: false,
            running: true,
            message: "Today".into(),
            project_options: Vec::new(),
            project_selected: 0,
            project_filter: None,
            project_query: String::new(),
            project_picker: false,
            project_return_view: View::Today,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Space,
    Escape,
    Backspace,
    CtrlC,
    Character(char),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    None,
    ViewChanged(View),
    ProjectPickerOpened,
    ProjectChanged(Option<String>),
    Refresh,
    Quit,
}

pub fn handle_key(state: &mut InteractiveState, key: Key) -> Action {
    if state.command_mode {
        return handle_command_key(state, key);
    }
    if state.project_picker {
        return handle_project_key(state, key);
    }

    match key {
        Key::Up | Key::Left => {
            state.selected = wrap_previous(state.selected, MENU_ITEMS.len());
            Action::None
        }
        Key::Down | Key::Right => {
            state.selected = (state.selected + 1) % MENU_ITEMS.len();
            Action::None
        }
        Key::Enter | Key::Space => activate(state, MENU_ITEMS[state.selected].target),
        Key::Character('/') => {
            state.command_mode = true;
            state.command_text.clear();
            state.command_selected = 0;
            state.message = "Use ↑/↓ to choose a command".into();
            Action::None
        }
        Key::Character('q') | Key::CtrlC => {
            state.running = false;
            Action::Quit
        }
        Key::Escape => {
            if state.show_help {
                state.show_help = false;
                state.selected = menu_index(MenuTarget::View(state.active_view));
                state.message = state.active_view.label().into();
            }
            Action::None
        }
        Key::Character('r') => {
            state.show_help = false;
            Action::Refresh
        }
        Key::Character('?') => {
            state.show_help = true;
            state.message = "Help".into();
            Action::None
        }
        _ => Action::None,
    }
}

pub fn parse_slash_command(state: &mut InteractiveState, text: &str) -> Action {
    let command = text
        .trim()
        .strip_prefix('/')
        .unwrap_or(text.trim())
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let target = match command.as_str() {
        "today" | "day" => Some(MenuTarget::View(View::Today)),
        "week" => Some(MenuTarget::View(View::Week)),
        "month" => Some(MenuTarget::View(View::Month)),
        "all" => Some(MenuTarget::View(View::All)),
        "history day" | "daily" => Some(MenuTarget::View(View::HistoryDay)),
        "history week" | "weekly" => Some(MenuTarget::View(View::HistoryWeek)),
        "history month" | "monthly" => Some(MenuTarget::View(View::HistoryMonth)),
        "network" => Some(MenuTarget::View(View::Network)),
        "project" | "projects" => Some(MenuTarget::Project),
        "refresh" | "reload" => Some(MenuTarget::Refresh),
        "help" | "?" => Some(MenuTarget::Help),
        "quit" | "exit" | "q" => Some(MenuTarget::Quit),
        _ => None,
    };
    if let Some(target) = target {
        activate(state, target)
    } else {
        state.message = format!(
            "Unknown command: /{} · use /help",
            if command.is_empty() { "?" } else { &command }
        );
        Action::None
    }
}

pub fn sync_projects(state: &mut InteractiveState, projects: impl IntoIterator<Item = String>) {
    let mut seen = HashSet::new();
    state.project_options = projects
        .into_iter()
        .filter_map(|project| {
            (!project.trim().is_empty() && seen.insert(project.clone())).then_some(project)
        })
        .collect();
    if state
        .project_filter
        .as_ref()
        .is_some_and(|project| !state.project_options.contains(project))
    {
        state.project_filter = None;
    }
    state.project_selected = state.project_filter.as_ref().map_or(0, |project| {
        state
            .project_options
            .iter()
            .position(|option| option == project)
            .map_or(0, |index| index + 1)
    });
}

pub fn render_interactive_screen(
    state: &InteractiveState,
    content: &str,
    width: usize,
    height: usize,
    color: bool,
    clear: bool,
) -> String {
    let width = width.max(40);
    let height = height.max(12);
    let layout_width = width.min(132);
    let header = vec![
        style(
            &fit_display("CODEX METER · INTERACTIVE", layout_width),
            CYAN,
            color,
        ),
        style(&"─".repeat(layout_width), BLUE, color),
    ];

    let body = if state.command_mode {
        command_palette_lines(state, layout_width, color, 5).join("\n")
    } else if state.project_picker {
        project_picker_text(state, layout_width, height.saturating_sub(9).clamp(1, 8))
    } else if state.show_help {
        help_text(layout_width)
    } else {
        content.into()
    };
    let menu = menu_lines(state, layout_width, color);
    let status = if show_message_in_status(state) {
        format!("Scope · {}  │  {}", scope_label(state), state.message)
    } else {
        format!("Scope · {}", scope_label(state))
    };
    let footer = if state.command_mode {
        vec![
            style(&"─".repeat(layout_width), BLUE, color),
            style(
                &fit_display(
                    if layout_width < 60 {
                        "Type · ↑/↓ choose · Enter run · Esc back"
                    } else {
                        "Slash input · ↑/↓ choose · Enter run · Esc back"
                    },
                    layout_width,
                ),
                MUTED,
                color,
            ),
            style(
                &fit_display(&format!("/{}▌", state.command_text), layout_width),
                LIGHT,
                color,
            ),
        ]
    } else if state.project_picker || state.show_help {
        vec![
            style(&"─".repeat(layout_width), BLUE, color),
            style(&fit_display(&status, layout_width), GREEN, color),
            style(
                &fit_display(
                    if state.project_picker {
                        if layout_width < 60 {
                            "Type filter · ↑/↓ · Enter · Esc"
                        } else {
                            "Type to filter · ↑/↓ choose · Enter apply · Esc cancel"
                        }
                    } else if layout_width < 80 {
                        "Arrows · Enter open · / menu · q quit"
                    } else {
                        "Arrows choose · Enter/Space open · / commands · r refresh · q quit"
                    },
                    layout_width,
                ),
                MUTED,
                color,
            ),
        ]
    } else {
        let mut footer = vec![
            style(&"─".repeat(layout_width), BLUE, color),
            style(&fit_display(&status, layout_width), GREEN, color),
        ];
        footer.extend(menu);
        footer.push(style(
            &fit_display(
                if layout_width < 80 {
                    "Arrows · Enter open · / menu · q quit"
                } else {
                    "Arrows choose · Enter/Space open · / commands · r refresh · q quit"
                },
                layout_width,
            ),
            MUTED,
            color,
        ));
        footer
    };

    let available = height.saturating_sub(header.len() + footer.len()).max(1);
    let body_lines: Vec<String> = body.lines().map(str::to_owned).collect();
    let clipped = clip_body(&body_lines, available, layout_width, color);
    let prefix = if clear { "\x1b[H\x1b[2J" } else { "" };
    format!("{prefix}{}", [header, clipped, footer].concat().join("\n"))
}

pub type ContentRenderer<'a> = dyn FnMut(View, usize, bool, Option<&str>) -> String + 'a;

pub struct InteractiveCallbacks<'a> {
    pub render_content: &'a mut ContentRenderer<'a>,
    pub refresh: &'a mut dyn FnMut() -> Result<(), String>,
    pub list_projects: &'a mut dyn FnMut() -> Vec<String>,
    pub poll_updates: &'a mut dyn FnMut() -> bool,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter(stdout: &mut impl Write) -> io::Result<Self> {
        enable_raw_mode()?;
        // Construct the guard before emitting either terminal command. If a
        // writer fails midway, unwinding still performs the full cleanup.
        let guard = Self;
        execute!(stdout, EnterAlternateScreen, Hide)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Run the real crossterm event loop. Quota/remote workers should signal
/// `poll_updates`; this loop renders the local first screen before polling them.
pub fn run_interactive(callbacks: &mut InteractiveCallbacks<'_>, color: bool) -> io::Result<i32> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("interactive mode requires a terminal"));
    }
    let mut state = InteractiveState::default();
    sync_projects(&mut state, (callbacks.list_projects)());
    let mut stdout = io::stdout();
    let _terminal = TerminalGuard::enter(&mut stdout)?;

    (|| {
        let mut last_content = String::new();
        while state.running {
            let (columns, rows) = size().unwrap_or((110, 30));
            let content = if state.command_mode || state.project_picker || state.show_help {
                String::new()
            } else {
                let content = (callbacks.render_content)(
                    state.active_view,
                    usize::from(columns),
                    color,
                    state.project_filter.as_deref(),
                );
                last_content.clone_from(&content);
                content
            };
            stdout.write_all(
                render_interactive_screen(
                    &state,
                    &content,
                    usize::from(columns),
                    usize::from(rows),
                    color,
                    true,
                )
                .as_bytes(),
            )?;
            stdout.flush()?;

            loop {
                if event::poll(Duration::from_millis(100))? {
                    let Event::Key(event) = event::read()? else {
                        continue;
                    };
                    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }
                    let action = handle_key(&mut state, decode_key(event));
                    if action == Action::Refresh {
                        state.show_help = false;
                        state.message = "Refreshing usage sources…".into();
                        stdout.write_all(
                            render_interactive_screen(
                                &state,
                                &last_content,
                                usize::from(columns),
                                usize::from(rows),
                                color,
                                true,
                            )
                            .as_bytes(),
                        )?;
                        stdout.flush()?;
                        match (callbacks.refresh)() {
                            Ok(()) => {
                                state.message = "Usage refreshed".into();
                                sync_projects(&mut state, (callbacks.list_projects)());
                            }
                            Err(error) => state.message = format!("Refresh failed: {error}"),
                        }
                    }
                    break;
                }
                if (callbacks.poll_updates)() {
                    sync_projects(&mut state, (callbacks.list_projects)());
                    break;
                }
            }
        }
        Ok(0)
    })()
}

fn decode_key(event: KeyEvent) -> Key {
    if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('c') {
        return Key::CtrlC;
    }
    match event.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char(' ') => Key::Space,
        KeyCode::Char(character) => Key::Character(character),
        _ => Key::Character('\0'),
    }
}

fn handle_command_key(state: &mut InteractiveState, key: Key) -> Action {
    let suggestions = command_suggestions(&state.command_text);
    match key {
        Key::Up | Key::Down => {
            if !suggestions.is_empty() {
                state.command_selected = if key == Key::Up {
                    wrap_previous(state.command_selected, suggestions.len())
                } else {
                    (state.command_selected + 1) % suggestions.len()
                };
            }
            Action::None
        }
        Key::Enter => {
            let text = suggestions
                .get(
                    state
                        .command_selected
                        .min(suggestions.len().saturating_sub(1)),
                )
                .map_or_else(|| state.command_text.clone(), |item| item.name.into());
            close_command_palette(state);
            parse_slash_command(state, &text)
        }
        Key::Escape => {
            close_command_palette(state);
            Action::None
        }
        Key::Backspace => {
            state.command_text.pop();
            state.command_selected = 0;
            Action::None
        }
        Key::CtrlC => {
            state.running = false;
            Action::Quit
        }
        Key::Space => {
            if state.command_text.chars().count() < 48 {
                state.command_text.push(' ');
                state.command_selected = 0;
            }
            Action::None
        }
        Key::Character(character) if !character.is_control() => {
            if state.command_text.chars().count() < 48 {
                state.command_text.push(character);
                state.command_selected = 0;
            }
            Action::None
        }
        _ => Action::None,
    }
}

fn handle_project_key(state: &mut InteractiveState, key: Key) -> Action {
    let choices = project_choices(state);
    match key {
        Key::Up | Key::Left => {
            if !choices.is_empty() {
                state.project_selected = wrap_previous(state.project_selected, choices.len());
            }
            Action::None
        }
        Key::Down | Key::Right => {
            if !choices.is_empty() {
                state.project_selected = (state.project_selected + 1) % choices.len();
            }
            Action::None
        }
        Key::Enter | Key::Space if key == Key::Enter || state.project_query.is_empty() => {
            if choices.is_empty() {
                state.message = "No projects match the filter".into();
                return Action::None;
            }
            let selected = choices[state.project_selected.min(choices.len() - 1)].clone();
            state.project_filter = selected.clone();
            state.project_query.clear();
            state.project_picker = false;
            state.active_view = state.project_return_view;
            state.selected = menu_index(MenuTarget::View(state.active_view));
            state.message = format!("Scope: {}", scope_label(state));
            Action::ProjectChanged(selected)
        }
        Key::Escape => {
            state.project_query.clear();
            state.project_picker = false;
            state.active_view = state.project_return_view;
            state.selected = menu_index(MenuTarget::View(state.active_view));
            state.message = state.active_view.label().into();
            Action::None
        }
        Key::CtrlC => {
            state.running = false;
            Action::Quit
        }
        Key::Backspace => {
            state.project_query.pop();
            reset_project_selection(state);
            state.message = "Choose a project".into();
            Action::None
        }
        Key::Space => {
            if state.project_query.chars().count() < 48 {
                state.project_query.push(' ');
                state.project_selected = 0;
                state.message = "Filtering projects".into();
            }
            Action::None
        }
        Key::Character(character) if !character.is_control() => {
            if state.project_query.chars().count() < 48 {
                state.project_query.push(character);
                state.project_selected = 0;
                state.message = "Filtering projects".into();
            }
            Action::None
        }
        _ => Action::None,
    }
}

fn close_command_palette(state: &mut InteractiveState) {
    state.command_mode = false;
    state.command_text.clear();
    state.command_selected = 0;
    state.message = state.active_view.label().into();
}

fn activate(state: &mut InteractiveState, target: MenuTarget) -> Action {
    match target {
        MenuTarget::Quit => {
            state.running = false;
            Action::Quit
        }
        MenuTarget::Refresh => Action::Refresh,
        MenuTarget::Help => {
            state.show_help = true;
            state.message = "Help".into();
            state.selected = menu_index(MenuTarget::Help);
            Action::None
        }
        MenuTarget::Project => {
            state.project_picker = true;
            state.project_query.clear();
            state.project_return_view = state.active_view;
            state.project_selected = state.project_filter.as_ref().map_or(0, |project| {
                state
                    .project_options
                    .iter()
                    .position(|option| option == project)
                    .map_or(0, |index| index + 1)
            });
            state.selected = menu_index(MenuTarget::Project);
            state.show_help = false;
            state.message = "Choose a project".into();
            Action::ProjectPickerOpened
        }
        MenuTarget::View(view) => {
            state.active_view = view;
            state.selected = menu_index(MenuTarget::View(view));
            state.show_help = false;
            state.message = view.label().into();
            Action::ViewChanged(view)
        }
    }
}

fn menu_index(target: MenuTarget) -> usize {
    MENU_ITEMS
        .iter()
        .position(|item| item.target == target)
        .unwrap_or(0)
}

fn menu_lines(state: &InteractiveState, width: usize, color: bool) -> Vec<String> {
    if width < 80 {
        return vec![style(
            &fit_display(
                &format!(
                    "Menu {}/{} · ▶ {}",
                    state.selected + 1,
                    MENU_ITEMS.len(),
                    MENU_ITEMS[state.selected].label
                ),
                width,
            ),
            CYAN,
            color,
        )];
    }

    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0;
    for (index, item) in MENU_ITEMS.iter().enumerate() {
        let active = matches!(item.target, MenuTarget::View(view) if view == state.active_view)
            || (item.target == MenuTarget::Project && state.project_picker);
        let marker = if index == state.selected {
            '▶'
        } else if active {
            '●'
        } else {
            ' '
        };
        let item_style = if index == state.selected {
            CYAN
        } else if matches!(item.target, MenuTarget::View(view) if view == state.active_view) {
            GREEN
        } else {
            MUTED
        };
        let token = style(&format!("{marker} {}", item.label), item_style, color);
        let token_width = display_width(item.label) + 2;
        let separator = usize::from(!current.is_empty()) * 3;
        if !current.is_empty() && current_width + separator + token_width > width {
            lines.push(current.join(" · "));
            current.clear();
            current_width = 0;
        }
        let separator = usize::from(!current.is_empty()) * 3;
        current.push(token);
        current_width += separator + token_width;
    }
    if !current.is_empty() {
        lines.push(current.join(" · "));
    }
    lines
}

fn command_suggestions(text: &str) -> Vec<&'static CommandItem> {
    let query = text
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    COMMAND_ITEMS
        .iter()
        .filter(|item| query.is_empty() || item.name.contains(&query))
        .collect()
}

fn command_palette_lines(
    state: &InteractiveState,
    width: usize,
    color: bool,
    page_size: usize,
) -> Vec<String> {
    let suggestions = command_suggestions(&state.command_text);
    if suggestions.is_empty() {
        return vec![style(
            &fit_display("Commands · no matches · Backspace to edit", width),
            YELLOW,
            color,
        )];
    }
    let selected = state.command_selected.min(suggestions.len() - 1);
    let page_start = selected / page_size * page_size;
    let page_end = (page_start + page_size).min(suggestions.len());
    let heading = if width < 60 {
        format!(
            "Commands {}-{}/{} · ↑/↓ · Enter · Esc",
            page_start + 1,
            page_end,
            suggestions.len()
        )
    } else {
        format!(
            "Commands · {}-{} of {} · ↑/↓ choose · Enter run · Esc back",
            page_start + 1,
            page_end,
            suggestions.len()
        )
    };
    let mut lines = vec![style(&fit_display(&heading, width), MUTED, color)];
    for (offset, item) in suggestions[page_start..page_end].iter().enumerate() {
        let index = page_start + offset;
        let marker = if index == selected { '▶' } else { ' ' };
        let line = format!("{marker} /{:<14}  {}", item.name, item.description);
        lines.push(style(
            &fit_display(&line, width),
            if index == selected { CYAN } else { LIGHT },
            color,
        ));
    }
    lines
}

pub fn project_picker_text(state: &InteractiveState, width: usize, page_size: usize) -> String {
    let choices = project_choices(state);
    let selected = state.project_selected.min(choices.len().saturating_sub(1));
    let page_size = page_size.max(1);
    let page_start = selected / page_size * page_size;
    let page_end = (page_start + page_size).min(choices.len());
    let heading = if choices.is_empty() {
        "Projects · no matches · Backspace to edit".into()
    } else if width < 60 {
        format!(
            "Projects {}-{}/{} · ↑/↓ · Enter · Esc",
            page_start + 1,
            page_end,
            choices.len()
        )
    } else {
        format!(
            "Projects · {}-{} of {} · ↑/↓ choose · Enter apply · Esc cancel",
            page_start + 1,
            page_end,
            choices.len()
        )
    };
    let mut lines = vec![
        "Choose project scope".into(),
        fit_display(
            &format!(
                "Filter · {}▌",
                if state.project_query.is_empty() {
                    "type to search".into()
                } else {
                    safe_label(&state.project_query)
                }
            ),
            width,
        ),
        fit_display(&heading, width),
        "─".repeat(width.min(88)),
    ];
    for (offset, project) in choices[page_start..page_end].iter().enumerate() {
        let index = page_start + offset;
        let marker = if index == selected { '▶' } else { ' ' };
        let label = project.as_deref().unwrap_or("All projects");
        let current = if project == &state.project_filter {
            "  (current)"
        } else {
            ""
        };
        lines.push(fit_display(
            &format!("{marker} {}{current}", safe_label(label)),
            width,
        ));
    }
    if !state.project_query.is_empty() && choices.is_empty() {
        lines.push(String::new());
        lines.push(fit_display(
            &format!("No projects match “{}”.", safe_label(&state.project_query)),
            width,
        ));
    } else if state.project_options.is_empty() {
        lines.push(String::new());
        lines.push(fit_display(
            "No named projects have been imported yet.",
            width,
        ));
    }
    lines.join("\n")
}

fn project_choices(state: &InteractiveState) -> Vec<Option<String>> {
    let query = case_fold(state.project_query.trim());
    if !query.is_empty() {
        return state
            .project_options
            .iter()
            .filter(|project| case_fold(project).contains(&query))
            .cloned()
            .map(Some)
            .collect();
    }
    std::iter::once(None)
        .chain(state.project_options.iter().cloned().map(Some))
        .collect()
}

/// A compact Unicode case-fold sufficient for identifiers and project paths.
/// Rust's standard library lowercasing handles the general Unicode mapping;
/// these expansions cover the important differences between lowercasing and
/// Python's `str.casefold` used by the reference implementation.
fn case_fold(value: &str) -> String {
    let mut folded = String::new();
    for character in value.chars() {
        match character {
            'ß' | 'ẞ' => folded.push_str("ss"),
            'ς' => folded.push('σ'),
            'ſ' => folded.push('s'),
            'ﬀ' => folded.push_str("ff"),
            'ﬁ' => folded.push_str("fi"),
            'ﬂ' => folded.push_str("fl"),
            'ﬃ' => folded.push_str("ffi"),
            'ﬄ' => folded.push_str("ffl"),
            'ﬅ' | 'ﬆ' => folded.push_str("st"),
            _ => folded.extend(character.to_lowercase()),
        }
    }
    folded
}

fn reset_project_selection(state: &mut InteractiveState) {
    let choices = project_choices(state);
    state.project_selected = if state.project_query.is_empty() {
        choices
            .iter()
            .position(|project| project == &state.project_filter)
            .unwrap_or(0)
    } else {
        0
    };
}

fn scope_label(state: &InteractiveState) -> String {
    state
        .project_filter
        .as_deref()
        .map(safe_label)
        .unwrap_or_else(|| "All projects".into())
}

fn safe_label(value: &str) -> String {
    let clean = value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    fit_display(clean.trim(), 96)
}

fn show_message_in_status(state: &InteractiveState) -> bool {
    !state.message.is_empty()
        && state.message != state.active_view.label()
        && !state.message.starts_with("Scope:")
}

fn clip_body(lines: &[String], available: usize, width: usize, color: bool) -> Vec<String> {
    if lines.len() <= available {
        return lines.to_vec();
    }
    let warning = style(
        &fit_display("… terminal too short; enlarge it to see more", width),
        YELLOW,
        color,
    );
    if available == 1 {
        return vec![warning];
    }
    let closing_frame = lines
        .last()
        .filter(|line| line.contains('╰') && line.contains('╯'))
        .cloned();
    if let Some(closing_frame) = closing_frame.filter(|_| available >= 2) {
        let mut candidates: Vec<String> = lines[..lines.len() - 1]
            .iter()
            .filter(|line| !(line.contains('├') && line.contains('┤')))
            .cloned()
            .collect();
        let visible = available - 2;
        let quota_bar_would_be_cut = candidates
            .iter()
            .position(|line| line.contains("Used  "))
            .is_some_and(|index| index >= visible);
        if quota_bar_would_be_cut
            && candidates
                .first()
                .is_some_and(|line| line.contains('╭') && line.contains('╮'))
        {
            candidates.remove(0);
        }
        let mut compact: Vec<String> = candidates.into_iter().take(visible).collect();
        compact.push(warning);
        compact.push(closing_frame);
        return compact;
    }
    let mut clipped = lines[..available - 1].to_vec();
    clipped.push(warning);
    clipped
}

fn help_text(width: usize) -> String {
    [
        "Keyboard",
        "  ↑ ↓ ← →   choose a menu item",
        "  Enter/Space open the selected item",
        "  /           type a slash command",
        "  r           refresh usage sources and account limits",
        "  Esc         close help or slash commands",
        "  q           quit (except while typing a filter or command)",
        "",
        "Slash commands",
        "  /today  /week  /month  /all",
        "  /history day  /history week  /history month",
        "  /network  /project  /refresh  /help  /quit",
        "",
        "Project picker",
        "  Type to filter; Backspace edits; Enter applies; Esc cancels",
    ]
    .into_iter()
    .map(|line| fit_display(line, width))
    .collect::<Vec<_>>()
    .join("\n")
}

fn style(text: &str, style: &str, color: bool) -> String {
    if color {
        format!("{style}{text}{RESET}")
    } else {
        text.into()
    }
}

fn wrap_previous(index: usize, length: usize) -> usize {
    if index == 0 { length - 1 } else { index - 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::{WEEK_MINUTES, WeeklyQuota};
    use crate::tui::{Overview, OverviewOptions, render_overview};

    #[test]
    fn arrows_enter_and_space_navigate_views() {
        let mut state = InteractiveState::default();
        assert_eq!(handle_key(&mut state, Key::Right), Action::None);
        assert_eq!(state.selected, 1);
        assert_eq!(
            handle_key(&mut state, Key::Enter),
            Action::ViewChanged(View::Week)
        );
        handle_key(&mut state, Key::Down);
        assert_eq!(
            handle_key(&mut state, Key::Space),
            Action::ViewChanged(View::Month)
        );
    }

    #[test]
    fn slash_palette_q_is_text_until_escape() {
        let mut state = InteractiveState::default();
        handle_key(&mut state, Key::Character('/'));
        assert_eq!(handle_key(&mut state, Key::Character('q')), Action::None);
        assert_eq!(state.command_text, "q");
        assert!(state.running);
        handle_key(&mut state, Key::Escape);
        assert!(!state.command_mode);
        assert_eq!(handle_key(&mut state, Key::Character('q')), Action::Quit);
        assert!(!state.running);
    }

    #[test]
    fn slash_palette_pages_and_uses_english_descriptions() {
        let mut state = InteractiveState::default();
        handle_key(&mut state, Key::Character('/'));
        for _ in 0..5 {
            handle_key(&mut state, Key::Down);
        }
        let rendered = render_interactive_screen(&state, "hidden", 100, 30, false, false);
        assert!(rendered.contains("Commands · 6-10 of 12"));
        assert!(rendered.contains("▶ /history week"));
        assert!(rendered.contains("Show weekly usage history"));
        assert!(!rendered.contains("hidden"));
    }

    #[test]
    fn project_order_filter_and_q_modal_semantics() {
        let mut state = InteractiveState::default();
        sync_projects(
            &mut state,
            ["recent", "older", "recent", ""].map(str::to_owned),
        );
        assert_eq!(state.project_options, ["recent", "older"]);
        assert_eq!(
            parse_slash_command(&mut state, "project"),
            Action::ProjectPickerOpened
        );
        handle_key(&mut state, Key::Character('q'));
        assert_eq!(state.project_query, "q");
        assert!(state.running);
        handle_key(&mut state, Key::Escape);
        assert!(!state.project_picker);
    }

    #[test]
    fn project_picker_filters_unicode_and_applies_scope() {
        let mut state = InteractiveState::default();
        sync_projects(
            &mut state,
            ["codex-stats", "Earth-Agent", "中文项目"].map(str::to_owned),
        );
        parse_slash_command(&mut state, "project");
        for character in "earth".chars() {
            handle_key(&mut state, Key::Character(character));
        }
        let rendered = project_picker_text(&state, 80, 8);
        assert!(rendered.contains("Earth-Agent"));
        assert!(!rendered.contains("codex-stats"));
        assert_eq!(
            handle_key(&mut state, Key::Enter),
            Action::ProjectChanged(Some("Earth-Agent".into()))
        );
        assert_eq!(state.project_filter.as_deref(), Some("Earth-Agent"));
    }

    #[test]
    fn project_search_uses_unicode_case_folding() {
        let mut state = InteractiveState::default();
        sync_projects(&mut state, ["Straße".to_owned(), "Σίσυφος".to_owned()]);
        parse_slash_command(&mut state, "project");
        for character in "strasse".chars() {
            handle_key(&mut state, Key::Character(character));
        }
        assert!(project_picker_text(&state, 80, 8).contains("Straße"));
    }

    #[test]
    fn short_narrow_screen_keeps_quota_and_bottom_menu() {
        let quotas = [WeeklyQuota {
            limit_id: "codex".into(),
            name: "Codex".into(),
            used_percent: 18,
            resets_at: None,
            window_minutes: WEEK_MINUTES,
            plan_type: None,
        }];
        let mut options = OverviewOptions::new("TODAY", 40, false);
        options.weekly_quotas = Some(&quotas);
        let body = render_overview(&Overview::default(), &[], &options);
        let rendered =
            render_interactive_screen(&InteractiveState::default(), &body, 40, 14, false, false);
        assert!(rendered.lines().count() <= 14);
        assert!(rendered.lines().all(|line| display_width(line) <= 40));
        assert!(rendered.contains("ACCOUNT WEEKLY LIMITS"));
        assert!(rendered.contains("82% left"));
        assert!(rendered.contains("Menu 1/12 · ▶ Today"));
        assert!(rendered.contains("terminal too short"));

        let twelve_lines =
            render_interactive_screen(&InteractiveState::default(), &body, 40, 12, false, false);
        assert!(twelve_lines.contains("Used  █"));
        assert!(twelve_lines.lines().count() <= 12);
    }

    #[test]
    fn short_wide_screen_preserves_panel_bottom() {
        let body = std::iter::once("╭────────────────────╮".to_owned())
            .chain((0..20).map(|index| format!("│ row {index:<14} │")))
            .chain(std::iter::once("╰────────────────────╯".to_owned()))
            .collect::<Vec<_>>()
            .join("\n");
        let rendered =
            render_interactive_screen(&InteractiveState::default(), &body, 180, 18, false, false);
        assert!(rendered.contains("terminal too short"));
        assert!(rendered.contains("╰────────────────────╯"));
        assert!(rendered.contains("Project"));
        assert!(rendered.lines().all(|line| display_width(line) <= 132));
    }

    #[test]
    fn menu_order_default_scope_and_main_modal_rules_match_reference() {
        assert_eq!(
            MENU_ITEMS.iter().map(|item| item.label).collect::<Vec<_>>(),
            [
                "Today",
                "Week",
                "Month",
                "All time",
                "Daily history",
                "Weekly history",
                "Monthly history",
                "Network",
                "Project",
                "Refresh",
                "Help",
                "Quit",
            ]
        );
        let mut state = InteractiveState::default();
        assert_eq!(state.active_view, View::Today);
        assert!(state.project_filter.is_none());
        assert_eq!(handle_key(&mut state, Key::Up), Action::None);
        assert_eq!(state.selected, MENU_ITEMS.len() - 1);
        assert_eq!(handle_key(&mut state, Key::Down), Action::None);
        assert_eq!(state.selected, 0);

        handle_key(&mut state, Key::Right);
        handle_key(&mut state, Key::Enter);
        assert_eq!(state.active_view, View::Week);
        handle_key(&mut state, Key::Character('?'));
        assert!(state.show_help);
        state.selected = menu_index(MenuTarget::Help);
        assert_eq!(handle_key(&mut state, Key::Escape), Action::None);
        assert!(!state.show_help);
        assert_eq!(state.selected, menu_index(MenuTarget::View(View::Week)));
        handle_key(&mut state, Key::Character('?'));
        assert_eq!(handle_key(&mut state, Key::Character('r')), Action::Refresh);
        assert!(!state.show_help);
    }

    #[test]
    fn every_slash_alias_routes_to_the_expected_view_or_action() {
        for (command, expected) in [
            ("today", View::Today),
            ("day", View::Today),
            ("week", View::Week),
            ("month", View::Month),
            ("all", View::All),
            ("history day", View::HistoryDay),
            ("daily", View::HistoryDay),
            ("history week", View::HistoryWeek),
            ("weekly", View::HistoryWeek),
            ("history month", View::HistoryMonth),
            ("monthly", View::HistoryMonth),
            ("network", View::Network),
        ] {
            let mut state = InteractiveState::default();
            assert_eq!(
                parse_slash_command(&mut state, command),
                Action::ViewChanged(expected),
                "{command}"
            );
            assert_eq!(state.active_view, expected, "{command}");
        }
        for command in ["project", "projects"] {
            let mut state = InteractiveState::default();
            assert_eq!(
                parse_slash_command(&mut state, command),
                Action::ProjectPickerOpened
            );
        }
        for command in ["refresh", "reload"] {
            assert_eq!(
                parse_slash_command(&mut InteractiveState::default(), command),
                Action::Refresh
            );
        }
        for command in ["help", "?"] {
            let mut state = InteractiveState::default();
            assert_eq!(parse_slash_command(&mut state, command), Action::None);
            assert!(state.show_help);
        }
        for command in ["quit", "exit", "q"] {
            assert_eq!(
                parse_slash_command(&mut InteractiveState::default(), command),
                Action::Quit
            );
        }
        let mut unknown = InteractiveState::default();
        assert_eq!(
            parse_slash_command(&mut unknown, "does not exist"),
            Action::None
        );
        assert!(unknown.message.contains("use /help"));
    }

    #[test]
    fn slash_typing_limit_backspace_wrap_and_ctrl_c_are_modal() {
        let mut state = InteractiveState::default();
        handle_key(&mut state, Key::Character('/'));
        for _ in 0..60 {
            handle_key(&mut state, Key::Character('X'));
        }
        assert_eq!(state.command_text.chars().count(), 48);
        handle_key(&mut state, Key::Backspace);
        handle_key(&mut state, Key::Space);
        assert_eq!(state.command_text.chars().count(), 48);

        state.command_text.clear();
        state.command_selected = 0;
        handle_key(&mut state, Key::Up);
        assert_eq!(state.command_selected, COMMAND_ITEMS.len() - 1);
        handle_key(&mut state, Key::Down);
        assert_eq!(state.command_selected, 0);
        assert_eq!(handle_key(&mut state, Key::CtrlC), Action::Quit);
        assert!(!state.running);
    }

    #[test]
    fn project_picker_handles_spaces_no_matches_and_restores_selection() {
        let mut state = InteractiveState {
            project_options: vec!["recent".into(), "my project".into(), "older".into()],
            project_filter: Some("older".into()),
            ..InteractiveState::default()
        };
        parse_slash_command(&mut state, "project");
        assert_eq!(state.project_selected, 3);
        for key in [
            Key::Character('m'),
            Key::Character('y'),
            Key::Space,
            Key::Character('p'),
        ] {
            handle_key(&mut state, key);
        }
        assert_eq!(state.project_query, "my p");
        assert!(project_picker_text(&state, 80, 8).contains("my project"));
        for _ in 0..4 {
            handle_key(&mut state, Key::Backspace);
        }
        assert!(state.project_query.is_empty());
        assert_eq!(state.project_selected, 3);

        for character in "missing".chars() {
            handle_key(&mut state, Key::Character(character));
        }
        assert!(project_picker_text(&state, 80, 8).contains("no matches"));
        assert_eq!(handle_key(&mut state, Key::Enter), Action::None);
        assert!(state.project_picker);
        assert_eq!(state.message, "No projects match the filter");
        assert_eq!(handle_key(&mut state, Key::Space), Action::None);
        assert!(state.project_query.ends_with(' '));
    }

    #[test]
    fn project_and_command_pages_fit_twelve_line_terminals() {
        let mut projects = InteractiveState::default();
        sync_projects(
            &mut projects,
            (0..10).map(|index| format!("project-{index}")),
        );
        parse_slash_command(&mut projects, "project");
        let project_screen = render_interactive_screen(&projects, "", 40, 12, false, false);
        assert!(project_screen.lines().count() <= 12);
        assert!(project_screen.contains("Projects 1-3/11"));
        assert!(project_screen.contains("All projects"));
        assert!(!project_screen.contains("project-2"));

        let mut command = InteractiveState::default();
        handle_key(&mut command, Key::Character('/'));
        let command_screen = render_interactive_screen(&command, "hidden", 80, 12, false, false);
        assert!(command_screen.lines().count() <= 12);
        assert!(command_screen.contains("View today's usage"));
        assert!(!command_screen.contains("hidden"));
    }

    #[test]
    fn crossterm_key_mapping_covers_arrows_utf8_and_controls() {
        assert_eq!(
            decode_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Key::Up
        );
        assert_eq!(
            decode_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Key::Enter
        );
        assert_eq!(
            decode_key(KeyEvent::new(KeyCode::Char('中'), KeyModifiers::NONE)),
            Key::Character('中')
        );
        assert_eq!(
            decode_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Key::CtrlC
        );
        assert_eq!(
            decode_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
            Key::Backspace
        );
        assert_eq!(
            decode_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Key::Escape
        );
    }
}
