// SPDX-License-Identifier: GPL-3.0-or-later
//! Scans one VST 3 bundle and prints what it offers, as the app's own
//! scanner child does. For tests and for checking a plugin by hand:
//! `daw-plugin-scan --scan-vst3 <bundle>`.

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [flag, bundle] if flag == daw_plugins::scan::SCAN_ARG => {
            std::process::exit(daw_plugins::scan::child_main(bundle.as_ref()))
        }
        _ => {
            eprintln!(
                "usage: daw-plugin-scan {} <bundle>",
                daw_plugins::scan::SCAN_ARG
            );
            std::process::exit(2)
        }
    }
}
