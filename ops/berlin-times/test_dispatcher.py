import importlib.util
import unittest
from datetime import datetime, timezone
from pathlib import Path


SPEC = importlib.util.spec_from_file_location(
    "dispatcher", Path(__file__).with_name("dispatcher.py")
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("could not load dispatcher")
DISPATCHER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DISPATCHER)


class DispatcherTest(unittest.TestCase):
    def test_builds_slot_in_berlin_time(self):
        now = datetime(2026, 8, 11, 22, 30, tzinfo=timezone.utc)
        self.assertEqual(DISPATCHER.edition_slot("morning", now), "2026-08-12-morning")

    def test_reads_published_slot_in_berlin_time(self):
        document = {
            "edition_name": "evening",
            "generated_at": "2026-08-11T15:58:58.521825097Z",
        }
        self.assertEqual(DISPATCHER.published_slot(document), "2026-08-11-evening")

    def test_rejects_invalid_published_metadata(self):
        with self.assertRaisesRegex(
            DISPATCHER.DispatcherError, "published edition metadata is invalid"
        ):
            DISPATCHER.published_slot({"edition_name": "late", "generated_at": None})

    def test_validates_generic_deployment_configuration(self):
        self.assertEqual(
            DISPATCHER.validate_repository("example-owner/example-repository"),
            "example-owner/example-repository",
        )
        self.assertEqual(
            DISPATCHER.validate_edition_url(
                "https://example-owner.github.io/example-repository/edition.json"
            ),
            "https://example-owner.github.io/example-repository/edition.json",
        )

    def test_rejects_credentialed_edition_url(self):
        with self.assertRaisesRegex(
            DISPATCHER.DispatcherError, "edition url must be credential-free https"
        ):
            DISPATCHER.validate_edition_url("https://user@example.com/edition.json")


if __name__ == "__main__":
    unittest.main()
