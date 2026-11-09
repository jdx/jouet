import json
import unittest

from jbench import timing


def event(kind, item_id, item_type="commandExecution"):
    return json.dumps({"type": kind, "item": {"type": item_type, "id": item_id}})


class TimingTest(unittest.TestCase):
    def test_unverifiable_command_streams_name_the_gap(self):
        shell = {"completed": 1, "failed": 0}
        complete = {"stream_complete": True, "unmatched_starts": 0,
                    "unmatched_completions": 0, "duplicate_starts": 0}
        self.assertIsNone(timing.verification_issue(complete, shell))
        self.assertEqual(timing.verification_issue(None, shell), "Codex command timing unavailable")
        self.assertEqual(timing.verification_issue({**complete, "stream_complete": False}, shell),
                         "Codex event stream was incomplete")
        self.assertEqual(timing.verification_issue({**complete, "unmatched_starts": 1}, None),
                         "agent command had no completion event")
        for field in ("unmatched_completions", "duplicate_starts"):
            self.assertEqual(timing.verification_issue({**complete, field: 1}, shell),
                             "Codex command events could not be matched")
        self.assertEqual(timing.verification_issue(complete, None), "Codex command event stream could not be verified")

    def test_overlapping_commands_count_once_and_incomplete_events_are_visible(self):
        lines = [event("item.started", "a"), event("item.started", "b"),
                 event("item.completed", "a"), event("item.completed", "b"),
                 event("item.started", "unfinished"), event("item.started", "unfinished"),
                 event("item.completed", "missing"),
                 event("item.started", "reason", "reasoning")]
        result = timing.codex_commands(lines, [1, 2, 3, 5, 6, 7, 8, 9], False)
        self.assertEqual(result, {"observed_command_active_s": 4, "completed_intervals": 2,
                                  "coalesced_intervals": 0,
                                  "unmatched_starts": 1, "unmatched_completions": 1,
                                  "duplicate_starts": 1,
                                  "stream_complete": False})

    def test_misaligned_sidecar_is_not_used(self):
        self.assertIsNone(timing.codex_commands([event("item.started", "a")], [], True))

    def test_commands_received_together_are_flagged_as_ambiguous(self):
        lines = [event("item.started", "a"), event("item.completed", "a")]
        result = timing.codex_commands(lines, [1, 1.00001], True)
        self.assertEqual(result["coalesced_intervals"], 1)

    def test_duplicate_start_does_not_shorten_the_interval(self):
        lines = [event("item.started", "a"), event("item.started", "a"),
                 event("item.completed", "a")]
        result = timing.codex_commands(lines, [1, 4, 5], True)
        self.assertEqual(result["observed_command_active_s"], 4)
        self.assertEqual(result["duplicate_starts"], 1)
