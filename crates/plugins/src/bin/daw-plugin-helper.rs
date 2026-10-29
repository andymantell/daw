// SPDX-License-Identifier: GPL-3.0-or-later
//! The plugin host's helper, on its own: scans one VST 3 bundle and
//! prints what it offers, or hosts plugins for another process, as the
//! app itself does when run with the same arguments. For tests, and for
//! checking a plugin by hand: `daw-plugin-helper --scan-vst3 <bundle>`.

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [flag, bundle] if flag == daw_plugins::scan::SCAN_ARG => {
            std::process::exit(daw_plugins::scan::child_main(bundle.as_ref()))
        }
        [flag] if flag == daw_plugins::host::HOST_ARG => {
            std::process::exit(daw_plugins::host::helper_main())
        }
        _ => {
            eprintln!(
                "usage: daw-plugin-helper {} <bundle> | daw-plugin-helper {}",
                daw_plugins::scan::SCAN_ARG,
                daw_plugins::host::HOST_ARG
            );
            std::process::exit(2)
        }
    }
}
