"""Dependency-free interactive terminal navigation for Codex Meter."""

from __future__ import annotations

import os
import select
import shutil
import sys
import termios
import tty
from dataclasses import dataclass
from typing import Callable, TextIO

from .tui import BLUE, CYAN, GREEN, LIGHT, MUTED, RESET, YELLOW


@dataclass(frozen=True)
class MenuItem:
    key: str
    label: str


@dataclass(frozen=True)
class CommandItem:
    name: str
    description: str


MENU_ITEMS = (
    MenuItem("today", "Today"),
    MenuItem("week", "Week"),
    MenuItem("month", "Month"),
    MenuItem("all", "All time"),
    MenuItem("history_day", "Daily history"),
    MenuItem("history_week", "Weekly history"),
    MenuItem("history_month", "Monthly history"),
    MenuItem("network", "Network"),
    MenuItem("refresh", "Refresh"),
    MenuItem("help", "Help"),
    MenuItem("quit", "Quit"),
)

COMMAND_ITEMS = (
    CommandItem("today", "查看今天的使用统计"),
    CommandItem("week", "查看本周汇总"),
    CommandItem("month", "查看本月汇总"),
    CommandItem("all", "查看从首次使用至今的汇总"),
    CommandItem("history day", "按天查看历史变化"),
    CommandItem("history week", "按周查看历史变化"),
    CommandItem("history month", "按月查看历史变化"),
    CommandItem("network", "查看 Token 速度与响应延迟"),
    CommandItem("refresh", "重新读取本机 Codex 记录"),
    CommandItem("help", "显示键盘和命令帮助"),
    CommandItem("quit", "退出 Codex Meter"),
)


@dataclass
class InteractiveState:
    selected: int = 0
    active_view: str = "today"
    command_mode: bool = False
    command_text: str = ""
    command_selected: int = 0
    show_help: bool = False
    running: bool = True
    message: str = "Today"


def handle_key(state: InteractiveState, key: str) -> str | None:
    """Update interactive state and return a side-effect action when needed."""
    if state.command_mode:
        return _handle_command_key(state, key)

    if key in ("up", "left"):
        state.selected = (state.selected - 1) % len(MENU_ITEMS)
        return None
    if key in ("down", "right"):
        state.selected = (state.selected + 1) % len(MENU_ITEMS)
        return None
    if key in ("enter", "space"):
        return _activate(state, MENU_ITEMS[state.selected].key)
    if key == "/":
        state.command_mode = True
        state.command_text = ""
        state.command_selected = 0
        state.message = "Use ↑/↓ to choose a command"
        return None
    if key in ("q", "ctrl_c"):
        state.running = False
        return "quit"
    if key == "escape":
        if state.show_help:
            state.show_help = False
            state.message = _label_for(state.active_view)
        return None
    if key == "r":
        return "refresh"
    if key == "?":
        state.show_help = True
        state.message = "Help"
    return None


def parse_slash_command(state: InteractiveState, text: str) -> str | None:
    command = text.strip().removeprefix("/").lower()
    aliases = {
        "today": "today",
        "day": "today",
        "week": "week",
        "month": "month",
        "all": "all",
        "history day": "history_day",
        "history week": "history_week",
        "history month": "history_month",
        "network": "network",
        "daily": "history_day",
        "weekly": "history_week",
        "monthly": "history_month",
        "refresh": "refresh",
        "reload": "refresh",
        "help": "help",
        "?": "help",
        "quit": "quit",
        "exit": "quit",
        "q": "quit",
    }
    target = aliases.get(" ".join(command.split()))
    if target is None:
        state.message = f"Unknown command: /{command or '?'} · use /help"
        return None
    return _activate(state, target)


def render_interactive_screen(
    state: InteractiveState,
    content: str,
    *,
    width: int,
    height: int,
    color: bool,
    clear: bool = True,
) -> str:
    width = max(40, width)
    height = max(12, height)
    title = "CODEX METER · INTERACTIVE"
    menu = _menu_lines(state, width, color)
    controls = (
        "Slash input · ↑/↓ choose · Enter run · Esc back"
        if state.command_mode
        else "Arrows choose · Enter/Space open · / commands · r refresh · q quit"
    )
    prompt = f"/{state.command_text}▌" if state.command_mode else f"● {state.message}"
    header = [
        _style(title, CYAN, color),
        _style("─" * min(width, 132), BLUE, color),
    ]
    body = _help_text(width) if state.show_help else content
    body_lines = body.splitlines()
    footer = [
        _style("─" * min(width, 132), BLUE, color),
        *menu,
        _style(controls[:width], MUTED, color),
    ]
    if state.command_mode:
        footer.extend(_command_palette_lines(state, width, color))
    footer.append(_style(prompt[:width], LIGHT if state.command_mode else GREEN, color))
    available = max(1, height - len(header) - len(footer))
    clipped = body_lines[:available]
    if len(body_lines) > available:
        clipped[-1:] = [_style("… terminal too short; enlarge it to see more"[:width], YELLOW, color)]
    prefix = "\x1b[H\x1b[2J" if clear else ""
    return prefix + "\n".join([*header, *clipped, *footer])


def run_interactive(
    render_content: Callable[[str, int, bool], str],
    refresh: Callable[[], None],
    *,
    color: bool = True,
    input_stream: TextIO = sys.stdin,
    output_stream: TextIO = sys.stdout,
) -> int:
    if not input_stream.isatty() or not output_stream.isatty():
        raise ValueError("interactive mode requires a terminal")

    state = InteractiveState()
    fd = input_stream.fileno()
    previous = termios.tcgetattr(fd)
    output_stream.write("\x1b[?1049h\x1b[?25l")
    output_stream.flush()
    try:
        tty.setcbreak(fd)
        while state.running:
            size = shutil.get_terminal_size((110, 30))
            content = render_content(state.active_view, size.columns, color)
            output_stream.write(
                render_interactive_screen(
                    state,
                    content,
                    width=size.columns,
                    height=size.lines,
                    color=color,
                )
            )
            output_stream.flush()
            action = handle_key(state, _read_key(fd))
            if action == "refresh":
                try:
                    refresh()
                except (OSError, ValueError) as error:
                    state.message = f"Refresh failed: {error}"
                else:
                    state.message = "Usage refreshed"
                    state.show_help = False
    except KeyboardInterrupt:
        pass
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, previous)
        output_stream.write("\x1b[?25h\x1b[?1049l")
        output_stream.flush()
    return 0


def _handle_command_key(state: InteractiveState, key: str) -> str | None:
    suggestions = _command_suggestions(state.command_text)
    if key in ("up", "down"):
        if suggestions:
            delta = -1 if key == "up" else 1
            state.command_selected = (state.command_selected + delta) % len(suggestions)
        return None
    if key == "enter":
        state.command_mode = False
        if suggestions:
            selected = min(state.command_selected, len(suggestions) - 1)
            text = suggestions[selected].name
        else:
            text = state.command_text
        state.command_text = ""
        state.command_selected = 0
        return parse_slash_command(state, text)
    if key == "escape":
        return _close_command_palette(state)
    if key == "backspace":
        state.command_text = state.command_text[:-1]
        state.command_selected = 0
        return None
    if key == "ctrl_c":
        state.running = False
        return "quit"
    if key == "space" and len(state.command_text) < 48:
        state.command_text += " "
        state.command_selected = 0
        return None
    if len(key) == 1 and key.isprintable() and len(state.command_text) < 48:
        state.command_text += key
        state.command_selected = 0
    return None


def _close_command_palette(state: InteractiveState) -> None:
    state.command_mode = False
    state.command_text = ""
    state.command_selected = 0
    state.message = _label_for(state.active_view)
    return None


def _activate(state: InteractiveState, key: str) -> str | None:
    if key == "quit":
        state.running = False
        return "quit"
    if key == "refresh":
        return "refresh"
    if key == "help":
        state.show_help = True
        state.message = "Help"
        state.selected = _menu_index(key)
        return None
    state.active_view = key
    state.selected = _menu_index(key)
    state.show_help = False
    state.message = _label_for(key)
    return "view"


def _menu_index(key: str) -> int:
    return next((index for index, item in enumerate(MENU_ITEMS) if item.key == key), 0)


def _label_for(key: str) -> str:
    return next((item.label for item in MENU_ITEMS if item.key == key), key)


def _menu_lines(state: InteractiveState, width: int, color: bool) -> list[str]:
    tokens: list[str] = []
    for index, item in enumerate(MENU_ITEMS):
        marker = "▶" if index == state.selected else "●" if item.key == state.active_view else " "
        style = CYAN if index == state.selected else GREEN if item.key == state.active_view else MUTED
        tokens.append(_style(f"{marker} {item.label}", style, color))

    lines: list[str] = []
    current: list[str] = []
    current_width = 0
    for token, item in zip(tokens, MENU_ITEMS):
        token_width = len(item.label) + 2
        separator = 3 if current else 0
        if current and current_width + separator + token_width > width:
            lines.append(" · ".join(current))
            current = []
            current_width = 0
            separator = 0
        current.append(token)
        current_width += separator + token_width
    if current:
        lines.append(" · ".join(current))
    return lines


def _command_suggestions(text: str) -> tuple[CommandItem, ...]:
    query = " ".join(text.strip().lower().split())
    if not query:
        return COMMAND_ITEMS
    return tuple(item for item in COMMAND_ITEMS if query in item.name)


def _command_palette_lines(
    state: InteractiveState,
    width: int,
    color: bool,
    *,
    page_size: int = 5,
) -> list[str]:
    suggestions = _command_suggestions(state.command_text)
    if not suggestions:
        return [
            _style("Commands · no matches · Backspace to edit"[:width], YELLOW, color),
        ]

    selected = min(state.command_selected, len(suggestions) - 1)
    page_start = selected // page_size * page_size
    page = suggestions[page_start : page_start + page_size]
    page_end = page_start + len(page)
    lines = [
        _style(
            f"Commands · {page_start + 1}-{page_end} of {len(suggestions)} · ↑/↓ choose · Enter run · Esc back"[:width],
            MUTED,
            color,
        )
    ]
    for offset, item in enumerate(page):
        index = page_start + offset
        marker = "▶" if index == selected else " "
        style = CYAN if index == selected else LIGHT
        lines.append(_style(f"{marker} /{item.name:<14}  {item.description}"[:width], style, color))
    return lines


def _help_text(width: int) -> str:
    lines = [
        "Keyboard",
        "  ↑ ↓ ← →   choose a menu item",
        "  Enter/Space open the selected item",
        "  /           type a slash command",
        "  r           refresh local Codex records",
        "  Esc         close help or slash commands",
        "  q           quit from the main screen",
        "",
        "Slash commands",
        "  /today  /week  /month  /all",
        "  /history day  /history week  /history month",
        "  /network  /refresh  /help  /quit",
    ]
    return "\n".join(line[:width] for line in lines)


def _read_key(fd: int) -> str:
    value = os.read(fd, 1)
    if value == b"\x1b":
        if not select.select([fd], [], [], 0.03)[0]:
            return "escape"
        prefix = os.read(fd, 1)
        if prefix != b"[" or not select.select([fd], [], [], 0.03)[0]:
            return "escape"
        suffix = prefix + os.read(fd, 1)
        return {
            b"[A": "up",
            b"[B": "down",
            b"[C": "right",
            b"[D": "left",
        }.get(suffix, "escape")
    if value in (b"\r", b"\n"):
        return "enter"
    if value == b" ":
        return "space"
    if value in (b"\x7f", b"\x08"):
        return "backspace"
    if value == b"\x03":
        return "ctrl_c"
    try:
        return value.decode("utf-8")
    except UnicodeDecodeError:
        return ""


def _style(text: str, style: str, color: bool) -> str:
    return f"{style}{text}{RESET}" if color else text
