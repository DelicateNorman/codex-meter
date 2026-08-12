from __future__ import annotations

import unittest

from codex_meter.doctor import _otel_exporter_name


class DoctorTests(unittest.TestCase):
    def test_otel_details_never_expose_headers(self) -> None:
        exporter = {"otlp-http": {"endpoint": "http://localhost", "headers": {"Authorization": "secret"}}}
        rendered = _otel_exporter_name(exporter)
        self.assertEqual(rendered, "otlp-http")
        self.assertNotIn("secret", rendered)


if __name__ == "__main__":
    unittest.main()
