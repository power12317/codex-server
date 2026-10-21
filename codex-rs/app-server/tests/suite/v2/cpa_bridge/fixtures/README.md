# Inference tool fixtures

`workbuddy-tools.json` contains the exact 22 Chat Completions function tool
declarations from the WorkBuddy incident recorded on 2026-10-16. It excludes the
request's business messages, HTTP headers and credentials. The source request log
has SHA-256 `cb1ae02e48e4d68a628fd6b8e1bb87ede1a5b4c05b3a55d3521f080cec408b4d`.

The schema regression sends both the nested Chat tool envelopes and normalized
Responses declarations through a real master and worker to a local HTTP mock.
It compares every tool and schema value at the received model request, including
all 41 fields lost in the original incident. Both Responses and Responses Lite
are covered. The next request replays a client function call/result pair, while
the returned command must never run locally.

Synthetic unit tests cover additional recursive schema forms and future keywords
that are absent from the incident sample. They exercise the complete schema
constructor used by inference, rather than the internal tool normalization
policy's deliberately limited deserializer.
