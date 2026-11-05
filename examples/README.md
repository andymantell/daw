# Examples

Projects that come with the repository, each a project folder the app
opens with File → Open project….

- `house-demo`: a layered house track built through the assistant
  interface by `crates/app/src/demo_song.rs` (which rebuilds it:
  `cargo test -p daw-app --bin daw demo_song::write_house_demo -- --ignored`).
  The vocal is a synthesised phrase, so everything in it is the app's own.
