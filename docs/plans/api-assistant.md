# A built-in assistant chat on the Anthropic API (2026-11-03)

Today an assistant drives the DAW from outside: Claude Code (or any MCP
client) connects to the app's local server (`crates/agent`, see
[AI_ASSISTANTS.md](../AI_ASSISTANTS.md)) and calls its tools. This plan
adds a second way in: a chat pane inside the app that talks to Claude
directly through the Anthropic API with the owner's own API key.

## Why

The owner has API credit going spare and wants to use it on the DAW. The
chat is **another way of using the same assistant**, not a new kind of
assistant: anything Claude Code can do to the DAW through MCP, the chat
can do, and the other way round. The MCP route stays exactly as it is.

## What it is used for

Long, multi-step jobs that run for several minutes: writing whole songs,
fleshing out the skeleton of a track, building a backing track in a
style to practise over. One request may make dozens or hundreds of tool
calls. The owner wants to **watch it work** and give feedback while it
does.

## Goals

- A chat pane in the app that drives the DAW through the same tools,
  instructions and undo steps as the MCP route.
- The owner sees each step as it happens and can steer mid-job.
- Spending is visible as it goes and cannot run away.

## Non-goals

- Anything Claude Code can do outside the DAW: reading files, running
  commands. The chat has the DAW's tools, plus optional web search.
- Use by anyone but the host. People connected to a shared session never
  see or use the chat, and the key never leaves the host's machine.
  **When the owner is a guest in someone else's session, the chat is
  unavailable** (the pane says so). It only works on projects the owner
  hosts or has open alone.
- Replacing or changing the MCP server.

## Anti-goals (a failure even if it works)

- **Spending the owner's whole credit balance in one go.** The spending
  guard below is a hard requirement, not polish.
- Two routes that drift apart: a tool, instruction or undo behaviour that
  works one way and not the other.
- The API key written anywhere it could travel: a project folder, a
  bundle, a log, a crash report.

## The pane (owner chose layout A)

A tab in the **right-hand region** of the main window (`Region::Right` in
`crates/app/src/workspace.rs`), beside the timeline at full height,
resizable and closable like the other right-hand tabs. Opened from an
**Assistant** menu. The mockup is in `.superpowers/brainstorm/` (not
committed); the description here is what counts.

**Top bar**

- **Model picker**, filled from the Models API (`GET /v1/models`) so new
  models appear without an app update.
- **Compact**, **New chat**, **History ▾** (this project's earlier chats).
- **Web search** toggle, off by default.
- **Running cost** of this chat in dollars, and a **context gauge**
  showing how full the conversation is.

**The conversation**

- Your messages, and Claude's replies streaming in as written.
- Each tool call shown as one plain line with a tick ("Bass: walking
  line, bars 1–12"); clicking it shows the raw call and result. The step
  in progress is highlighted.
- Clips Claude is working on get a light highlight on the timeline while
  it works (a hint only, not a new feature).

**Below the conversation**

- **Queued messages.** Anything typed while a job runs waits in a queue
  above the input box and is sent when the job finishes. Each queued
  message has **Send now**, which slips it in at the next pause between
  steps so Claude adjusts mid-job, and **✕** to drop it.
- The input box, **Queue/Send**, and **Stop**, which halts the job
  cleanly (the current step finishes or is abandoned; nothing half-done
  is left without an undo step).

## Behaviour

### Same tools, same rules as MCP

- Tool definitions come from the one registry, `all_tools()` in
  `crates/app/src/agent.rs`. No second list.
- Calls go down the same path as MCP calls (`Event::Call` into the UI
  thread, answered between frames), so each project-changing request is
  one **"Assistant: …"** undo step, exactly as now.
- **One accepted difference: audio.** `render_audio`
  (`crates/app/src/listen.rs`) sends short renders back to MCP clients
  as an `audio` content block. A Messages API tool result can't carry
  audio, so the chat gets only the text part (file path and details) and
  checks its work with `analyse`. The tool stays in the one registry;
  the chat drops the audio block when it builds the tool result.
  AI_ASSISTANTS.md says so. Owner's choice (2026-11-03).
- The system prompt starts from the same `INSTRUCTIONS` text the MCP
  server sends (`crates/app/src/assistants.rs`), so both routes behave
  alike.
- **Trusted automatically.** The built-in chat is the owner's own, so it
  does not get the look / edit / export permission prompt. It has full
  access including export (my call: it is the owner's chat on the
  owner's machine). "Stop assistants" (Ctrl+Shift+Backspace) also stops
  the chat.

### Model

- **Every chat starts on Claude Haiku 5.5** (`claude-haiku-5-5`), the
  cheapest model. That includes new chats, app restarts, and earlier
  chats reopened from History, whatever model they last ran on. The
  owner's choice is deliberately **not remembered**, so picking an
  expensive model once never sticks.
- Switching model mid-chat is allowed and applies from the next request.
- Thinking is adaptive. Effort is left at each model's default unless
  the builder finds a reason to expose it (builder's call).

### Spending guard (hard requirement)

- **Per job: $5 by default.** A job is one message and everything Claude
  does in answer to it. After every API call the app adds up the cost;
  once a job passes its limit it pauses and asks: "This job has cost
  $5.12. Continue for another $5, or stop?" Nothing more is sent until
  the owner answers.
- **Per chat: $20 by default**, same pause, across the whole chat.
- Both limits are editable in settings and remembered.
- The budget is **not** passed to Claude as an API task budget. (Changed
  while planning: Anthropic advises against task budgets in interactive
  chats, because the countdown after each step makes the model distrust
  messages typed mid-job, which is what "Send now" does.) The app's own
  check is the guard.
- One call can overshoot the limit slightly (cents, not dollars). No
  call may be started once a limit is passed.

### Cost and context

- The API returns token counts (input, output, cache read, cache write)
  but not money, and the Models API has no prices. The app holds a
  **price table** per model and multiplies. A model missing from the
  table shows token counts instead of dollars, and **the spending guard
  must still hold for it**: the builder decides how (for example,
  pricing unknown models at the most expensive known rate).
- Web search fees count towards the running cost and the limits.
- The context gauge shows the conversation's size against the model's
  context window (from the Models API).

### Compaction

- **Automatic compaction** on (the API summarises older parts of the
  chat when it gets large).
- **Compact** button to do it on demand, for example between sections of
  a song.
- **New chat** starts fresh; the old one stays in History.
- Prompt caching on the tool list and system prompt, since they are
  large (about 118 tools) and the same on every request.

### Web search

- Anthropic's server-side web search tool, offered only when the toggle
  is on. Use the newest tool version each model supports (Haiku gets the
  basic one).
- Capped at a few searches per job (builder picks the number).
- Shown as steps ("Searched: 12-bar blues shuffle drum patterns").

### History (private)

- Chats are kept **per project** and reopen with the project, so "now
  add a bridge" still knows the earlier conversation.
- Stored **only in the app's own data folder** (`config_dir()` /
  `%LOCALAPPDATA%`, builder's choice), never in the project folder, so
  they don't travel when the project is shared or bundled.
- How a chat is matched to its project (stable id vs folder path) is the
  builder's call; surviving a project being moved or renamed is nice to
  have, not required.

### API key

- Entered in the Settings window (the Assistants section), stored in the
  OS credential store (Windows Credential Manager; the macOS and Linux
  equivalents), with `ANTHROPIC_API_KEY` as a fallback when nothing is
  stored. Never written to settings files, projects, logs or crash
  reports.
- A clear message in the pane when no key is set, or the key is rejected.

## How it gets made (decided)

- Rust, in the existing crates. Anthropic has no Rust SDK, so the app
  calls `POST /v1/messages` over HTTP with streaming (server-sent
  events), using `ureq`, which the app already uses. No async runtime;
  a background thread per running chat, like the MCP server's threads.
- The MCP server and its protocol are unchanged.

## Left to the builder

- Module and crate boundaries (a new crate for the API client, or a
  module in `daw-app`).
- How "Send now" is put into the conversation at the next step boundary,
  as long as it reaches Claude before its next step.
- Price table format and how it's kept up to date.
- The exact search cap, error and retry handling, and how a refusal or
  rate limit is shown.
- Tests. At minimum: the spending guard (including an unknown model),
  that the key never reaches disk outside the credential store, and that
  a tool call from the chat makes the same undo step as one from MCP.
- Updating [AI_ASSISTANTS.md](../AI_ASSISTANTS.md) to describe both
  routes.
