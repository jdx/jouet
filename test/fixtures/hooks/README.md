Hook payloads for `jouet hook cursor|gemini|copilot|codex`, one per event
(and one per kind of call where an event takes several), in the fields each
harness's documentation gives (Cursor: cursor.com/docs/hooks; Gemini CLI:
geminicli.com/docs/hooks and the payload its hook runner builds; Copilot:
docs.github.com/en/copilot/reference/hooks-reference; Codex:
developers.openai.com/codex/hooks and the input schemas in
codex-rs/hooks/schema/generated). The tests set `cwd` and the command where
they need their own; `tests/graph.rs` puts its work tree in place of
`/work/app`.
