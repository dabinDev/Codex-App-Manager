# Codex model metadata

`codex-models-0.164.0.json` is the unmodified model catalog from OpenAI Codex
release `rust-v0.164.0` (retrieved 2026-10-24):

https://github.com/openai/codex/blob/rust-v0.164.0/codex-rs/models-manager/models.json

Copyright OpenAI. Distributed under the Apache License 2.0; see
`LICENSE-codex` in this directory. Preserve this source/version notice when
updating the catalog. The manager selects matching entries, preserving their
tool and instruction metadata. Unknown GPT IDs receive conservative defaults
instead of inheriting another model's capabilities.
