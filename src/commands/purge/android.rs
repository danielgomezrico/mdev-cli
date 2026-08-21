use std::path::Path;

use crate::commands::purge::common::delete_paths;
use crate::commands::purge::PurgeArgs;
use crate::logger::Logger;

pub fn run(args: &PurgeArgs, root: &Path, dry_run: bool, verbose: bool) {
    let logger = Logger::new();

    let explicit_flags = args.flutter || args.pub_cache || args.gradle || args.android || args.ios;
    let do_android_build = !explicit_flags || args.android;

    if do_android_build {
        delete_paths(
            &[
                root.join("build"),
                root.join("app").join("build"),
                root.join(".gradle"),
                root.join(".dart_tool"),
            ],
            dry_run,
            verbose,
            &logger,
        );
    }
}
