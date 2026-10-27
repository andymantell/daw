// SPDX-License-Identifier: GPL-3.0-or-later
fn main() {
    // The dark variant, so menus and dialogs match the dark timeline.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/timeline.slint", config).unwrap();
}
