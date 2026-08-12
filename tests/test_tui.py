from __future__ import annotations

import os
import unittest

from codex_meter.interactive import (
    COMMAND_ITEMS,
    InteractiveState,
    _read_key,
    _sync_projects,
    handle_key,
    parse_slash_command,
    render_interactive_screen,
)
from codex_meter.tui import render_history, render_network, render_overview


class TuiTests(unittest.TestCase):
    def test_empty_dashboard_uses_na_not_invented_metrics(self) -> None:
        rendered = render_overview({}, [], period="TEST", color=False, width=90)
        self.assertIn("API-EQUIV", rendered)
        self.assertIn("N/A", rendered)
        self.assertIn("Cache miss", rendered)
        self.assertIn("No imported usage", rendered)
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
