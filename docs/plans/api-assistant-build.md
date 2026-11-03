# Built-in assistant chat: build plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A chat pane in the app that drives the DAW through the Anthropic Messages API with the owner's key, alongside the unchanged MCP server.

**Architecture:** A new crate `daw-claude` holds everything that doesn't need the app: the SSE reader, the request builder, prices and the spending guard, and the job loop that talks to the API and runs tools through a trait. `daw-app` adds the key store, the per-project chat history, a controller that runs the job loop on a worker thread and answers its tool calls through the existing `agent.inbox`, and the `Assistant` pane on the right.

**Tech Stack:** Rust 2024 workspace, `ureq` 3 (already a dependency), `serde_json`, `keyring` 3 (new), egui 0.37 / `egui_kittest` for UI tests.

**Spec:** [api-assistant.md](api-assistant.md). Read it first; this plan argues from it.

Work on a branch (`api-assistant`), not `main`. Builds on Windows need `LIBCLANG_PATH` set (see README); `run.ps1` sets it.

## Global Constraints

- API: `POST https://api.anthropic.com/v1/messages` and `GET /v1/models`, header `anthropic-version: 2026-10-21`, key in `x-api-key`. Never send `temperature`, `top_p` or `top_k` (400 on this version). `tool_choice` is left out (auto); never `any` or `tool`.
- Every chat starts on `claude-haiku-5-5`, including chats reopened from History. The chosen model is never saved anywhere.
- Spending limits: **$5 per job, $20 per chat**, editable, remembered. No request starts once a limit is passed until the owner says continue. A model with no price is priced at the most expensive known rate.
- **No `task_budget`.** (Change from the spec, see Task 5 and the handoff note: Anthropic's guidance is not to use task budgets on interactive sessions, because their countdown after each tool result makes the model distrust text typed mid-job, which is what "Send now" is.)
- History is append-only within a chat: every content block the API returns (text, thinking with its signature, tool_use, server tool blocks, compaction) is stored and replayed byte-for-byte. Nothing before the newest turn is ever edited.
- Send every request with `thinking.block_binding.prefix_mismatch_behavior: "drop_block"` (beta `thinking-binding-controls-2026-08-01`), so a changed tool set (web search toggle, model switch, an app update changing a schema) drops stale thinking blocks instead of failing.
- "Send now" text goes as a `text` block **after the last `tool_result`** in the user message that carries the results. Never inside a `tool_result`, never as a system message.
- The API key lives only in the OS credential store or `ANTHROPIC_API_KEY`. Never in settings files, chat history, projects, logs, crash reports, or `Debug` output.
- Chat history: per project, in the app's data folder only (`config_dir()`), never in the project folder. Unsaved projects keep their chat in memory only.
- No test calls the real API. The HTTP transport is a trait; tests use canned SSE fixtures.
- Don't copy `timeout_global(20s)` from `update.rs`: streams run for minutes. Use a connect timeout and a per-read idle timeout (90 s) instead.
- Every new egui control needs an `// ai:` mark (`crates/app/src/widget_check.rs`). Chat and key controls are `// ai: person`; pane/tab/toggle-view controls are `// ai: view`.
- `Pane::Assistant` has id `"assistant"`, which is saved in `layout.txt` and must never change.
- Keep the MCP server, its tools and its behaviour unchanged.

## Review Focus

1. **Project switched or closed mid-job:** the job stops and no tool call lands on the new project. Test in Task 8.
2. **Stop pressed after a `tool_use` streamed but before its result:** the saved history is still a valid conversation (unanswered calls get `is_error` results "Stopped by the person."). Test in Task 5.
3. **Prices at the edges:** Haiku 5.5 above 100K prompt tokens, a model missing from the table, a refusal fallback served by another model. The guard must hold in each. Tests in Task 2.
4. **429 / 529 / dropped connection mid-job:** retried with backoff, usage counted once per attempt (never twice), history intact. Test in Task 4.
5. **App quits mid-job:** the history on disk is only ever saved at consistent points, so it reopens as a valid conversation. Test in Task 7.

---

### Task 1: `daw-claude` crate, SSE reader and message accumulator

**Files:**
- Create: `crates/claude/Cargo.toml`, `crates/claude/src/lib.rs`, `crates/claude/src/sse.rs`, `crates/claude/src/stream.rs`
- Create: `crates/claude/tests/fixtures/text.sse`, `tool_use.sse`, `thinking.sse`, `web_search.sse`, `error.sse`
- Modify: `Cargo.toml` (workspace members)

**Interfaces:**
- Produces:
  - `sse::Events<R: BufRead>`: `Iterator<Item = io::Result<(String, String)>>` of `(event, data)`; `sse::events(r: R) -> Events<R>`.
  - `stream::Accumulator::new() -> Accumulator`; `feed(&mut self, data: &serde_json::Value) -> Result<Option<Delta>, StreamError>`; `finish(self) -> Result<Message, StreamError>`.
  - `stream::Delta { Text { index: usize, text: String }, Thinking { index: usize, text: String }, BlockStart { index: usize, kind: String }, BlockStop { index: usize } }`.
  - `stream::Message { model: String, content: Vec<Value>, stop_reason: Option<String>, usage: Value }` — `content` is the exact blocks to store, `usage` the merged usage object (from `message_start` then `message_delta`).
  - `stream::StreamError { Api { kind: String, message: String }, Bad(String) }` (an `event: error` becomes `Api`).

Copy the SPDX header and crate layout from `crates/agent` (deps: `serde_json` only). Fixtures are hand-written SSE in the format from the streaming docs (`https://platform.claude.com/docs/en/build-with-claude/streaming.md`): `message_start`, `content_block_start`, `content_block_delta` (`text_delta`, `thinking_delta`, `signature_delta`, `input_json_delta`), `content_block_stop`, `message_delta`, `message_stop`, `ping`.

- [ ] **Step 1: Write the failing tests** in `stream.rs`:
  - `text_fixture_gives_one_text_block`: `content == [{"type":"text","text":"Hello there"}]`, `stop_reason == Some("end_turn")`, `usage["output_tokens"] == 12`.
  - `tool_use_input_is_parsed_from_json_deltas`: the `tool_use` block's `input` is the parsed object `{"track":"Bass","bars":12}`, not a string; `stop_reason == Some("tool_use")`.
  - `thinking_keeps_its_signature`: the thinking block has both `thinking` text and `signature` exactly as streamed.
  - `unknown_block_types_are_kept_verbatim`: a `server_tool_use` and a `web_search_tool_result` block (in `web_search.sse`) come out equal to their `content_block_start` payloads (plus any `input_json_delta`).
  - `error_event_is_an_api_error`: `error.sse` → `Err(StreamError::Api { kind: "overloaded_error", .. })`.
  - `sse_handles_crlf_and_multiline_data` in `sse.rs`.
- [ ] **Step 2: Run** `cargo test -p daw-claude` — expected: FAIL (types not defined).
- [ ] **Step 3: Implement** `sse.rs` and `stream.rs`. Blocks are kept as `serde_json::Value` objects; `input_json_delta` text is buffered per index and parsed at `content_block_stop` (an empty buffer means `{}`). A delta type the accumulator doesn't know is an error in debug builds (`debug_assert!`) and ignored in release, so a new API delta shows up in tests.
- [ ] **Step 4: Run** `cargo test -p daw-claude` — expected: PASS.
- [ ] **Step 5: Commit** `feat(claude): SSE reader and message accumulator`.

### Task 2: Prices and the spending guard

**Files:**
- Create: `crates/claude/src/price.rs`, `crates/claude/src/guard.rs`

**Interfaces:**
- Consumes: `stream::Message.usage` (a `Value`).
- Produces:
  - `price::Usage { input: u64, output: u64, cache_read: u64, cache_write_5m: u64, cache_write_1h: u64, web_searches: u64 }`; `Usage::from_api(usage: &Value) -> Usage`.
  - `price::cost(model: &str, u: &Usage) -> Cost`; `Cost { dollars: f64, known: bool }` (`known` false when the model isn't in the table and the dearest rate was used).
  - `guard::Limits { job: f64, chat: f64 }`, `Limits::default()` = `{ job: 5.0, chat: 20.0 }`.
  - `guard::Guard::new(limits: Limits, chat_spent: f64) -> Guard`; `start_job(&mut self)`; `add(&mut self, dollars: f64)`; `check(&self) -> Verdict`; `extend(&mut self, scope: Scope)`; `job_spent(&self) -> f64`; `chat_spent(&self) -> f64`.
  - `guard::Verdict { Go, Pause { scope: Scope, spent: f64 } }`, `guard::Scope { Job, Chat }`.

The table holds per-million-token rates for input, output, cache read, 5-minute and 1-hour cache writes, and the web search fee per search. Take every number from the pricing page (`https://platform.claude.com/docs/en/about-claude/pricing.md`) when you write it, and put the date you read it in a comment. Known so far: Haiku 5.5 $0.10 / $0.50 up to 100K prompt tokens, $0.50 / $2.50 above; Sonnet 5.5 $2 / $10; Opus 5.5 $4 / $20 (cache read $0.20); Fable 5.1 $10 / $50 (cache read $0.25). Check the usage field that counts searches on the web search docs page (expected `usage.server_tool_use.web_search_requests`). For the 100K tier, the prompt size is `input + cache_read + cache_write_5m + cache_write_1h`. If `usage.iterations` is present (a refusal fallback ran), price each iteration by its own model; check the field names on the refusals docs page.

`extend(Scope::Job)` raises the job allowance by another `limits.job`; `extend(Scope::Chat)` raises the chat allowance by `limits.chat`. Chat is checked before job.

- [ ] **Step 1: Write the failing tests:**
  - `haiku_small_prompt`: 10,000 input + 1,000 output on `claude-haiku-5-5` → `0.0015` dollars (±1e-9), `known`.
  - `haiku_long_prompt_uses_higher_tier`: 150,000 input + 1,000 output → `0.075 + 0.0025 = 0.0775`.
  - `unknown_model_priced_at_dearest_rate`: `cost("claude-future-9", &u)` equals Fable 5.1's cost for `u`, `known == false`.
  - `web_searches_are_charged`: a usage with `web_searches: 3` costs 3 × the fee more than with 0.
  - `fallback_iterations_priced_per_model` (fixture usage with two iterations on different models).
  - `guard_pauses_at_job_limit`: after `add(5.01)`, `check() == Pause { scope: Job, spent: 5.01 }`; after `extend(Job)`, `Go` until spent passes 10.0.
  - `guard_chat_limit_counts_earlier_spend`: `Guard::new(default, 19.0)`, `start_job()`, `add(1.5)` → `Pause { scope: Chat, .. }` though the job is under $5.
  - `start_job_resets_only_the_job`.
- [ ] **Step 2: Run** `cargo test -p daw-claude price guard` — FAIL.
- [ ] **Step 3: Implement** `price.rs` and `guard.rs`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(claude): prices and spending guard`.

### Task 3: Request builder

**Files:**
- Create: `crates/claude/src/request.rs`

**Interfaces:**
- Produces:
  - `request::Spec<'a> { model: &'a str, system: &'a str, tools: &'a [Value], web_search: bool, messages: &'a [Value], compact_now: bool }`.
  - `request::body(spec: &Spec) -> Value`; `request::betas(spec: &Spec) -> Vec<&'static str>`.
  - `request::web_search_tool(model: &str) -> Value`; `request::shows_updates(model: &str) -> bool`; `request::has_fallbacks(model: &str) -> bool`.
  - `pub const DEFAULT_MODEL: &str = "claude-haiku-5-5"`, `pub const MAX_TOKENS: u32 = 64000`, `pub const MAX_SEARCHES: u32 = 5`.

What `body` sets:
- `model`, `max_tokens: MAX_TOKENS`, `stream: true`.
- `system`: one text block with `cache_control: {"type":"ephemeral"}`.
- `tools`: the given tools, the last one with `cache_control: {"type":"ephemeral"}`, then the web search tool if `web_search` (with `max_uses: MAX_SEARCHES`).
- `messages` as given, plus top-level `cache_control: {"type":"ephemeral"}` so the newest turn is cached.
- `thinking`: `{"type":"adaptive","display":"updates","block_binding":{"prefix_mismatch_behavior":"drop_block"}}` when `shows_updates(model)`, else the same without `display`. (Fable 5.1, Fable 5, Opus 5.5 and Sonnet 5.5 show updates; on the others the between-steps commentary already comes as text.) If the API rejects `block_binding` together with `display`, follow the error and record what works in a comment.
- `context_management: {"edits":[{"type":"compact_20260112"}]}` (automatic compaction).
- `fallbacks: "default"` when `has_fallbacks(model)` (Fable 5.1, Opus 5.5, Opus 5, Sonnet 5.5).
- When `compact_now`, the on-demand compaction field (beta `compact-2026-09-04`). Read its exact shape on `https://platform.claude.com/docs/en/build-with-claude/compaction.md` before writing it.

`web_search_tool`: `{"type":"web_search_20260209","name":"web_search"}` on Opus 5.5/5/4.8/4.7/4.6, Sonnet 5.5/5/4.6 and Fable; `web_search_20250305` on everything else (Haiku). `betas` lists `compact-2026-01-12`, `thinking-binding-controls-2026-08-01`, plus `thinking-display-updates-2026-08-18` when `shows_updates`, `server-side-fallback-2026-07-01` when `has_fallbacks`, `compact-2026-09-04` when `compact_now`.

- [ ] **Step 1: Write the failing tests:**
  - `no_sampling_fields_or_tool_choice`: `body` has no `temperature`, `top_p`, `top_k`, `tool_choice` keys.
  - `last_tool_and_system_are_cached`.
  - `haiku_gets_basic_search_and_no_updates`: `web_search_tool("claude-haiku-5-5")["type"] == "web_search_20250305"`, `body["thinking"].get("display").is_none()`, no `fallbacks`.
  - `opus_gets_updates_fallbacks_and_new_search` (all three plus their betas).
  - `search_tool_only_when_toggled` and has `max_uses == 5`.
  - `same_inputs_same_bytes`: `serde_json::to_string(&body(..))` is equal across two calls (no maps with random order, no timestamps).
- [ ] **Step 2: Run** `cargo test -p daw-claude request` — FAIL.
- [ ] **Step 3: Implement** `request.rs`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(claude): request builder`.

### Task 4: Transport, models list and retries

**Files:**
- Create: `crates/claude/src/http.rs`
- Modify: `crates/claude/Cargo.toml` (add `ureq`, `rustls` with the same versions and features as `crates/app/Cargo.toml`)

**Interfaces:**
- Produces:
  - `http::Transport` trait: `fn post(&self, path: &str, headers: &[(&str, String)], body: &Value) -> Result<Box<dyn BufRead + Send>, HttpError>`; `fn get(&self, path: &str, headers: &[(&str, String)]) -> Result<Value, HttpError>`.
  - `http::HttpError { Status { code: u16, retry_after: Option<Duration>, message: String }, Network(String) }`; `HttpError::retryable(&self) -> bool` (408, 409, 429, 500–599, network).
  - `http::Ureq::new() -> Ureq` (implements `Transport`; base URL `https://api.anthropic.com`, platform verifier TLS as in `crates/app/src/update.rs`, connect timeout 15 s, per-read idle timeout 90 s, user agent `{APP_NAME}/{version}` passed in).
  - `http::headers(key: &str, betas: &[&str]) -> Vec<(&'static str, String)>` (`x-api-key`, `anthropic-version: 2026-10-21`, `content-type`, and `anthropic-beta` joined with commas when non-empty).
  - `http::ModelInfo { id: String, name: String, context: u64 }`; `http::list_models(t: &dyn Transport, key: &str) -> Result<Vec<ModelInfo>, HttpError>` (follows `has_more` / `last_id` paging; `context` from `max_input_tokens`).
  - `http::backoff(attempt: u32, retry_after: Option<Duration>) -> Duration` (1, 2, 4, 8 s; `retry_after` wins), `pub const MAX_ATTEMPTS: u32 = 4`.
  - Test helper `http::fake::Script` (`#[cfg(any(test, feature = "fake"))]`): a `Transport` that returns queued responses (`Ok(fixture bytes)` or `Err(HttpError)`) and records every request body.
- [ ] **Step 1: Write the failing tests:**
  - `headers_never_debug_print_the_key`: `format!("{:?}", HttpError::…)` and any `Debug` on types holding headers don't contain the key.
  - `models_paged`: two fake pages → three `ModelInfo`s with contexts.
  - `retryable_codes`: 429, 529, 503 and `Network` are retryable; 400, 401, 404 aren't.
  - `backoff_respects_retry_after`.
- [ ] **Step 2: Run** `cargo test -p daw-claude http` — FAIL.
- [ ] **Step 3: Implement** `http.rs`. Error bodies are read (bounded to 64 KiB) for the API's `error.message`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(claude): HTTP transport and models list`.

### Task 5: The job loop

**Files:**
- Create: `crates/claude/src/job.rs`

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces:
  - `job::Tools` trait: `fn call(&self, name: &str, input: Value) -> Result<Value, String>` (the app's answer, which may hold `$content`).
  - `job::Control { SendNow(String), Continue, Stop }`.
  - `job::Event { Text { text: String }, Thinking { text: String }, Step { id: String, name: String, input: Value }, StepDone { id: String, ok: bool }, Searched { query: String }, Spent { job: f64, chat: f64, known: bool, context_used: u64 }, Paused { scope: Scope, spent: f64 }, Saved { messages: Vec<Value>, spent: f64 }, Notice(String), Done(End) }`.
  - `job::End { Finished, Stopped, Refused(String), Failed(String) }`.
  - `job::Job { transport: Arc<dyn Transport>, key: String, model: String, system: String, tools: Vec<Value>, web_search: bool, compact_now: bool }`.
  - `job::run(job: &Job, messages: Vec<Value>, guard: &mut Guard, tools: &dyn Tools, ctl: &Receiver<Control>, out: &Sender<Event>) -> End`. The caller has already appended the person's message to `messages`.
  - `job::tool_result(id: &str, answer: Result<Value, String>) -> Value` — a `tool_result` block. The answer's `$content` is removed. Images (`{"type":"image","data","mimeType"}`) become `{"type":"image","source":{"type":"base64","media_type","data"}}`. Audio blocks are dropped and replaced by the text "Audio isn't sent to this chat; use analyse to check how it sounds." The rest of the answer goes in as JSON text. `Err(e)` gives `is_error: true`.

The loop, per request:
1. `guard.check()`: on `Pause` emit `Paused` and block on `ctl` (`Continue` → `guard.extend(scope)`, then go on; `Stop` → end).
2. Build the body (Task 3), `post`, and feed the stream to the accumulator, emitting `Text` / `Thinking` / `Step` as blocks arrive. A `Control::Stop` seen between reads ends the stream: the partial assistant message is discarded, and the usage seen so far is still added.
3. On a retryable error, sleep `backoff`, retry up to `MAX_ATTEMPTS`; each attempt's usage is added once.
4. Add the cost, emit `Spent`, append the assistant message.
5. Then by `stop_reason`:
   - `tool_use`: run each `tool_use` in order through `tools`, emitting `StepDone`. Stop seen between calls answers the rest with `is_error` "Stopped by the person.". Build one user message with every `tool_result`, then any `SendNow` texts drained from `ctl` as `text` blocks after them. Append, emit `Saved`, loop.
   - `pause_turn`: append and loop without a new user message.
   - `end_turn`, `max_tokens`: emit `Saved`, end `Finished` (with a `Notice` for `max_tokens`).
   - `refusal`: emit `Saved`, end `Refused(stop_details.explanation or category)`.
   - Status 401 / 403 → `Failed("The API key was rejected.")`.

`Saved` is only emitted where `messages` is a valid conversation (after a `tool_result` user message or a final assistant message). Task 7 only writes history on `Saved`.

- [ ] **Step 1: Write the failing tests** (all on `http::fake::Script` and a fake `Tools` that records calls):
  - `two_steps_then_end`: `tool_use.sse` then `text.sse` → tools called once, two requests sent, `End::Finished`, final `messages` has 4 entries.
  - `send_now_goes_after_the_results`: `SendNow("busier bass")` queued during the first step → the second request's last user message content is `[tool_result…, {"type":"text","text":"busier bass"}]`.
  - `audio_is_not_sent`: a tool answer with an audio `$content` → its `tool_result` has no audio block and contains the "use analyse" text.
  - `stop_after_tool_use_keeps_history_valid` (Review Focus 2): Stop arrives before the tool runs → saved messages end with a user message whose `tool_result` has `is_error: true`, and no tool ran.
  - `guard_pause_blocks_until_answered`: limits `{job: 0.0001, ..}` → `Paused` emitted, no second request until `Continue`; with `Stop`, no second request ever.
  - `retry_counts_usage_once` (Review Focus 4): `Err(Status 529)` then the fixture → one retry; `guard.job_spent()` equals the fixture's cost exactly.
  - `history_is_append_only`: for each request after the first, the previous request's `messages` is a prefix of this one, byte-for-byte as JSON.
  - `key_rejected`: `Err(Status 401)` → `End::Failed("The API key was rejected.")`.
- [ ] **Step 2: Run** `cargo test -p daw-claude job` — FAIL.
- [ ] **Step 3: Implement** `job.rs`.
- [ ] **Step 4: Run** `cargo test -p daw-claude` — PASS.
- [ ] **Step 5: Commit** `feat(claude): job loop with tools, send-now, stop and guard`.

### Task 6: API key store and settings

**Files:**
- Create: `crates/app/src/chat_key.rs`
- Modify: `crates/app/Cargo.toml` (add `daw-claude = { path = "../claude" }`, `keyring = { version = "3", features = ["windows-native", "apple-native", "linux-native"] }`), `crates/app/src/main.rs` (mod), `crates/app/src/settings.rs` (Assistants section), `crates/app/src/assistants.rs` (draw the key and limits under the existing page)

**Interfaces:**
- Produces:
  - `chat_key::KeyStore` trait: `fn get(&self) -> Option<String>; fn set(&self, key: &str) -> Result<(), String>; fn clear(&self) -> Result<(), String>`.
  - `chat_key::OsKeys` (keyring entry service `"DAW"`, user `"anthropic-api-key"`), `chat_key::MemoryKeys` for tests.
  - `chat_key::api_key(store: &dyn KeyStore) -> Option<String>` (stored key, else `ANTHROPIC_API_KEY`, else `None`; empty strings count as none).
  - `chat_key::ChatSettings { limits: Limits }` with `load() -> ChatSettings` / `save(&self)` at `config_dir().join("chat.txt")`, lines `job_limit=5` and `chat_limit=20`, in the style of `assistants.rs` `settings_path`.
  - UI on Settings → AI assistants: a masked key field with Save and Remove (`// ai: person`), a line saying where the key is kept, and the two limits as dollar fields (`// ai: person`).
- [ ] **Step 1: Write the failing tests:**
  - `env_is_the_fallback`: `MemoryKeys` empty + env set → env value; stored key wins over env.
  - `settings_file_never_holds_the_key`: set a key through the settings UI with `MemoryKeys`, save settings, read `chat.txt` and every file in the test's config dir → none contains the key.
  - `limits_round_trip`.
- [ ] **Step 2: Run** `cargo test -p daw-app chat_key` — FAIL.
- [ ] **Step 3: Implement.** The app holds `Box<dyn KeyStore>` (an `OsKeys` normally, `MemoryKeys` under `cfg(test)` and in headless mode).
- [ ] **Step 4: Run** `cargo test -p daw-app chat_key`, `cargo test -p daw-app widget` and `cargo deny check` — PASS (add `keyring`'s licences to `deny.toml` only if they're already-allowed kinds; otherwise stop and ask).
- [ ] **Step 5: Commit** `feat(app): API key in the OS credential store, chat limits`.

### Task 7: Chat history per project

**Files:**
- Create: `crates/app/src/chat_store.rs`

**Interfaces:**
- Produces:
  - `chat_store::Chat { id: String, title: String, created_ms: i64, messages: Vec<Value>, spent: f64 }`. No model field.
  - `chat_store::Store::for_project(folder: Option<&Path>) -> Store`: `config_dir()/chats/<sha256 of the canonical folder path, hex, first 16>/`; `None` (unsaved) → a memory-only store.
  - `Store::list(&self) -> Vec<(String, String, i64)>` (id, title, created, newest first); `load(&self, id: &str) -> Option<Chat>`; `save(&self, chat: &Chat) -> io::Result<()>`.
  - `chat_store::new_chat() -> Chat` (id from `daw_project::new_id()`, title "New chat" until the first message, then its first 40 characters).

`save` writes to `<id>.json.tmp` then renames, so a quit mid-write never leaves a broken file.

- [ ] **Step 1: Write the failing tests:**
  - `saved_chat_reopens`, `list_newest_first`.
  - `never_inside_the_project_folder`: after saving, the project folder's file list is unchanged.
  - `unsaved_project_writes_nothing`.
  - `reopened_history_is_valid` (Review Focus 5): feed `job::run` (fake transport) a two-step job, kill it after the first `Saved`, reload → messages end with the `tool_result` user message, every `tool_use` in them has a matching `tool_result`, and a new job started on them sends them unchanged as the prefix of its first request.
- [ ] **Step 2: Run** `cargo test -p daw-app chat_store` — FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(app): private per-project chat history`.

### Task 8: Chat controller (worker, tools through the inbox, stop rules)

**Files:**
- Create: `crates/app/src/chat.rs`
- Modify: `crates/app/src/assistants.rs` (make `INSTRUCTIONS` `pub(crate)`), `crates/app/src/commands.rs` (`Cmd::StopAssistants` also stops the chat), the project open/close path in `crates/app/src/project.rs` (call `chat_project_changed`), `crates/app/src/main.rs` (`App.chat: ChatState`, call `chat_tick` each frame next to `agent_tick`)

**Interfaces:**
- Consumes: `job::run`, `Store`, `api_key`, `Request` / `Access` / `all_tools` / `schema_of` from `crate::agent`.
- Produces:
  - `chat::api_tools() -> Vec<Value>`: every `all_tools()` entry as `{"name","description","input_schema": schema_of(t)}`, in registry order.
  - `chat::SYSTEM` text: `INSTRUCTIONS` + `" You are the chat inside the app. The person watches each step and may send messages while you work; they arrive after tool results and come from the person at the computer."`
  - `chat::chat_available(app: &App) -> Result<(), String>`. Returns `Ok(())` today, because live sessions don't exist yet (ROADMAP §1.6). Its doc comment says the live-session work must make it return `Err("Only the host can use the chat.")` for a guest.
  - `chat::ChatState { chat: Chat, store: Store, model: String /* starts as DEFAULT_MODEL */, web_search: bool, queue: Vec<String>, models: Vec<ModelInfo>, steps: Vec<StepLine>, paused: Option<(Scope, f64)>, worker: Option<Worker>, notice: Option<String>, guard_limits: Limits }`.
  - `App::chat_send(&mut self, text: String)` (starts a job if idle, else queues); `App::chat_send_now(&mut self, i: usize)`; `App::chat_stop(&mut self)`; `App::chat_continue(&mut self)`; `App::chat_new(&mut self)`; `App::chat_open(&mut self, id: &str)` (sets `model = DEFAULT_MODEL`); `App::chat_compact(&mut self)`; `App::chat_project_changed(&mut self)`; `App::chat_tick(&mut self, ctx: &egui::Context)`.
  - `chat::InboxTools { inbox: Sender<Request>, cancelled: Arc<AtomicBool> }` implements `job::Tools`: refuses with "The project changed, so the job was stopped." once `cancelled`, else sends `Request { access: Access::EditExport, .. }` and waits for the reply.

The worker thread owns a clone of the messages and sends `job::Event`s over a channel. `chat_tick` drains them, updates `steps` and the totals, writes history on `Saved`, and starts the next queued message after `Done`. When a job finishes with messages still queued, they go together as one user message.

- [ ] **Step 1: Write the failing tests** (use `agent::driver::Driver` and the fake transport):
  - `chat_call_is_one_assistant_undo_step`: a scripted `tool_use` of `add_track` → the project's newest undo label is `Assistant: …`, the same as `Driver::call("add_track", …)` gives.
  - `project_switch_stops_the_job` (Review Focus 1): script a `tool_use`, call `chat_project_changed` before the tool runs → the tool isn't applied to the newly opened project, and the job ends.
  - `stop_assistants_stops_the_chat`: `Cmd::StopAssistants` during a job → `End::Stopped`.
  - `reopened_chat_starts_on_haiku`: set `model` to `claude-fable-5-1`, `chat_open` the same chat → `model == "claude-haiku-5-5"`.
  - `queued_messages_wait_for_the_job`, and `send_now_moves_one_into_the_job` (the next request carries it after the results).
  - `api_tools_match_mcp_tools`: names and schemas equal what `Assistants::start` gives the MCP server.
- [ ] **Step 2: Run** `cargo test -p daw-app chat::` — FAIL.
- [ ] **Step 3: Implement** `chat.rs` and the hooks.
- [ ] **Step 4: Run** `cargo test -p daw-app` — PASS (whole crate: the MCP tests must still pass unchanged).
- [ ] **Step 5: Commit** `feat(app): chat controller on the agent inbox`.

### Task 9: The Assistant pane

**Files:**
- Create: `crates/app/src/chat_pane.rs`
- Modify: `crates/app/src/workspace.rs` (`Pane::Assistant`, `PANES: [Pane; 20]`, id `"assistant"`, title `"Assistant"`, dispatch arm next to `Pane::Inspector`, default region `Region::Right`), `crates/app/src/commands.rs` (`Cmd::AssistantChat` opens/closes the pane, following `Cmd::NotationSettings`), `crates/app/src/menus.rs` (an **Assistant** menu: Chat, AI assistants…, Stop assistants), `crates/app/widgets-unmarked.txt` (no new entries allowed), the timeline drawing (light highlight on clips named in the running step's input)

**Interfaces:**
- Consumes: `ChatState` and the `App::chat_*` methods from Task 8.
- Produces: `App::assistant_pane(&mut self, ui: &mut egui::Ui)`; `chat_pane::step_line(name: &str, input: &Value) -> String` (the tool name in words plus its main argument, at most 60 characters); `App::chat_highlight(&self) -> Vec<String>` (clip ids in the running step's input, under `clip_id` / `clip_ids`).

Layout, top to bottom, as in the spec's "The pane" section:
1. Top bar: model picker (all listed models; Haiku first if absent from the list), Compact, New chat, History ▾, Web search toggle, cost `$0.42` (or `12.3K tokens` when `known` is false), context gauge `used / context`.
2. Messages: the person's messages right-aligned, Claude's text streaming in, steps as tick lines (click one to expand the raw input and result), the running step highlighted.
3. Queue: each queued message with **Send now** and **✕**.
4. Input box with **Send** (or **Queue** while a job runs) and **■ Stop**.

The pause asks "This job has cost $5.12. Continue for another $5, or stop?" (and "This chat has cost $20.40. Continue for another $20, or stop?") with Continue and Stop buttons. With no key it shows "Add your Anthropic API key in Settings → AI assistants to use the chat." When `chat_available` is `Err` it shows that message and nothing else.

- [ ] **Step 1: Write the failing tests** (kittest, fake transport):
  - `pane_opens_on_the_right`: `Cmd::AssistantChat` → `workspace.layout.region_of(Pane::Assistant) == Some(Region::Right)`.
  - `no_key_message`.
  - `pause_shows_and_continue_resumes`.
  - `queued_message_has_send_now`.
  - `step_line_is_short`: `step_line("add_track", {"name":"Bass","kind":"instrument"}) == "Add track: Bass"`.
- [ ] **Step 2: Run** `cargo test -p daw-app chat_pane` — FAIL.
- [ ] **Step 3: Implement.** Every control carries `// ai: person` (picker, buttons, input, toggle) or `// ai: view` (History list, expanding a step).
- [ ] **Step 4: Run** `cargo test -p daw-app` — PASS, including `widget_check` and the layout round-trip tests.
- [ ] **Step 5: Commit** `feat(app): Assistant pane`.

### Task 10: Docs and a real-key check

**Files:**
- Modify: `docs/AI_ASSISTANTS.md` (a "Built-in chat" section: key, limits, model defaulting to Haiku, history kept private, web search toggle, that `render_audio` sends no sound to the chat, and that the chat is trusted without a prompt), `docs/ROADMAP.md` (a line under 2.11), `README.md` (one line under Documents if AI_ASSISTANTS.md is listed)

- [ ] **Step 1:** Write the docs.
- [ ] **Step 2: Real-key smoke test, by the owner** (no automated test uses the API). With a key set, run `.\run.ps1` and:
  1. Ask Haiku to "add a drum track with a 4-bar rock beat". The steps appear, the clip appears, and the cost is under $0.05.
  2. Queue a second message mid-job and press Send now. Claude acknowledges it on its next step.
  3. Turn Web search on and ask it to look something up. A "Searched:" step appears and the cost rises.
  4. Set the job limit to $0.01. The pause appears, and Stop ends the job.
  5. Switch to Opus 5.5 mid-chat and send a message. No error, and the commentary between steps is visible.
  6. Close and reopen the project. The chat is in History and reopens on Haiku.
  7. Press Compact. The context gauge drops.
- [ ] **Step 3: Commit** `docs: built-in assistant chat`.
