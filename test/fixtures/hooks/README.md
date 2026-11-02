Hook payloads for `jouet hook cursor|gemini|copilot`, one per event, in the
fields each harness's documentation gives (Cursor: cursor.com/docs/hooks;
Gemini CLI: geminicli.com/docs/hooks and the payload its hook runner builds;
Copilot: docs.github.com/en/copilot/reference/hooks-reference). The tests
set `cwd` and the command where they need their own.

`claude/` holds Claude Code's, for a subagent's calls (the Agent tool) and the
lead's stop, in the fields of Claude Code's hook input schema
(code.claude.com/docs/en/hooks): a subagent's carry the lead's `session_id`
and their own `agent_id` and `agent_type`; the lead's have no `agent_id`.
