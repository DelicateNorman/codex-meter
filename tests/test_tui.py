from __future__ import annotations

import os
import unittest

from codex_meter.interactive import (
    COMMAND_ITEMS,
    InteractiveState,
    _display_width,
    _project_picker_text,
    _read_key,
    _sync_projects,
    handle_key,
    parse_slash_command,
    render_interactive_screen,
)
from codex_meter.tui import _display_width as tui_display_width
from codex_meter.tui import render_history, render_network, render_overview


class TuiTests(unittest.TestCase):
    def test_empty_dashboard_uses_na_not_invented_metrics(self) -> None:
        rendered = render_overview({}, [], period="TEST", color=False, width=90)
        self.assertIn("API-EQUIV", rendered)
        self.assertIn("N/A", rendered)
        self.assertIn("Cache miss", rendered)
        self.assertIn("No Codex usage found", rendered)
        self.assertIn("press r here to refresh", rendered)
        self.assertNotIn("\x1b", rendered)

    def test_arrow_enter_and_space_navigate_views(self) -> None:
        state = InteractiveState()
        self.assertIsNone(handle_key(state, "right"))
        self.assertEqual(state.selected, 1)
        self.assertEqual(handle_key(state, "enter"), "view")
        self.assertEqual(state.active_view, "week")

        handle_key(state, "down")
        self.assertEqual(handle_key(state, "space"), "view")
        self.assertEqual(state.active_view, "month")

    def test_fast_arrow_and_enter_remain_two_keys(self) -> None:
        read_fd, write_fd = os.pipe()
        try:
            os.write(write_fd, b"\x1b[C\r")
            self.assertEqual(_read_key(read_fd), "right")
            self.assertEqual(_read_key(read_fd), "enter")
        finally:
            os.close(read_fd)
            os.close(write_fd)

    def test_key_reader_accepts_multibyte_utf8_input(self) -> None:
        read_fd, write_fd = os.pipe()
        try:
            os.write(write_fd, "项目".encode("utf-8"))
            self.assertEqual(_read_key(read_fd), "项")
            self.assertEqual(_read_key(read_fd), "目")
        finally:
            os.close(read_fd)
            os.close(write_fd)

    def test_slash_commands_switch_refresh_and_quit(self) -> None:
        state = InteractiveState()
        self.assertEqual(parse_slash_command(state, "/all"), "view")
        self.assertEqual(state.active_view, "all")
        self.assertEqual(parse_slash_command(state, "history month"), "view")
        self.assertEqual(state.active_view, "history_month")
        self.assertEqual(parse_slash_command(state, "network"), "view")
        self.assertEqual(state.active_view, "network")
        self.assertEqual(parse_slash_command(state, "project"), "project_picker")
        self.assertTrue(state.project_picker)
        handle_key(state, "escape")
        self.assertEqual(parse_slash_command(state, "refresh"), "refresh")
        self.assertEqual(parse_slash_command(state, "quit"), "quit")
        self.assertFalse(state.running)

    def test_slash_input_mode_accepts_text_and_enter(self) -> None:
        state = InteractiveState()
        handle_key(state, "/")
        for character in "week":
            handle_key(state, character)
        self.assertEqual(handle_key(state, "enter"), "view")
        self.assertEqual(state.active_view, "week")
        self.assertFalse(state.command_mode)

        handle_key(state, "/")
        for key in ("h", "i", "s", "t", "o", "r", "y", "space", "m", "o", "n", "t", "h"):
            handle_key(state, key)
        self.assertEqual(handle_key(state, "enter"), "view")
        self.assertEqual(state.active_view, "history_month")

    def test_slash_palette_uses_arrows_and_enter(self) -> None:
        state = InteractiveState()
        handle_key(state, "/")
        handle_key(state, "down")
        handle_key(state, "down")
        self.assertEqual(state.command_selected, 2)
        self.assertEqual(handle_key(state, "enter"), "view")
        self.assertEqual(state.active_view, "month")

    def test_slash_palette_uses_q_as_text_and_escape_returns(self) -> None:
        state = InteractiveState()
        handle_key(state, "/")
        self.assertTrue(state.command_mode)
        self.assertIsNone(handle_key(state, "q"))
        self.assertEqual(state.command_text, "q")
        self.assertTrue(state.running)
        self.assertIsNone(handle_key(state, "escape"))
        self.assertFalse(state.command_mode)
        self.assertTrue(state.running)
        self.assertEqual(handle_key(state, "q"), "quit")
        self.assertFalse(state.running)

        state = InteractiveState()
        handle_key(state, "/")
        handle_key(state, "n")
        self.assertIsNone(handle_key(state, "q"))
        self.assertEqual(state.command_text, "nq")
        self.assertTrue(state.running)

    def test_slash_palette_automatically_pages_and_shows_descriptions(self) -> None:
        state = InteractiveState()
        handle_key(state, "/")
        for _ in range(5):
            handle_key(state, "down")
        rendered = render_interactive_screen(
            state,
            "dashboard",
            width=100,
            height=30,
            color=False,
            clear=False,
        )
        self.assertIn("Commands · 6-10 of 12", rendered)
        self.assertIn("▶ /history week", rendered)
        self.assertIn("Show weekly usage history", rendered)
        self.assertTrue(all(item.description.isascii() for item in COMMAND_ITEMS))
        self.assertNotIn("dashboard", rendered)
        self.assertLessEqual(len(rendered.splitlines()), 30)

    def test_project_order_from_storage_is_preserved_and_deduplicated(self) -> None:
        state = InteractiveState(project_filter="older")
        _sync_projects(state, ["recent", "older", "recent", ""])
        self.assertEqual(state.project_options, ("recent", "older"))
        self.assertEqual(state.project_selected, 2)

    def test_project_picker_applies_scope_and_preserves_it_across_views(self) -> None:
        state = InteractiveState(project_options=("alpha", "beta"))
        self.assertEqual(parse_slash_command(state, "project"), "project_picker")
        self.assertTrue(state.project_picker)
        handle_key(state, "down")
        self.assertEqual(handle_key(state, "enter"), "project")
        self.assertEqual(state.project_filter, "alpha")
        self.assertFalse(state.project_picker)
        self.assertEqual(state.active_view, "today")

        handle_key(state, "right")
        self.assertEqual(handle_key(state, "enter"), "view")
        self.assertEqual(state.active_view, "week")
        self.assertEqual(state.project_filter, "alpha")

        rendered = render_interactive_screen(
            state, "project dashboard", width=100, height=30, color=False, clear=False,
        )
        self.assertIn("Scope · alpha", rendered)

    def test_project_picker_filters_as_user_types(self) -> None:
        state = InteractiveState(project_options=("codex-stats", "Earth-Agent", "中文项目"))
        parse_slash_command(state, "project")
        for character in "earth":
            handle_key(state, character)
        rendered = _project_picker_text(state, 80)
        self.assertEqual(state.project_query, "earth")
        self.assertIn("Earth-Agent", rendered)
        self.assertNotIn("codex-stats", rendered)
        self.assertNotIn("All projects", rendered)
        self.assertEqual(handle_key(state, "enter"), "project")
        self.assertEqual(state.project_filter, "Earth-Agent")
        self.assertEqual(state.project_query, "")

    def test_project_picker_supports_unicode_filter_and_no_match_recovery(self) -> None:
        state = InteractiveState(project_options=("codex-stats", "中文项目"))
        parse_slash_command(state, "project")
        handle_key(state, "中")
        self.assertIn("中文项目", _project_picker_text(state, 80))
        handle_key(state, "backspace")
        for character in "missing":
            handle_key(state, character)
        self.assertIn("no matches", _project_picker_text(state, 80))
        self.assertIsNone(handle_key(state, "enter"))
        self.assertTrue(state.project_picker)
        self.assertEqual(state.message, "No projects match the filter")
        for _ in "missing":
            handle_key(state, "backspace")
        self.assertIn("All projects", _project_picker_text(state, 80))

    def test_unicode_project_names_respect_terminal_column_width(self) -> None:
        state = InteractiveState(project_options=("非常长的中文项目名称" * 4,))
        parse_slash_command(state, "project")
        rendered = _project_picker_text(state, 40)
        self.assertTrue(all(_display_width(line) <= 40 for line in rendered.splitlines()))

        overview = render_overview(
            {}, [], period="DAY · PROJECT " + "中文项目" * 20, color=False, width=80,
        )
        self.assertTrue(all(tui_display_width(line) <= 80 for line in overview.splitlines()))

    def test_project_picker_treats_q_as_filter_text_and_escape_cancels(self) -> None:
        state = InteractiveState(project_options=("q-project",))
        parse_slash_command(state, "project")
        self.assertIsNone(handle_key(state, "q"))
        self.assertEqual(state.project_query, "q")
        self.assertTrue(state.running)
        self.assertIsNone(handle_key(state, "escape"))
        self.assertFalse(state.project_picker)
        self.assertEqual(state.project_query, "")

    def test_project_picker_accepts_spaces_and_restores_current_selection(self) -> None:
        state = InteractiveState(
            project_options=("recent", "my project", "older"),
            project_filter="older",
        )
        parse_slash_command(state, "project")
        self.assertEqual(state.project_selected, 3)
        for key in ("m", "y", "space", "p"):
            handle_key(state, key)
        self.assertEqual(state.project_query, "my p")
        self.assertIn("my project", _project_picker_text(state, 80))
        for _ in "my p":
            handle_key(state, "backspace")
        self.assertEqual(state.project_query, "")
        self.assertEqual(state.project_selected, 3)

    def test_interactive_screen_explains_keyboard_controls(self) -> None:
        rendered = render_interactive_screen(
            InteractiveState(),
            "dashboard",
            width=100,
            height=30,
            color=False,
            clear=False,
        )
        self.assertIn("INTERACTIVE", rendered)
        self.assertIn("Enter/Space", rendered)
        self.assertIn("/ commands", rendered)
        self.assertIn("dashboard", rendered)
        self.assertLess(rendered.index("dashboard"), rendered.index("▶ Today"))
        self.assertNotIn("\x1b", rendered)

    def test_help_escape_returns_cursor_to_active_view(self) -> None:
        state = InteractiveState(active_view="month", selected=2, message="Month")
        handle_key(state, "?")
        self.assertTrue(state.show_help)
        state.selected = 10
        self.assertIsNone(handle_key(state, "escape"))
        self.assertFalse(state.show_help)
        self.assertEqual(state.selected, 2)
        self.assertEqual(state.message, "Month")

    def test_refresh_shortcut_closes_help_before_refresh(self) -> None:
        state = InteractiveState(active_view="week", selected=10, show_help=True, message="Help")
        self.assertEqual(handle_key(state, "r"), "refresh")
        self.assertFalse(state.show_help)

    def test_wide_short_screen_wraps_menu_and_preserves_panel_bottom(self) -> None:
        body = "\n".join([
            "╭" + "─" * 20 + "╮",
            *[f"│ row {index:<14} │" for index in range(20)],
            "╰" + "─" * 20 + "╯",
        ])
        rendered = render_interactive_screen(
            InteractiveState(active_view="month", selected=2, message="Month"),
            body,
            width=180,
            height=18,
            color=False,
            clear=False,
        )
        lines = rendered.splitlines()
        self.assertIn("terminal too short", rendered)
        self.assertIn("╰────────────────────╯", rendered)
        self.assertIn("Project", rendered)
        self.assertTrue(all(len(line) <= 132 for line in lines))
        self.assertNotEqual(lines[-1], "● Month")

    def test_slash_palette_fits_a_twelve_line_terminal(self) -> None:
        state = InteractiveState()
        handle_key(state, "/")
        rendered = render_interactive_screen(
            state,
            "dashboard should be hidden",
            width=80,
            height=12,
            color=False,
            clear=False,
        )
        self.assertLessEqual(len(rendered.splitlines()), 12)
        self.assertIn("View today's usage", rendered)
        self.assertNotIn("dashboard should be hidden", rendered)

    def test_short_dashboard_keeps_summary_instead_of_separator_rows(self) -> None:
        body = "\n".join((
            "╭" + "─" * 20 + "╮",
            "│ OVERVIEW             │",
            "├" + "─" * 20 + "┤",
            "│ TOKENS 123           │",
            "├" + "─" * 20 + "┤",
            "│ details              │",
            "╰" + "─" * 20 + "╯",
        ))
        rendered = render_interactive_screen(
            InteractiveState(),
            body,
            width=80,
            height=12,
            color=False,
            clear=False,
        )
        self.assertIn("TOKENS 123", rendered)
        self.assertIn("terminal too short", rendered)
        self.assertIn("╰────────────────────╯", rendered)

    def test_narrow_terminal_uses_compact_navigation_without_overflow(self) -> None:
        rendered = render_interactive_screen(
            InteractiveState(),
            "x" * 100,
            width=40,
            height=12,
            color=False,
            clear=False,
        )
        lines = rendered.splitlines()
        self.assertLessEqual(len(lines), 12)
        self.assertTrue(all(len(line) <= 40 for line in lines))
        self.assertIn("40 columns available · 80 required", rendered)
        self.assertIn("Menu 1/12 · ▶ Today", rendered)

    def test_project_picker_adapts_page_size_to_short_terminals(self) -> None:
        state = InteractiveState(project_options=tuple(f"project-{index}" for index in range(10)))
        parse_slash_command(state, "project")
        body = _project_picker_text(state, 40, page_size=3)
        rendered = render_interactive_screen(
            state,
            body,
            width=40,
            height=12,
            color=False,
            clear=False,
        )
        self.assertLessEqual(len(rendered.splitlines()), 12)
        self.assertIn("Projects 1-3/11 · ↑/↓ · Enter · Esc", rendered)
        self.assertIn("All projects", rendered)
        self.assertNotIn("project-2", rendered)

    def test_history_renderer_is_compact_and_handles_unknown_cost(self) -> None:
        rendered = render_history(
            [{
                "period_start": "2026-08-12",
                "sessions": 2,
                "turns": 4,
                "calls": 5,
                "input_tokens": 100,
                "cached_input_tokens": 50,
                "total_tokens": 120,
                "cost_usd": None,
            }],
            group="day",
            username="test-user",
            color=False,
        )
        self.assertIn("test-user", rendered)
        self.assertIn("50.0%", rendered)
        self.assertIn("N/A", rendered)

    def test_network_renderer_shows_latency_and_estimated_token_speed(self) -> None:
        rendered = render_network(
            [{
                "local_time": "12:34:56",
                "model": "gpt-test",
                "output_tokens": 200,
                "ttft_ms": 1000,
                "e2e_ms": 5000,
                "exact_output_tps": None,
            }],
            [],
            period="DAY · 2026-08-12",
            username="test-user",
            color=False,
            width=100,
        )
        self.assertIn("NETWORK & RESPONSE", rendered)
        self.assertIn("First token", rendered)
        self.assertIn("50.0*", rendered)
        self.assertIn("no probe/capture samples", rendered)


if __name__ == "__main__":
    unittest.main()
