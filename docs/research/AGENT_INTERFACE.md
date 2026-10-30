# Research: agent interface (MCP server)

Checked 2026-10-30, for a feature request: expose everything the app can do
so an AI assistant can operate it, perceive the state of a mix (for example
analyse a track's spectrum and adjust an EQ), and help with testing.

## Findings

- **The protocol.** The Model Context Protocol is an open protocol that
  "enables seamless integration between LLM applications and external data
  sources and tools". It uses JSON-RPC 2.0 between hosts (the assistant
  app), clients and servers. Servers offer **resources** ("context and
  data"), **prompts** ("templated messages and workflows") and **tools**
  ("functions for the AI model to execute"). Clients may offer elicitation
  (the server asking the user for more input) and sampling. The spec also
  defines subscriptions and notifications, progress, cancellation and
  long-running tasks. *(Source: the protocol specification, revision
  2026-07-28, read 2026-10-30.)*
- **Licence.** The specification and schema repository says "This project
  is licensed under the MIT License." Compatible with our GPL-3.0-or-later.
- **Transports.** The spec defines stdio (a local child process) and a
  streamable HTTP transport. The HTTP transport has an authorization
  framework; stdio servers "SHOULD NOT follow this specification, and
  instead retrieve credentials from the environment". The protocol is
  stateless by design: every request carries what it needs, and a
  connection is not a session.
- **Security principles in the spec:** user consent and control over all
  data access and operations; hosts must obtain consent before exposing
  user data or invoking any tool; tool descriptions from a server are
  untrusted unless the server is trusted.
- **Rust implementation.** An official Rust SDK exists (`rmcp`) with stdio
  and streamable HTTP server transports. Its licence is not stated on the
  page we read; check the LICENSE file before adopting. We can also
  implement the JSON-RPC surface ourselves; the schema is published as
  TypeScript and JSON Schema.

## Design consequences

- **Transport choice.** The app is long-running with a window, so the
  server is a local HTTP endpoint on the loopback address, with a token
  shown in settings, like the phone remote's pairing. A small stdio
  launcher can be offered later for assistants that only spawn processes.
- **Commands first.** The app's command table (crates/app/src/commands.rs)
  is the natural tool list: every row becomes a tool with a JSON schema for
  its arguments. Queries need adding as a read side.
- **Perception beyond state.** Reading the project document is not enough
  for mixing help. Add analysis tools that compute on demand from the
  engine: spectrum (averaged over a range), loudness, peak and RMS,
  correlation, and "mix minus one track". Rendered audio of a region as a
  file lets an assistant that can hear listen.
- **Consent model.** A per-session permission level (read; edit; edit and
  export), a visible "assistant" marker on undo entries, one-key stop, and
  no code execution from the assistant side. This matches the spec's
  principles and the project's "never execute content received from
  peers" rule.
- **Testing.** The same tools make end-to-end tests possible without
  driving the UI: a headless app plus a script of tool calls, with
  deterministic rendering compared to golden files. It also lets an
  assistant run a build on the owner's machine and report what happened.

## Not researched yet

- The Rust SDK's licence and maturity, versus writing the server by hand.
- Which assistant apps support local streamable HTTP servers with a token,
  and which need a stdio launcher.
- Whether the MCP Apps extension (interactive UI in conversations) is
  useful for showing meters or spectra inside an assistant's chat.
