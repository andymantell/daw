# Contributing

Thanks for helping. Please read these before sending a change.

## Ground rules

- **Clean-room policy.** Read [docs/CLEAN_ROOM.md](docs/CLEAN_ROOM.md).
  In short: no copying code, assets or text from other products, no
  reverse engineering, and no using copyleft DAW projects' source as
  implementation reference.
- **Licence.** The project is GPL-3.0-or-later
  ([decision 0001](docs/decisions/0001-project-licence.md)). By
  contributing you agree your contribution is under the same licence.
  New dependencies must be GPLv3-compatible; CI checks this
  (`deny.toml`).
- **Code of conduct.** See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Sign-off (DCO)

Every commit needs a `Signed-off-by` line, certifying that you have the
right to submit it under the project licence, as set out in the
Developer Certificate of Origin (https://developercertificate.org/).
Add it with:

```
git commit -s
```

CI rejects pull requests with unsigned commits.

## Before you push

From the repository root:

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Source files start with `// SPDX-License-Identifier: GPL-3.0-or-later`.

## Design changes

Significant design decisions get a short record in
[docs/decisions/](docs/decisions/) using the
[template](docs/decisions/TEMPLATE.md).
